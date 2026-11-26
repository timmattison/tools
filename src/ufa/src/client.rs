use crate::http::{
    build_client, read_json_response, read_response_without_document, Api, Timeouts,
};
use anyhow::{Context, Result};
use reqwest::{Client, Url};
use serde::de::DeserializeOwned;

/// Where the controller serves the integration API, relative to the URL of
/// the controller.
///
/// Only [`integration_api_base`] reads it. The client and the discovery probe
/// both get the integration API from that function, so the two cannot
/// disagree about where it is.
const INTEGRATION_API_PATH: &str = "/proxy/network/integration/v1/";

/// The base URL of the integration API of the controller at `controller`.
///
/// A controller behind a reverse proxy answers at a path prefix, and the
/// integration API hangs off that prefix rather than off the origin. An
/// origin with no path of its own leaves the integration path standing on
/// its own.
///
/// A URL that already names the integration API is used where it stands. The
/// path is what the controller serves, so a user who pasted the whole thing
/// has given the right answer, and appending to it would ask for
/// `.../integration/v1/proxy/network/integration/v1/`.
///
/// The result always ends in a slash, whichever shape it was built from.
/// Every request is a relative join onto this URL, and `Url::join` replaces
/// the last segment of a path that does not end in one.
///
/// # Arguments
///
/// * `controller` - The URL of the controller, with or without a path
///   prefix.
///
/// # Returns
///
/// The base URL of the integration API.
pub fn integration_api_base(controller: &Url) -> Url {
    let mut base_url = controller.clone();
    let prefix = controller.path().trim_end_matches('/');
    let suffix = if prefix.ends_with(INTEGRATION_API_PATH.trim_end_matches('/')) {
        "/"
    } else {
        INTEGRATION_API_PATH
    };
    base_url.set_path(&format!("{prefix}{suffix}"));
    base_url
}

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

        let base_url =
            integration_api_base(&Url::parse(base_url).context("Invalid UniFi controller URL")?);

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
        let response = self.send_post(path, body).await?;

        read_json_response(Api::Controller, response).await
    }

    /// Send `body` to `path` as a POST, and read no document from the answer.
    ///
    /// The integration API documents the success answer of the actions of a
    /// device and of a port with no content, so the body of the answer is not
    /// read as JSON.
    ///
    /// # Arguments
    ///
    /// * `path` - The path of the request, relative to the integration API.
    /// * `body` - The document to send as JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, or if the status of the answer
    /// is not a success.
    pub async fn post_action<B>(&self, path: &str, body: &B) -> Result<()>
    where
        B: serde::Serialize,
    {
        let response = self.send_post(path, body).await?;

        read_response_without_document(Api::Controller, response).await
    }

    /// Send `body` to `path` as a POST.
    ///
    /// # Arguments
    ///
    /// * `path` - The path of the request, relative to the integration API.
    /// * `body` - The document to send as JSON.
    ///
    /// # Returns
    ///
    /// The answer, with its body not read yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL cannot be built, or if the request fails.
    async fn send_post<B>(&self, path: &str, body: &B) -> Result<reqwest::Response>
    where
        B: serde::Serialize,
    {
        let url = self
            .base_url
            .join(path)
            .context("Failed to construct request URL")?;

        self.client
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|e| self.handle_request_error(e, "POST"))
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

    fn handle_request_error(&self, error: reqwest::Error, method: &str) -> anyhow::Error {
        if is_tls_failure(&error) {
            anyhow::anyhow!(
                "TLS certificate error: {}\n\nTo connect to a UniFi controller with a self-signed certificate:\n  - Use the --insecure flag\n  - Or set UNIFI_INSECURE=true in the .env file beside the config file (`ufa config path` prints the config file)\n\nNote: This disables certificate verification and should only be used for trusted networks.",
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

    /// What a plain HTTP server writes when a client opens a TLS handshake on
    /// it: an HTTP answer, which is not a TLS record.
    const PLAIN_HTTP_ANSWER: &[u8] =
        b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

    /// Start a host that answers the first bytes of each connection with
    /// [`PLAIN_HTTP_ANSWER`], and then closes the connection.
    ///
    /// [`TestServer`] reads an HTTP request head before it answers, and a
    /// `ClientHello` holds no such head, thus this host reads one chunk only.
    ///
    /// # Returns
    ///
    /// The port of the host, and the task that serves it.
    async fn plain_http_host() -> (u16, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a test host must be able to bind a loopback port");
        let port = listener
            .local_addr()
            .expect("a bound listener has a local address")
            .port();

        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut hello = [0_u8; 1_024];
                let _ = socket.read(&mut hello).await;
                let _ = socket.write_all(PLAIN_HTTP_ANSWER).await;
                let _ = socket.flush().await;
            }
        });

        (port, task)
    }

    /// A real handshake that the host refuses is a TLS failure, in each
    /// build of `ufa`.
    ///
    /// The tests above build the refusal by hand, in the shape that rustls
    /// gives. Only a real handshake shows the shape that the TLS backend of
    /// the client gives. A build of the whole workspace turns on the default
    /// features of reqwest, and reqwest then picks native-tls unless the
    /// builder names a backend. native-tls reports the refusal in a different
    /// shape, and the advice about `--insecure` does not show.
    #[tokio::test]
    async fn a_refused_handshake_of_a_real_client_is_a_tls_failure() {
        let (port, host) = plain_http_host().await;
        let client = build_client(Api::Controller, "an-api-key", false, Timeouts::PRODUCTION)
            .expect("a test key must build a client");

        let error = client
            .get(format!("https://127.0.0.1:{port}/"))
            .send()
            .await
            .expect_err("a host that answers in plain HTTP cannot complete a TLS handshake");
        host.abort();

        assert!(
            is_tls_failure(&error),
            "a refused handshake must be recognised as a TLS failure, got: {error:?}"
        );
    }

    /// `ufa` reads only the `.env` file beside the config file. Thus the TLS
    /// advice must name that place. A user who obeys an advice with no place
    /// puts the setting into a `.env` that `ufa` never reads.
    #[tokio::test]
    async fn the_tls_advice_names_the_env_file_that_ufa_reads() {
        let (port, host) = plain_http_host().await;
        let client = UnifiClient::new(&format!("https://127.0.0.1:{port}"), "an-api-key", false)
            .expect("a test key must build a client");

        let error = client
            .get::<serde_json::Value>("info")
            .await
            .expect_err("a host that answers in plain HTTP cannot complete a TLS handshake");
        host.abort();
        let advice = format!("{error:#}");

        assert!(
            advice.contains(".env file beside the config file"),
            "a user who obeys the advice puts the setting into a .env that ufa never reads, got: {advice}"
        );
        assert!(
            advice.contains("ufa config path"),
            "a user cannot find the config file without the command that prints it, got: {advice}"
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
