use anyhow::{Context, Result};
use reqwest::{Client, header, Url};
use serde::de::DeserializeOwned;
use crate::site_manager::models::{Host, HostsResponse, ErrorResponse};

pub struct SiteManagerClient {
    client: Client,
    base_url: Url,
}

impl SiteManagerClient {
    pub async fn new(api_key: &str) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::HeaderName::from_static("x-api-key"),
            header::HeaderValue::from_str(api_key)
                .context("Invalid Site Manager API key")?
        );
        headers.insert(header::ACCEPT, header::HeaderValue::from_static("application/json"));

        let client = Client::builder()
            .default_headers(headers)
            .build()
            .context("Failed to create HTTP client")?;

        let base_url = Url::parse("https://api.ui.com/v1/")
            .context("Failed to parse Site Manager API URL")?;
        
        Ok(Self {
            client,
            base_url,
        })
    }

    pub async fn get_hosts(&self) -> Result<Vec<Host>> {
        let response: HostsResponse = self.get("hosts").await?;
        Ok(response.hosts)
    }

    pub async fn get_host(&self, id: &str) -> Result<Host> {
        let path = format!("hosts/{}", id);
        self.get(&path).await
    }

    async fn get<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let url = self.base_url.join(path)
            .context("Failed to construct request URL")?;

        let response = self.client
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
        let text = response.text().await
            .context("Failed to read response body")?;

        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
                anyhow::bail!(
                    "Site Manager authentication failed (HTTP {}). Please check your Site Manager API key.",
                    status
                );
            }
            
            if let Ok(error_response) = serde_json::from_str::<ErrorResponse>(&text) {
                anyhow::bail!("Site Manager API error: {} (HTTP {})", error_response.message, status);
            }
            anyhow::bail!("Site Manager API HTTP error {}: {}", status, text);
        }

        serde_json::from_str(&text)
            .with_context(|| format!("Failed to parse Site Manager API response JSON: {}", text))
    }
}