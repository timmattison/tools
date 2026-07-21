use crate::site_manager::models::{ErrorResponse, Host, HostsResponse};
use anyhow::{Context, Result};
use reqwest::{header, Client, Url};
use serde::de::DeserializeOwned;

/// The Site Manager API path segment that cloud hosts live under.
const HOSTS_SEGMENT: &str = "hosts";

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
/// Returns an error if the URL cannot be constructed.
fn host_url(base_url: &Url, id: &str) -> Result<Url> {
    base_url
        .join(&format!("{HOSTS_SEGMENT}/{id}"))
        .context("Failed to construct request URL")
}

pub struct SiteManagerClient {
    client: Client,
    base_url: Url,
}

impl SiteManagerClient {
    pub async fn new(api_key: &str) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::HeaderName::from_static("x-api-key"),
            header::HeaderValue::from_str(api_key).context("Invalid Site Manager API key")?,
        );
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );

        let client = Client::builder()
            .default_headers(headers)
            .build()
            .context("Failed to create HTTP client")?;

        let base_url =
            Url::parse("https://api.ui.com/v1/").context("Failed to parse Site Manager API URL")?;

        Ok(Self { client, base_url })
    }

    pub async fn get_hosts(&self) -> Result<Vec<Host>> {
        let url = self
            .base_url
            .join(HOSTS_SEGMENT)
            .context("Failed to construct request URL")?;
        let response: HostsResponse = self.get(url).await?;
        Ok(response.hosts)
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

        self.handle_response(response).await
    }

    async fn handle_response<T>(&self, response: reqwest::Response) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let status = response.status();
        let text = response
            .text()
            .await
            .context("Failed to read response body")?;

        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                anyhow::bail!(
                    "Site Manager authentication failed (HTTP {}). Please check your Site Manager API key.",
                    status
                );
            }

            if let Ok(error_response) = serde_json::from_str::<ErrorResponse>(&text) {
                anyhow::bail!(
                    "Site Manager API error: {} (HTTP {})",
                    error_response.message,
                    status
                );
            }
            anyhow::bail!("Site Manager API HTTP error {}: {}", status, text);
        }

        serde_json::from_str(&text)
            .with_context(|| format!("Failed to parse Site Manager API response JSON: {}", text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
