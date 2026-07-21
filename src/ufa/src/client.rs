use crate::http::{read_json_response, Api};
use anyhow::{Context, Result};
use reqwest::{header, Client, Url};
use serde::de::DeserializeOwned;

pub struct UnifiClient {
    client: Client,
    base_url: Url,
}

impl UnifiClient {
    pub async fn new(base_url: &str, api_key: &str, insecure: bool) -> Result<Self> {
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
            base_url.set_path("/proxy/network/integration/v1/");
        } else {
            // If there's already a path, append to it
            let current_path = base_url.path().trim_end_matches('/');
            base_url.set_path(&format!("{}/proxy/network/integration/v1/", current_path));
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

/// Decide whether `error` is the TLS layer refusing the connection.
///
/// # Arguments
///
/// * `error` - The failure to classify, usually a `reqwest::Error`.
///
/// # Returns
///
/// `true` if the connection failed in the TLS layer.
fn is_tls_failure(error: &(dyn std::error::Error + 'static)) -> bool {
    let text = error.to_string();
    text.contains("UnknownIssuer")
        || text.contains("certificate")
        || text.contains("CertificateRequired")
        || text.contains("self-signed")
        || text.contains("self signed")
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
