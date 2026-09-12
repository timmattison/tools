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
/// Returns an error if any request fails, if a response cannot be parsed, or
/// if the server stops making progress before the collection is exhausted.
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
/// Returns an error if any request fails, if a response cannot be parsed, or
/// if the server stops making progress before the collection is exhausted.
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
async fn collect_pages<T, F, Fut>(mut fetch_page: F) -> Result<Vec<T>>
where
    F: FnMut(u64) -> Fut,
    Fut: Future<Output = Result<Page<T>>>,
{
    let mut items: Vec<T> = Vec::new();

    loop {
        let page = fetch_page(collected(&items)).await?;

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
