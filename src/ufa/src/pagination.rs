//! Exhaustive retrieval of the UniFi API's paginated collections.
//!
//! Every list endpoint answers with a [`Page<T>`], which carries at most a
//! server-chosen slice of the collection (25 items by default). Callers that
//! want "all the devices" or "all the sites" must therefore walk the pages
//! themselves — and every place in this crate that forgot to do so silently
//! reported a truncated answer as if it were complete.
//!
//! This module exists so that walking is done exactly once. Callers name the
//! collection they want and receive all of it; offsets, page sizes, and
//! misbehaving-server guards stay in here.

use anyhow::Result;
use serde::de::DeserializeOwned;
use std::future::Future;

use crate::{client::UnifiClient, models::Page};

/// Items requested per round trip.
///
/// The server is free to answer with fewer — the walk advances by what it
/// actually received, so a server-side cap costs extra requests rather than
/// correctness — but asking for a full page keeps a large site down to a
/// handful of requests instead of dozens.
const PAGE_SIZE: u32 = 200;

/// The most items one walk reads from one collection.
///
/// What a collection can hold is bounded by the controller behind it. A
/// console adopts devices and serves clients in the thousands, and a site
/// keeps its vouchers in the thousands too. One million is far above each of
/// them, and that distance matters more than the number itself, as it does for
/// [`crate::http`]'s bound on one body. What it bounds is the rest: a server
/// that states a total with no end and answers each request with a full page.
const MAX_COLLECTION_ITEMS: u64 = 1_000_000;

/// Fetch every item of the paginated collection at `path`.
///
/// # Arguments
///
/// * `client` - The controller client to issue the requests with.
/// * `path` - Collection path relative to the API root, e.g. `sites/…/devices`.
///
/// # Returns
///
/// Every item in the collection, in the order the server returned them.
///
/// # Errors
///
/// Returns an error if any request fails, if a response cannot be parsed, if
/// the server stops making progress before the collection is exhausted, or if
/// it states more than [`MAX_COLLECTION_ITEMS`] items.
pub async fn fetch_all<T>(client: &UnifiClient, path: &str) -> Result<Vec<T>>
where
    T: DeserializeOwned,
{
    fetch_all_matching(client, path, None).await
}

/// Fetch every item of the collection at `path` that `filter` selects.
///
/// The same exhaustive walk as [`fetch_all`], with the server-side filter
/// expression applied. A caller that is about to act on "everything that
/// matches" needs the whole match, not the first page of it.
///
/// # Arguments
///
/// * `client` - The controller client to issue the requests with.
/// * `path` - Collection path relative to the API root.
/// * `filter` - The API's filter expression, or `None` for the whole
///   collection.
///
/// # Returns
///
/// Every matching item, in the order the server returned them.
///
/// # Errors
///
/// Returns an error if any request fails, if a response cannot be parsed, if
/// the server stops making progress before the collection is exhausted, or if
/// it states more than [`MAX_COLLECTION_ITEMS`] items.
pub async fn fetch_all_matching<T>(
    client: &UnifiClient,
    path: &str,
    filter: Option<&str>,
) -> Result<Vec<T>>
where
    T: DeserializeOwned,
{
    collect_pages(|offset| {
        let offset = offset.to_string();
        let limit = PAGE_SIZE.to_string();
        async move {
            let mut params: Vec<(&str, &dyn std::fmt::Display)> =
                vec![("limit", &limit), ("offset", &offset)];
            if let Some(expression) = filter.as_ref() {
                params.push(("filter", expression));
            }
            client.get_with_params(path, &params).await
        }
    })
    .await
}

/// Accumulate every page produced by `fetch_page` into a single collection.
///
/// `fetch_page` is called with the offset of the next item wanted, which makes
/// the walking logic independent of the transport and therefore testable
/// without a network.
///
/// The walk advances by the number of items actually received rather than by
/// the page size it asked for, so a server that caps or ignores the requested
/// limit still yields a complete answer. A page that comes back empty while
/// the server's own `totalCount` says more items exist means the server has
/// stopped making progress; that is reported as an error instead of being
/// looped on forever or quietly truncated.
///
/// Every page also has to be the page that was asked for. A server that
/// ignores the offset and answers the first page every time sends a full page
/// each time, so the empty-page guard never fires: the walk collects the same
/// items over and over until the count reaches `totalCount`, and reports that
/// pile as the collection. The page's own `offset` is what tells the two
/// apart, so it is compared against the offset the walk asked for.
///
/// A server that passes both of those guards can still state a total with no
/// end, and answer each request with a full page. So each page must state a
/// total of at most [`MAX_COLLECTION_ITEMS`]. The walk goes on only while it
/// holds fewer items than that total, and each page adds at least one item, so
/// the check on each page bounds the walk. A check on the first page alone
/// does not, because a server can raise the total on a later page.
async fn collect_pages<T, F, Fut>(mut fetch_page: F) -> Result<Vec<T>>
where
    F: FnMut(u64) -> Fut,
    Fut: Future<Output = Result<Page<T>>>,
{
    let mut items: Vec<T> = Vec::new();

    loop {
        let wanted = collected(&items);
        let page = fetch_page(wanted).await?;

        anyhow::ensure!(
            page.offset == wanted,
            "The server answered the request for items from offset {wanted} with the page at \
             offset {}. It is not advancing through the collection, so the answer repeats the \
             items it keeps sending and leaves the rest of them out.",
            page.offset
        );

        anyhow::ensure!(
            page.total_count <= MAX_COLLECTION_ITEMS,
            "The server states {} items in the collection, more than the {MAX_COLLECTION_ITEMS} \
             items ufa reads in one walk. Ask for a smaller part of the collection, or check the \
             server that answered.",
            page.total_count
        );

        if page.data.is_empty() {
            anyhow::ensure!(
                collected(&items) >= page.total_count,
                "The server returned an empty page after {} of {} items. \
                 It is not making progress through the collection, so the answer would be incomplete.",
                collected(&items),
                page.total_count
            );
            return Ok(items);
        }

        items.extend(page.data);

        if collected(&items) >= page.total_count {
            return Ok(items);
        }
    }
}

/// How many items have been collected so far, as an offset.
fn collected<T>(items: &[T]) -> u64 {
    u64::try_from(items.len()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A stand-in for the server's own page size.
    const SERVER_PAGE_SIZE: usize = 25;

    /// Build the page of `items` that starts at `offset`, as a well-behaved
    /// server would.
    fn page_at(items: &[u32], offset: u64, page_size: usize) -> Page<u32> {
        let start = usize::try_from(offset).unwrap().min(items.len());
        let end = (start + page_size).min(items.len());
        let data = items[start..end].to_vec();

        Page {
            offset,
            limit: u32::try_from(page_size).unwrap(),
            count: u32::try_from(data.len()).unwrap(),
            total_count: u64::try_from(items.len()).unwrap(),
            data,
        }
    }

    #[tokio::test]
    async fn collects_items_from_every_page() {
        let items: Vec<u32> = (0..57).collect();

        let collected = collect_pages(|offset| {
            let page = page_at(&items, offset, SERVER_PAGE_SIZE);
            async move { Ok(page) }
        })
        .await
        .expect("a well-behaved server must not fail");

        assert_eq!(
            collected, items,
            "a collection spanning several pages must be returned in full"
        );
    }

    #[tokio::test]
    async fn stops_when_the_last_page_exactly_fills_a_page() {
        let items: Vec<u32> = (0..50).collect();
        let requests = Cell::new(0_u32);

        let collected = collect_pages(|offset| {
            requests.set(requests.get() + 1);
            let page = page_at(&items, offset, SERVER_PAGE_SIZE);
            async move { Ok(page) }
        })
        .await
        .expect("a well-behaved server must not fail");

        assert_eq!(
            collected, items,
            "an exact multiple of the page size must be returned in full"
        );
        assert_eq!(
            requests.get(),
            2,
            "the walk must stop once the reported total is reached, not probe for an extra empty page"
        );
    }

    #[tokio::test]
    async fn an_empty_collection_yields_no_items() {
        let items: Vec<u32> = Vec::new();
        let requests = Cell::new(0_u32);

        let collected = collect_pages(|offset| {
            requests.set(requests.get() + 1);
            let page = page_at(&items, offset, SERVER_PAGE_SIZE);
            async move { Ok(page) }
        })
        .await
        .expect("an empty collection is not an error");

        assert!(collected.is_empty(), "an empty collection must stay empty");
        assert_eq!(
            requests.get(),
            1,
            "an empty collection needs a single request"
        );
    }

    /// A server that ignores the offset and answers the first page every time
    /// defeats the empty-page guard above: every page it sends is full, so the
    /// walk never sees one that is empty. It collects the first page over and
    /// over until the count reaches the stated total, then reports the pile as
    /// the collection. Most of it is the same items again, and the rest of the
    /// collection was never read.
    #[tokio::test]
    async fn a_server_that_ignores_the_requested_offset_is_an_error() {
        let items: Vec<u32> = (0..57).collect();

        let collected = collect_pages(|_offset| {
            // The first page, whatever was asked for.
            let page = page_at(&items, 0, SERVER_PAGE_SIZE);
            async move { Ok(page) }
        })
        .await;

        let error =
            collected.expect_err("a server that ignores the offset must be reported, not believed");
        let report = format!("{error:#}");
        assert!(
            report.contains("25"),
            "the failure should say which offset was asked for, got {report}"
        );
        assert!(
            report.contains('0'),
            "the failure should say which offset came back, got {report}"
        );
    }

    /// A server that answers each request with a full page at the correct
    /// offset passes both guards above. When it also states a total that has
    /// no end, `--all` sends requests and keeps items until the memory of the
    /// machine runs out.
    ///
    /// The fake answers the second request with an error, so a walk with no
    /// cap fails here at once. It does not run until the memory runs out.
    #[tokio::test]
    async fn a_total_over_the_cap_is_refused_at_the_first_page() {
        let requests = Cell::new(0_u32);

        let collected = collect_pages(|offset| {
            requests.set(requests.get() + 1);
            let request = requests.get();
            async move {
                anyhow::ensure!(request == 1, "the walk sent request {request} past the cap");
                Ok(Page {
                    offset,
                    limit: u32::try_from(SERVER_PAGE_SIZE).unwrap(),
                    count: u32::try_from(SERVER_PAGE_SIZE).unwrap(),
                    total_count: u64::MAX,
                    data: vec![0_u32; SERVER_PAGE_SIZE],
                })
            }
        })
        .await;

        let error = collected.expect_err("a total with no end must be refused, not walked");
        let report = format!("{error:#}");
        assert!(
            report.contains(&MAX_COLLECTION_ITEMS.to_string()),
            "the failure must name the cap, got {report}"
        );
        assert!(
            report.contains(&u64::MAX.to_string()),
            "the failure must name the total the server stated, got {report}"
        );
        assert_eq!(requests.get(), 1, "the walk must stop at the first page");
    }

    /// A server can state a small total first and raise it on a later page, so
    /// the cap holds for each page and not only for the first one.
    #[tokio::test]
    async fn a_total_that_grows_past_the_cap_is_refused() {
        let collected = collect_pages(|offset| async move {
            let total_count = if offset == 0 {
                10
            } else {
                MAX_COLLECTION_ITEMS + 1
            };
            Ok(Page {
                offset,
                limit: 1,
                count: 1,
                total_count,
                data: vec![0_u32],
            })
        })
        .await;

        let error = collected.expect_err("a total that grows past the cap must be refused");
        assert!(
            format!("{error:#}").contains(&MAX_COLLECTION_ITEMS.to_string()),
            "the failure must name the cap, got {error:#}"
        );
    }

    #[tokio::test]
    async fn a_server_that_stops_making_progress_is_an_error() {
        // A misbehaving server: it claims a hundred items but hands back an
        // empty page every time. Looping until the total is reached would
        // never terminate.
        let collected = collect_pages(|offset| async move {
            Ok(Page {
                offset,
                limit: u32::try_from(SERVER_PAGE_SIZE).unwrap(),
                count: 0,
                total_count: 100,
                data: Vec::<u32>::new(),
            })
        })
        .await;

        let error = collected.expect_err("a non-advancing server must be reported, not looped on");
        let report = format!("{error:#}");
        assert!(
            report.contains("100"),
            "the failure should say how many items were expected, got {report}"
        );
    }
}
