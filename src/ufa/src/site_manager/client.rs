use crate::http::{build_client, read_json_response, Api, Timeouts};
use crate::site_manager::models::{Host, HostResponse, HostsResponse};
use anyhow::{Context, Result};
use reqwest::{Client, Url};
use serde::de::DeserializeOwned;
use std::collections::HashSet;

/// The Site Manager API path segment that cloud hosts live under.
const HOSTS_SEGMENT: &str = "hosts";

/// The query parameter that carries the token of the next page of a listing.
const NEXT_TOKEN_PARAMETER: &str = "nextToken";

/// Path segments that name no resource, and that a path-segment builder
/// silently discards rather than appending.
const NON_SEGMENTS: [&str; 3] = ["", ".", ".."];

/// Append `segments` to `base_url` as percent-encoded path segments.
///
/// Every segment is encoded in full -- slashes, question marks, hashes and
/// non-ASCII alike -- so a segment taken from user input can only ever name a
/// child of `base_url`, never steer the request elsewhere. This is what
/// [`Url::join`] deliberately does *not* do: `join` treats its argument as a
/// relative reference, where `../`, a leading `/`, `?` and `#` all carry
/// meaning.
///
/// # Arguments
///
/// * `base_url` - The API base URL, which must end in a slash.
/// * `segments` - The path segments to append, unencoded.
///
/// # Returns
///
/// The URL naming the resource at `base_url` + `segments`.
///
/// # Errors
///
/// Returns an error if `base_url` cannot be a base (so it has no path to
/// append to), or if any segment names no resource -- empty, `.` or `..` --
/// since those would be dropped and quietly address the parent collection
/// instead.
fn api_url(base_url: &Url, segments: &[&str]) -> Result<Url> {
    for segment in segments {
        if NON_SEGMENTS.contains(segment) {
            anyhow::bail!("{segment:?} does not name a Site Manager API resource");
        }
    }

    let mut url = base_url.clone();
    {
        let mut path = url.path_segments_mut().map_err(|()| {
            anyhow::anyhow!("Site Manager API base URL {base_url} cannot be a base")
        })?;
        // The base URL ends in a slash, i.e. a trailing empty segment; without
        // dropping it the appended segments would follow a doubled slash.
        path.pop_if_empty().extend(segments);
    }

    Ok(url)
}

/// Build the request URL for a single cloud host.
///
/// # Arguments
///
/// * `base_url` - The Site Manager API base URL.
/// * `id` - The host/console id, exactly as the user supplied it.
///
/// # Returns
///
/// The URL of that host's resource.
///
/// # Errors
///
/// Returns an error if the URL cannot be constructed, or if `id` names no
/// host.
fn host_url(base_url: &Url, id: &str) -> Result<Url> {
    api_url(base_url, &[HOSTS_SEGMENT, id])
}

/// Build the request URL for the cloud host listing.
///
/// # Arguments
///
/// * `base_url` - The Site Manager API base URL.
///
/// # Returns
///
/// The URL of the host collection.
///
/// # Errors
///
/// Returns an error if the URL cannot be constructed.
fn hosts_url(base_url: &Url) -> Result<Url> {
    api_url(base_url, &[HOSTS_SEGMENT])
}

pub struct SiteManagerClient {
    client: Client,
    base_url: Url,
}

impl SiteManagerClient {
    pub fn new(api_key: &str) -> Result<Self> {
        // The Site Manager API is a public host with a public certificate, so
        // nothing here ever waives verification.
        let client = build_client(Api::SiteManager, api_key, false, Timeouts::PRODUCTION)?;

        let base_url =
            Url::parse("https://api.ui.com/v1/").context("Failed to parse Site Manager API URL")?;

        Ok(Self { client, base_url })
    }

    /// A client that sends `api_key` to a Site Manager API at `origin`, in
    /// place of `api.ui.com`.
    ///
    /// The base URL is the one thing a test has to replace, and
    /// [`SiteManagerClient::new`] names `api.ui.com` and nothing else.
    /// Everything else about the client -- the key header, the refused
    /// redirect, the bounds on the request -- comes from the same
    /// `build_client` a command uses.
    ///
    /// # Arguments
    ///
    /// * `origin` - The origin of the test server, such as
    ///   `http://127.0.0.1:52341`.
    /// * `api_key` - The key the client sends.
    ///
    /// # Returns
    ///
    /// The client.
    ///
    /// # Panics
    ///
    /// Panics if the key cannot go in a header, and if the origin is not a
    /// base URL.
    #[cfg(test)]
    pub(crate) fn talking_to(origin: &str, api_key: &str) -> Self {
        let client = build_client(Api::SiteManager, api_key, false, Timeouts::PRODUCTION)
            .expect("a plain key must build a Site Manager client");
        let base_url =
            Url::parse(&format!("{origin}/v1/")).expect("a loopback origin is a base URL");

        Self { client, base_url }
    }

    /// Fetch every cloud host on the account.
    ///
    /// The API answers one page of hosts (25 by default) and, when more
    /// follow, a `nextToken` for the next page. This method follows the
    /// tokens. The first request carries no query. Each later request carries
    /// the one pair `nextToken=<token>`, with the token percent-encoded as one
    /// value. The hosts of each page are appended in order, and the walk
    /// stops when an answer has no token or an empty one. The request sends
    /// no `pageSize`: the spec states no maximum for it, and the default costs
    /// only more requests.
    ///
    /// One guard protects the walk. The method keeps each token that it has
    /// sent, and an answer that gives one of them again is an error, because
    /// the walk would read the same pages again without end. There is no
    /// guard against an empty page that carries a token, because a paged
    /// store can answer an empty page before the end of the listing, and the
    /// walk follows it. There is no bound on the number of pages either.
    ///
    /// # Returns
    ///
    /// Every cloud host on the account, in the order the pages gave them.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails, if an answer cannot be read, or
    /// if an answer gives a page token that the walk already sent.
    pub async fn get_hosts(&self) -> Result<Vec<Host>> {
        let first = hosts_url(&self.base_url)?;
        let mut hosts = Vec::new();
        let mut sent_tokens: HashSet<String> = HashSet::new();
        let mut url = first.clone();

        loop {
            let page: HostsResponse = self.get(url).await?;
            hosts.extend(page.data);

            let Some(token) = page.next_token.filter(|token| !token.is_empty()) else {
                return Ok(hosts);
            };
            if !sent_tokens.insert(token.clone()) {
                anyhow::bail!(
                    "The Site Manager API gave the page token {token:?} a second time, so the \
                     walk would read the same pages again."
                );
            }

            url = first.clone();
            url.query_pairs_mut()
                .append_pair(NEXT_TOKEN_PARAMETER, &token);
        }
    }

    /// Fetch one cloud host.
    ///
    /// # Arguments
    ///
    /// * `id` - The host id, exactly as the user supplied it.
    ///
    /// # Returns
    ///
    /// The host that `id` names. The API gives it in the `data` key.
    ///
    /// # Errors
    ///
    /// Returns an error if `id` names no host, if the request fails, or if
    /// the answer cannot be read.
    pub async fn get_host(&self, id: &str) -> Result<Host> {
        let url = host_url(&self.base_url, id)?;
        let response: HostResponse = self.get(url).await?;

        Ok(response.data)
    }

    async fn get<T>(&self, url: Url) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Failed to send request to Site Manager API")?;

        read_json_response(Api::SiteManager, response).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{json_response, TestServer};

    /// The Site Manager API base URL every request is built from.
    fn base_url() -> Url {
        Url::parse("https://api.ui.com/v1/").expect("the base URL is a constant")
    }

    /// Assert that `id` is carried as exactly one path segment underneath
    /// `/v1/hosts/`, whatever punctuation it happens to contain.
    ///
    /// A host id arrives straight from the command line, so anything it can do
    /// to the request URL -- climb out of the `hosts/` collection, start a
    /// query string, replace the path outright -- is an injection into a
    /// request made with the user's API key.
    fn assert_id_stays_one_segment_under_hosts(id: &str) {
        let url = host_url(&base_url(), id)
            .unwrap_or_else(|error| panic!("a host id of {id:?} must still build a URL: {error}"));

        assert_eq!(
            url.host_str(),
            Some("api.ui.com"),
            "a host id of {id:?} redirected the request to another host: {url}"
        );
        assert_eq!(
            url.query(),
            None,
            "a host id of {id:?} injected a query string: {url}"
        );
        assert_eq!(
            url.fragment(),
            None,
            "a host id of {id:?} injected a fragment: {url}"
        );

        let segments: Vec<&str> = url
            .path_segments()
            .unwrap_or_else(|| panic!("a host id of {id:?} produced a cannot-be-a-base URL: {url}"))
            .collect();

        assert_eq!(
            segments.len(),
            3,
            "a host id of {id:?} must stay a single segment under /v1/hosts/, got {segments:?} from {url}"
        );
        assert_eq!(
            &segments[..2],
            &["v1", HOSTS_SEGMENT],
            "a host id of {id:?} escaped the hosts path: {url}"
        );
        assert!(
            !segments[2].is_empty(),
            "a host id of {id:?} vanished from the request path: {url}"
        );
    }

    #[test]
    fn a_plain_host_id_lands_directly_under_hosts() {
        let url = host_url(
            &base_url(),
            "900A6F00301A0000000004D1BC3A0000000004E6BA8E000000005E8B2D3B",
        )
        .expect("a plain host id must build a URL");

        assert_eq!(
            url.as_str(),
            "https://api.ui.com/v1/hosts/900A6F00301A0000000004D1BC3A0000000004E6BA8E000000005E8B2D3B"
        );
    }

    /// Encoding the host id must not have moved the listing endpoint.
    #[test]
    fn the_host_listing_url_is_unchanged() {
        let url = hosts_url(&base_url()).expect("the host listing URL must build");

        assert_eq!(url.as_str(), "https://api.ui.com/v1/hosts");
    }

    #[test]
    fn a_traversing_host_id_cannot_climb_out_of_the_hosts_path() {
        assert_id_stays_one_segment_under_hosts("../../v1/something");
    }

    #[test]
    fn a_host_id_with_a_slash_cannot_add_a_path_segment() {
        assert_id_stays_one_segment_under_hosts("a/b");
    }

    #[test]
    fn a_host_id_with_a_question_mark_cannot_start_a_query_string() {
        assert_id_stays_one_segment_under_hosts("a?b=c");
    }

    #[test]
    fn a_host_id_with_a_hash_cannot_start_a_fragment() {
        assert_id_stays_one_segment_under_hosts("a#frag");
    }

    #[test]
    fn a_leading_slash_cannot_replace_the_whole_path() {
        assert_id_stays_one_segment_under_hosts("/absolute");
    }

    #[test]
    fn a_host_id_with_a_space_is_encoded_rather_than_split() {
        assert_id_stays_one_segment_under_hosts("has space");
    }

    #[test]
    fn a_multi_byte_host_id_is_encoded_rather_than_dropped() {
        assert_id_stays_one_segment_under_hosts("日本語-🎉-café");
    }

    /// A client that talks to `origin` rather than to the real Site Manager
    /// API.
    ///
    /// See [`SiteManagerClient::talking_to`] for why a test builds its own.
    fn client_talking_to(origin: &str) -> SiteManagerClient {
        SiteManagerClient::talking_to(origin, "an-api-key")
    }

    /// The 200 example of `GET /v1/hosts` in the official Site Manager spec.
    const SPEC_HOSTS_PAGE: &str = include_str!("openapi-examples/hosts-page.json");

    /// The `console` example of `GET /v1/hosts/{id}` in the official spec.
    const SPEC_HOST_CONSOLE: &str = include_str!("openapi-examples/host-console.json");

    /// The `network-server` example of `GET /v1/hosts/{id}` in the spec.
    const SPEC_HOST_NETWORK_SERVER: &str =
        include_str!("openapi-examples/host-network-server.json");

    /// The id of the host in the spec example of the listing.
    const SPEC_CONSOLE_ID: &str =
        "70A7419783ED0000000006797F060000000006C719490000000062ABD4EA:1261206302";

    /// The spec listing example, with its `nextToken` set to `token`, or
    /// removed when `token` is `None`.
    fn spec_page(token: Option<&str>) -> serde_json::Value {
        let mut page: serde_json::Value =
            serde_json::from_str(SPEC_HOSTS_PAGE).expect("the spec example is JSON");
        let object = page.as_object_mut().expect("the example is an object");
        match token {
            Some(token) => {
                object.insert("nextToken".to_string(), token.into());
            }
            None => {
                object.remove("nextToken");
            }
        }
        page
    }

    /// The host ids that `hosts` hold, in order.
    fn ids(hosts: &[Host]) -> Vec<&str> {
        hosts.iter().map(|host| host.id.as_str()).collect()
    }

    /// The listing example of the spec is one page, and it reads as one. A
    /// page with an empty `nextToken` is the last page too, because the spec
    /// sends an empty string for a value that it does not know.
    #[tokio::test]
    async fn the_last_page_of_the_spec_example_is_read_as_the_listing() {
        for token in [None, Some("")] {
            let body = spec_page(token).to_string();
            let server = TestServer::replying(&json_response(&body)).await;
            let client = client_talking_to(server.origin());

            let hosts = client
                .get_hosts()
                .await
                .expect("the answer of the spec must be readable");

            assert_eq!(
                server.requests().len(),
                1,
                "a last page needs one request, token {token:?}"
            );
            assert_eq!(ids(&hosts), [SPEC_CONSOLE_ID]);
            let shown = serde_json::to_value(&hosts[0]).expect("a host is JSON");
            assert_eq!(
                shown["registrationTime"], "",
                "the time stays as the API sent it"
            );
        }
    }

    /// The walk sends each `nextToken` back as one percent-encoded value, and
    /// reads the pages in order.
    #[tokio::test]
    async fn the_walk_follows_the_next_token_of_each_page() {
        let first = spec_page(Some("page 2&x=y:z")).to_string();
        let mut last = spec_page(None);
        last["data"][0]["id"] = "second-host".into();
        let routes: Vec<(String, String)> = vec![(
            "nextToken=page+2%26x%3Dy%3Az".to_string(),
            json_response(&last.to_string()),
        )];
        let server = TestServer::routing(&routes, &json_response(&first)).await;
        let client = client_talking_to(server.origin());

        let hosts = client
            .get_hosts()
            .await
            .expect("two pages of the spec shape must be readable");

        assert_eq!(server.requests().len(), 2, "two pages need two requests");
        assert_eq!(ids(&hosts), [SPEC_CONSOLE_ID, "second-host"]);
    }

    /// A server that gives a token the walk already sent would send the walk
    /// round forever. The walk refuses the answer instead. The pages give
    /// the tokens A, B, and A again, so a walk that keeps only the last token
    /// does not stop.
    #[tokio::test]
    async fn a_page_token_that_comes_back_is_refused() {
        let first = json_response(&spec_page(Some("token-a")).to_string());
        let second = json_response(&spec_page(Some("token-b")).to_string());
        let routes: Vec<(String, String)> = vec![
            ("nextToken=token-a".to_string(), second),
            ("nextToken=token-b".to_string(), first.clone()),
        ];
        let server = TestServer::routing(&routes, &first).await;
        let client = client_talking_to(server.origin());

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(60), client.get_hosts())
            .await
            .expect("the walk must stop on its own");

        let error = outcome.expect_err("a repeated token is not a listing");
        let report = format!("{error:#}");
        assert!(report.contains("a second time"), "got: {report}");
        assert_eq!(
            server.requests().len(),
            3,
            "the walk must stop when A comes back after B"
        );
    }

    /// `GET /v1/hosts/{id}` gives the host inside `data`, for each example
    /// of the spec.
    #[tokio::test]
    async fn a_host_in_the_data_key_of_the_spec_is_read() {
        for (body, id) in [
            (SPEC_HOST_CONSOLE, SPEC_CONSOLE_ID),
            (
                SPEC_HOST_NETWORK_SERVER,
                "1d9cf3ee-0c0f-466e-933c-9af829f09b50",
            ),
        ] {
            let server = TestServer::replying(&json_response(body)).await;
            let client = client_talking_to(server.origin());

            let host = client
                .get_host(id)
                .await
                .expect("the answer of the spec must be readable");

            assert_eq!(host.id, id);
        }
    }

    /// `.` and `..` name no host at all, and a URL builder that appends path
    /// segments silently drops them -- which would turn `ufa cloud host ..`
    /// into a request for the whole host listing.
    #[test]
    fn a_dot_host_id_is_rejected_rather_than_silently_dropped() {
        for id in ["", ".", ".."] {
            let error = host_url(&base_url(), id).err().unwrap_or_else(|| {
                panic!("a host id of {id:?} names no host and must be rejected")
            });

            assert!(
                error.to_string().contains(id) || id.is_empty(),
                "the rejection should quote the offending id, got: {error}"
            );
        }
    }
}
