use crate::http::{build_client, read_json_response, Api, Timeouts};
use anyhow::{Context, Result};
use reqwest::{Client, Url};
use serde::de::DeserializeOwned;

/// Where the controller serves the integration API, relative to its origin.
///
/// Discovery probes this path directly to tell a controller from anything
/// else that happens to answer on the same port, so it lives next to the
/// client that talks to it rather than being spelled out twice.
pub const INTEGRATION_API_PATH: &str = "/proxy/network/integration/v1/";

pub struct UnifiClient {
    client: Client,
    base_url: Url,
}

impl UnifiClient {
    pub fn new(base_url: &str, api_key: &str, insecure: bool) -> Result<Self> {
        // Check if this is a cloud console URL
        if crate::site_manager::is_cloud_console_url(base_url) {
            anyhow::bail!(
                "Cloud console URLs are not yet supported for direct API access.\n\
                To work with cloud-hosted consoles, use the 'ufa cloud' commands to discover console IDs.\n\
                For direct API access, use the local IP address of your UniFi controller."
            );
        }
        let client = build_client(Api::Controller, api_key, insecure, Timeouts::PRODUCTION)?;

        let mut base_url = Url::parse(base_url).context("Invalid UniFi controller URL")?;

        // A controller behind a reverse proxy answers at a path prefix, and
        // the integration API hangs off that prefix rather than off the
        // origin. An origin with no path of its own leaves the integration
        // path standing on its own.
        //
        // A URL that already names the integration API is used where it
        // stands. The path is what the controller serves, so a user who
        // pasted the whole thing has given the right answer, and appending to
        // it would ask for `.../integration/v1/proxy/network/integration/v1/`.
        //
        // The result always ends in a slash, whichever shape it was built
        // from. Every request is a relative join onto this URL, and
        // `Url::join` replaces the last segment of a path that does not end
        // in one.
        let prefix = base_url.path().trim_end_matches('/');
        let suffix = if prefix.ends_with(INTEGRATION_API_PATH.trim_end_matches('/')) {
            "/"
        } else {
            INTEGRATION_API_PATH
        };
        base_url.set_path(&format!("{prefix}{suffix}"));

        Ok(Self { client, base_url })
    }

    pub async fn get<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let url = self
            .base_url
            .join(path)
            .context("Failed to construct request URL")?;

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| self.handle_request_error(e, "GET"))?;

        read_json_response(Api::Controller, response).await
    }

    pub async fn get_with_params<T>(
        &self,
        path: &str,
        params: &[(&str, &dyn std::fmt::Display)],
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let mut url = self
            .base_url
            .join(path)
            .context("Failed to construct request URL")?;

        {
            let mut query_pairs = url.query_pairs_mut();
            for (key, value) in params {
                query_pairs.append_pair(key, &value.to_string());
            }
        }

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| self.handle_request_error(e, "GET"))?;

        read_json_response(Api::Controller, response).await
    }

    pub async fn post<T, B>(&self, path: &str, body: &B) -> Result<T>
    where
        T: DeserializeOwned,
        B: serde::Serialize,
    {
        let url = self
            .base_url
            .join(path)
            .context("Failed to construct request URL")?;

        let response = self
            .client
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|e| self.handle_request_error(e, "POST"))?;

        read_json_response(Api::Controller, response).await
    }

    pub async fn delete<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let url = self
            .base_url
            .join(path)
            .context("Failed to construct request URL")?;

        let response = self
            .client
            .delete(url)
            .send()
            .await
            .map_err(|e| self.handle_request_error(e, "DELETE"))?;

        read_json_response(Api::Controller, response).await
    }

    pub async fn delete_with_params<T>(
        &self,
        path: &str,
        params: &[(&str, &dyn std::fmt::Display)],
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let mut url = self
            .base_url
            .join(path)
            .context("Failed to construct request URL")?;

        {
            let mut query_pairs = url.query_pairs_mut();
            for (key, value) in params {
                query_pairs.append_pair(key, &value.to_string());
            }
        }

        let response = self
            .client
            .delete(url)
            .send()
            .await
            .map_err(|e| self.handle_request_error(e, "DELETE"))?;

        read_json_response(Api::Controller, response).await
    }

    fn handle_request_error(&self, error: reqwest::Error, method: &str) -> anyhow::Error {
        if is_tls_failure(&error) {
            anyhow::anyhow!(
                "TLS certificate error: {}\n\nTo connect to a UniFi controller with a self-signed certificate:\n  - Use the --insecure flag\n  - Or set UNIFI_INSECURE=true in your .env file\n\nNote: This disables certificate verification and should only be used for trusted networks.",
                error
            )
        } else {
            anyhow::anyhow!("Failed to send {} request: {}", method, error)
        }
    }
}

/// How far down a chain of causes to look for the TLS layer.
///
/// A real rejection sits three links down; the bound only exists so a chain
/// that somehow refers back to itself cannot spin forever.
const MAX_ERROR_CHAIN_DEPTH: usize = 16;

/// Decide whether `error` is the TLS layer refusing the connection.
///
/// The TLS stack reports a peer it will not talk to as an I/O error of kind
/// [`std::io::ErrorKind::InvalidData`], wrapped in the connector's own I/O
/// error. That kind is the signal this looks for, walking both the chain of
/// causes and the errors nested inside I/O errors -- which the chain alone
/// does not reach, because [`std::io::Error`]'s `source` returns the source
/// *of* the error it carries rather than that error itself.
///
/// Matching on the rendered message instead would break on any rewording by
/// reqwest or rustls, and on any locale that is not English. Matching on
/// `rustls::Error` itself would be narrower still -- it would separate a
/// rejected certificate, which `--insecure` can get past, from a protocol
/// failure, which it cannot -- but rustls reaches this crate only through
/// reqwest's private dependency on it. Naming the type here would mean
/// pinning a second copy of rustls to whatever version reqwest resolves to,
/// and a reqwest upgrade that moved to another major version of rustls would
/// silently stop matching, with nothing failing to compile to say so.
///
/// # Arguments
///
/// * `error` - The failure to classify, usually a `reqwest::Error`.
///
/// # Returns
///
/// `true` if the connection failed in the TLS layer.
fn is_tls_failure(error: &(dyn std::error::Error + 'static)) -> bool {
    fn search(error: &(dyn std::error::Error + 'static), depth: usize) -> bool {
        if depth == 0 {
            return false;
        }

        if let Some(io_error) = error.downcast_ref::<std::io::Error>() {
            if io_error.kind() == std::io::ErrorKind::InvalidData {
                return true;
            }
            if let Some(nested) = io_error.get_ref() {
                if search(nested, depth - 1) {
                    return true;
                }
            }
        }

        error
            .source()
            .is_some_and(|source| search(source, depth - 1))
    }

    search(error, MAX_ERROR_CHAIN_DEPTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{empty_json, redirect_to, TestServer};
    use std::io;

    /// The shape reqwest hands back when rustls rejects a peer: the rustls
    /// failure inside an `InvalidData` I/O error, inside the connector's own
    /// I/O error. Captured from a real request to a self-signed host.
    fn tls_rejection(message: &'static str) -> io::Error {
        io::Error::other(io::Error::new(io::ErrorKind::InvalidData, message))
    }

    /// A TLS rejection must be recognised by what it *is*, not by the English
    /// wording rustls happened to use for it: the message is rustls's to
    /// change, and it is not the user's language.
    #[test]
    fn a_tls_rejection_is_recognised_whatever_it_says() {
        let error = tls_rejection("certificat du pair invalide : émetteur inconnu");

        assert!(
            is_tls_failure(&error),
            "a TLS rejection must be recognised regardless of its wording"
        );
    }

    /// The wording rustls uses today must keep working too.
    #[test]
    fn todays_rustls_certificate_rejection_is_recognised() {
        let error = tls_rejection("invalid peer certificate: UnknownIssuer");

        assert!(
            is_tls_failure(&error),
            "the certificate rejection rustls raises today must be recognised"
        );
    }

    /// Advising `--insecure` for a failure that has nothing to do with TLS
    /// sends the user to disable certificate verification over a problem it
    /// cannot fix -- and the word "certificate" can turn up in a hostname, a
    /// proxy's message or a path.
    #[test]
    fn a_non_tls_failure_that_merely_says_certificate_is_not_a_tls_failure() {
        let error = io::Error::other("no route to host: certificates.example.com");

        assert!(
            !is_tls_failure(&error),
            "only a failure from the TLS layer may be reported as one"
        );
    }

    /// Building a client reads no configuration and opens no connection, so
    /// it belongs in ordinary code rather than behind an await -- and this
    /// test, which is not async, is only able to call it because it is.
    #[test]
    fn a_client_is_built_without_an_async_context() {
        let client = UnifiClient::new("https://192.168.1.1", "an-api-key", true)
            .expect("a local controller URL must build a client");

        assert_eq!(
            client.base_url.as_str(),
            "https://192.168.1.1/proxy/network/integration/v1/"
        );
    }

    /// A controller behind a reverse proxy answers at a path prefix, and the
    /// integration API hangs off that prefix rather than off the origin.
    ///
    /// The base URL has to end in a slash whatever it is built from, because
    /// every request is a relative join onto it: `Url::join` replaces the
    /// last segment of a path that does not end in one, so a missing slash
    /// sends `info` to `.../v1/info` in one case and `.../integration/info`
    /// in another.
    #[test]
    fn a_controller_behind_a_path_prefix_keeps_the_prefix() {
        let client = UnifiClient::new("https://192.168.1.1/unifi", "an-api-key", true)
            .expect("a prefixed controller URL must build a client");

        assert_eq!(
            client.base_url.as_str(),
            "https://192.168.1.1/unifi/proxy/network/integration/v1/"
        );
    }

    /// The same prefix, spelled with the trailing slash a browser adds. The
    /// two spellings name one controller and must reach one base URL.
    #[test]
    fn a_path_prefix_that_ends_in_a_slash_reaches_the_same_base_url() {
        let client = UnifiClient::new("https://192.168.1.1/unifi/", "an-api-key", true)
            .expect("a prefixed controller URL must build a client");

        assert_eq!(
            client.base_url.as_str(),
            "https://192.168.1.1/unifi/proxy/network/integration/v1/"
        );
    }

    /// A user who pastes the whole integration URL has already given the
    /// right answer. Appending to it asks the controller for
    /// `.../integration/v1/proxy/network/integration/v1/`, which nothing
    /// serves, and every command then fails on a 404 that names a path the
    /// user never typed.
    #[test]
    fn a_url_that_already_names_the_integration_api_is_used_as_it_stands() {
        let client = UnifiClient::new(
            "https://192.168.1.1/proxy/network/integration/v1/",
            "an-api-key",
            true,
        )
        .expect("the integration URL itself must build a client");

        assert_eq!(
            client.base_url.as_str(),
            "https://192.168.1.1/proxy/network/integration/v1/"
        );
    }

    /// The same URL without its trailing slash. It still names the
    /// integration API, and the base URL still has to end in a slash for the
    /// relative joins above it.
    #[test]
    fn the_integration_api_without_a_trailing_slash_gains_one_and_nothing_else() {
        let client = UnifiClient::new(
            "https://192.168.1.1/proxy/network/integration/v1",
            "an-api-key",
            true,
        )
        .expect("the integration URL itself must build a client");

        assert_eq!(
            client.base_url.as_str(),
            "https://192.168.1.1/proxy/network/integration/v1/"
        );
    }

    /// A cloud console URL is rejected up front, with advice on what to use
    /// instead, rather than by whatever the first request happens to fail on.
    #[test]
    fn a_cloud_console_url_is_rejected_when_the_client_is_built() {
        let error = UnifiClient::new("https://unifi.ui.com/consoles/ABC123", "an-api-key", false)
            .err()
            .expect("a cloud console URL has no direct API");

        assert!(
            error.to_string().contains("ufa cloud"),
            "the rejection must point at the cloud commands, got: {error}"
        );
    }

    /// A controller that answers with a redirect must not be able to steer
    /// the user's API key to a host of its choosing.
    ///
    /// reqwest follows up to ten redirects unless it is told otherwise, and it
    /// drops only the standard credential headers when the host changes.
    /// `x-api-key` is not one of them, so a controller that answers 302 with a
    /// foreign host reads the key straight off the second request. The risk is
    /// sharper under `--insecure`, where no certificate is checked at all.
    #[tokio::test]
    async fn a_redirect_does_not_carry_the_api_key_to_another_host() {
        const API_KEY: &str = "the-users-secret-key";

        let elsewhere = TestServer::replying(&empty_json()).await;
        let controller =
            TestServer::replying(&redirect_to(&format!("{}/moved", elsewhere.origin()))).await;

        let client = UnifiClient::new(controller.origin(), API_KEY, false)
            .expect("a loopback URL must build a client");
        let outcome: Result<serde_json::Value> = client.get("info").await;

        let received = elsewhere.requests();
        assert!(
            received
                .iter()
                .all(|request| request.header("x-api-key").is_none()),
            "the API key reached the host the controller named: {received:?}"
        );
        assert!(
            received.is_empty(),
            "the request followed the redirect to another host: {received:?}"
        );

        let error = outcome.expect_err("a 302 carries no JSON body for this client to read");
        assert!(
            error.to_string().contains("302"),
            "the redirect must be reported rather than followed, got: {error}"
        );
    }

    /// The ordinary connection failures must stay ordinary.
    #[test]
    fn a_refused_connection_is_not_a_tls_failure() {
        let error = io::Error::from(io::ErrorKind::ConnectionRefused);

        assert!(
            !is_tls_failure(&error),
            "a refused connection is not a TLS problem"
        );
    }
}
