use crate::http::{read_json_response, Api};
use anyhow::{Context, Result};
use reqwest::{header, Client, Url};
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
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::HeaderName::from_static("x-api-key"),
            header::HeaderValue::from_str(api_key).context("Invalid API key")?,
        );
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );

        let client_builder = Client::builder()
            .default_headers(headers)
            .danger_accept_invalid_certs(insecure);

        let client = client_builder
            .build()
            .context("Failed to create HTTP client")?;

        let mut base_url = Url::parse(base_url).context("Invalid UniFi controller URL")?;

        // Set the path to exactly what we need, ensuring it ends with a slash
        if base_url.path() == "/" || base_url.path().is_empty() {
            base_url.set_path(INTEGRATION_API_PATH);
        } else {
            // If there's already a path, append to it
            let current_path = base_url.path().trim_end_matches('/');
            base_url.set_path(&format!("{current_path}{INTEGRATION_API_PATH}"));
        }

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
