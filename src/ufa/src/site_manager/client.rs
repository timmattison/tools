use crate::http::{build_client, read_json_response, Api, Timeouts};
use crate::site_manager::models::{Host, HostsResponse};
use anyhow::{Context, Result};
use reqwest::{Client, Url};
use serde::de::DeserializeOwned;

/// The Site Manager API path segment that cloud hosts live under.
const HOSTS_SEGMENT: &str = "hosts";

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

/// Take the hosts out of `response`, but only when they are all of them.
///
/// # Arguments
///
/// * `response` - The listing the Site Manager API answered with.
///
/// # Returns
///
/// Every host the response carried.
///
/// # Errors
///
/// Returns an error if the response carries fewer hosts than the `total` it
/// states for the collection.
fn whole_listing(response: HostsResponse) -> Result<Vec<Host>> {
    let HostsResponse { hosts, total } = response;

    if let Some(total) = total {
        let arrived = u64::try_from(hosts.len()).unwrap_or(u64::MAX);

        anyhow::ensure!(
            arrived >= u64::from(total),
            "The Site Manager API answered with {arrived} of the {total} cloud hosts it states \
             exist. The listing takes one request, which reaches one page, so this answer is \
             incomplete."
        );
    }

    Ok(hosts)
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

    /// Fetch every cloud host on the account.
    ///
    /// # One request, checked against the total the server states
    ///
    /// The listing endpoint answers one page, and this client sends no paging
    /// parameters with the request. `total` is what the server states for the
    /// whole collection, so a listing shorter than `total` is a page and not
    /// the answer. This refuses such a listing instead of a report of a
    /// fraction of the account as the whole of it.
    ///
    /// A walk over the pages is the better answer, and it needs the name of
    /// the field the endpoint returns for the next page. No document in this
    /// repository records that name: `integration.json` is the controller
    /// API, and it holds no host listing at all. A walk against a guessed
    /// field name reads the first page again and reports it as complete,
    /// which is the same defect with more code behind it. So the check stands
    /// here until the real field name is known.
    ///
    /// A server that states no `total` says nothing about completeness. There
    /// is nothing to compare such an answer against, so the listing is taken
    /// as it arrived. A refusal there stops a command that works today over a
    /// field the server never sent.
    ///
    /// # Returns
    ///
    /// Every cloud host on the account.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, if the answer cannot be read,
    /// or if the listing is shorter than the total the server states.
    pub async fn get_hosts(&self) -> Result<Vec<Host>> {
        let response: HostsResponse = self.get(hosts_url(&self.base_url)?).await?;

        whole_listing(response)
    }

    pub async fn get_host(&self, id: &str) -> Result<Host> {
        let url = host_url(&self.base_url, id)?;
        self.get(url).await
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

    /// One cloud host, with every field the model demands of it.
    const A_HOST: &str =
        r#"{"id":"h1","hardwareId":"hw1","type":"console","isBlocked":false,"owner":true}"#;

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
    /// The base URL is the one thing a test has to replace, and
    /// [`SiteManagerClient::new`] names `api.ui.com` and nothing else, so the
    /// client is built field by field here. Everything else about it -- the
    /// key header, the refused redirect, the bounds on the request -- comes
    /// from the same `build_client` a command uses.
    fn client_talking_to(origin: &str) -> SiteManagerClient {
        let client = build_client(Api::SiteManager, "an-api-key", false, Timeouts::PRODUCTION)
            .expect("a plain key must build a Site Manager client");
        let base_url =
            Url::parse(&format!("{origin}/v1/")).expect("a loopback origin is a base URL");

        SiteManagerClient { client, base_url }
    }

    /// The listing body that carries `hosts` hosts, and the `total` the server
    /// states for the whole collection.
    ///
    /// A `total` of `None` leaves the field out altogether, which is what a
    /// server that states no total sends.
    fn hosts_body(hosts: usize, total: Option<u32>) -> String {
        let listed = vec![A_HOST; hosts].join(",");

        match total {
            Some(total) => format!(r#"{{"hosts":[{listed}],"total":{total}}}"#),
            None => format!(r#"{{"hosts":[{listed}]}}"#),
        }
    }

    /// The listing endpoint answers one page. An account with more hosts than
    /// fit in it therefore hands back a fraction of the collection, and the
    /// server says so in `total`. Reporting that fraction as the whole
    /// listing is the failure `crate::pagination` exists to prevent: `ufa
    /// cloud hosts` prints a short table, and the "Total hosts" line under it
    /// agrees with the short table.
    #[tokio::test]
    async fn a_host_listing_shorter_than_the_stated_total_is_refused() {
        let server = TestServer::replying(&json_response(&hosts_body(2, Some(41)))).await;
        let client = client_talking_to(server.origin());

        let error = client
            .get_hosts()
            .await
            .expect_err("2 of 41 hosts is not the listing, and must not be reported as one");

        let report = format!("{error:#}");
        assert!(
            report.contains("41"),
            "the refusal must say how many hosts the server states exist, got: {report}"
        );
        assert!(
            report.contains('2'),
            "the refusal must say how many hosts arrived, got: {report}"
        );
    }

    /// The ordinary answer must stay ordinary: as many hosts as the server
    /// states exist is the whole listing.
    #[tokio::test]
    async fn a_host_listing_that_matches_the_stated_total_is_returned() {
        let server = TestServer::replying(&json_response(&hosts_body(3, Some(3)))).await;
        let client = client_talking_to(server.origin());

        let hosts = client
            .get_hosts()
            .await
            .expect("a listing that matches the stated total is complete");

        assert_eq!(hosts.len(), 3, "every host the server sent must come back");
    }

    /// A server that states no total says nothing about completeness, so
    /// there is nothing to compare the answer against. The listing is taken
    /// as it arrived, because a refusal here breaks a command that works.
    #[tokio::test]
    async fn a_host_listing_the_server_states_no_total_for_is_returned() {
        let server = TestServer::replying(&json_response(&hosts_body(2, None))).await;
        let client = client_talking_to(server.origin());

        let hosts = client
            .get_hosts()
            .await
            .expect("a listing with no stated total is not evidence of a short answer");

        assert_eq!(hosts.len(), 2, "every host the server sent must come back");
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
