use anyhow::{Context, Result};
use reqwest::{Client, header, Url};
use serde::de::DeserializeOwned;
use serde_json::Value;

pub struct UnifiClient {
    client: Client,
    base_url: Url,
}

impl UnifiClient {
    pub async fn new(base_url: &str, api_key: &str, insecure: bool) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::HeaderName::from_static("x-api-key"),
            header::HeaderValue::from_str(api_key)
                .context("Invalid API key")?
        );
        headers.insert(header::ACCEPT, header::HeaderValue::from_static("application/json"));

        let client_builder = Client::builder()
            .default_headers(headers)
            .danger_accept_invalid_certs(insecure);

        let client = client_builder.build()
            .context("Failed to create HTTP client")?;

        let mut base_url = Url::parse(base_url)
            .context("Invalid UniFi controller URL")?;
        
        // Set the path to exactly what we need, ensuring it ends with a slash
        if base_url.path() == "/" || base_url.path().is_empty() {
            base_url.set_path("/proxy/network/integration/v1/");
        } else {
            // If there's already a path, append to it
            let current_path = base_url.path().trim_end_matches('/');
            base_url.set_path(&format!("{}/proxy/network/integration/v1/", current_path));
        }

        
        Ok(Self {
            client,
            base_url,
        })
    }

    pub async fn get<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let url = self.base_url.join(path)
            .context("Failed to construct request URL")?;


        let response = self.client
            .get(url)
            .send()
            .await
            .map_err(|e| self.handle_request_error(e, "GET"))?;

        self.handle_response(response).await
    }

    pub async fn get_with_params<T>(&self, path: &str, params: &[(&str, &dyn std::fmt::Display)]) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let mut url = self.base_url.join(path)
            .context("Failed to construct request URL")?;

        {
            let mut query_pairs = url.query_pairs_mut();
            for (key, value) in params {
                query_pairs.append_pair(key, &value.to_string());
            }
        }


        let response = self.client
            .get(url)
            .send()
            .await
            .map_err(|e| self.handle_request_error(e, "GET"))?;

        self.handle_response(response).await
    }

    pub async fn post<T, B>(&self, path: &str, body: &B) -> Result<T>
    where
        T: DeserializeOwned,
        B: serde::Serialize,
    {
        let url = self.base_url.join(path)
            .context("Failed to construct request URL")?;

        let response = self.client
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|e| self.handle_request_error(e, "POST"))?;

        self.handle_response(response).await
    }

    pub async fn delete<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let url = self.base_url.join(path)
            .context("Failed to construct request URL")?;

        let response = self.client
            .delete(url)
            .send()
            .await
            .map_err(|e| self.handle_request_error(e, "DELETE"))?;

        self.handle_response(response).await
    }

    pub async fn delete_with_params<T>(&self, path: &str, params: &[(&str, &dyn std::fmt::Display)]) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let mut url = self.base_url.join(path)
            .context("Failed to construct request URL")?;

        {
            let mut query_pairs = url.query_pairs_mut();
            for (key, value) in params {
                query_pairs.append_pair(key, &value.to_string());
            }
        }

        let response = self.client
            .delete(url)
            .send()
            .await
            .map_err(|e| self.handle_request_error(e, "DELETE"))?;

        self.handle_response(response).await
    }

    async fn handle_response<T>(&self, response: reqwest::Response) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let status = response.status();
        let text = response.text().await
            .context("Failed to read response body")?;

        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
                anyhow::bail!(
                    "Authentication failed (HTTP {}). Please check your API key or generate a new one in Settings -> Control Plane -> Integrations",
                    status
                );
            }
            
            if let Ok(error_response) = serde_json::from_str::<Value>(&text) {
                if let Some(message) = error_response.get("message").and_then(|m| m.as_str()) {
                    anyhow::bail!("API error: {} (HTTP {})", message, status);
                }
            }
            anyhow::bail!("HTTP error {}: {}", status, text);
        }

        serde_json::from_str(&text)
            .with_context(|| format!("Failed to parse response JSON: {}", text))
    }

    fn handle_request_error(&self, error: reqwest::Error, method: &str) -> anyhow::Error {
        let error_str = error.to_string();
        if error_str.contains("UnknownIssuer") || 
           error_str.contains("certificate") ||
           error_str.contains("CertificateRequired") ||
           error_str.contains("self-signed") ||
           error_str.contains("self signed") {
            anyhow::anyhow!(
                "TLS certificate error: {}\n\nTo connect to a UniFi controller with a self-signed certificate:\n  - Use the --insecure flag\n  - Or set UNIFI_INSECURE=true in your .env file\n\nNote: This disables certificate verification and should only be used for trusted networks.",
                error
            )
        } else {
            anyhow::anyhow!("Failed to send {} request: {}", method, error)
        }
    }
}