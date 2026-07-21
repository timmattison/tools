//! Turning an HTTP response from a UniFi API into a typed value or a useful
//! error.
//!
//! Both APIs this CLI talks to -- the controller's local integration API and
//! the hosted Site Manager API -- answer failures the same way: a non-success
//! status, sometimes carrying a JSON body with a human-readable `message`.
//! The only thing that differs is how the API is named to the user, so that
//! is the only thing callers get to choose.

use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Deserialize;

/// The error body a UniFi API answers a failed request with.
///
/// Both APIs use the same shape; any further fields they carry (an error
/// code, a request id) are not surfaced to the user, so they are ignored.
#[derive(Deserialize)]
struct ApiErrorBody {
    message: String,
}

/// Which UniFi API a response came from.
///
/// This decides how the API is named in the error raised for it, and what to
/// suggest when it rejects the caller's credential -- the two APIs use
/// separate keys, issued in different places.
#[derive(Debug, Clone, Copy)]
pub enum Api {
    /// The controller's local integration API.
    Controller,
    /// The hosted Site Manager (`api.ui.com`) API.
    SiteManager,
}

impl Api {
    /// How this API is named in the errors raised for it.
    fn label(self) -> &'static str {
        match self {
            Api::Controller => "API",
            Api::SiteManager => "Site Manager API",
        }
    }

    /// What to tell the user when this API rejects their credential.
    ///
    /// # Arguments
    ///
    /// * `status` - The rejecting status, which distinguishes a key that is
    ///   not accepted at all from one that lacks the necessary permission.
    ///
    /// # Returns
    ///
    /// The full message to raise.
    fn authentication_failure(self, status: StatusCode) -> String {
        match self {
            Api::Controller => format!(
                "Authentication failed (HTTP {status}). Please check your API key or generate a new one in Settings -> Control Plane -> Integrations"
            ),
            Api::SiteManager => format!(
                "Site Manager authentication failed (HTTP {status}). Please check your Site Manager API key."
            ),
        }
    }
}

/// Read `response` as `T`, or raise the most specific error the response
/// supports.
///
/// A rejected credential is reported as such, with advice for the API that
/// rejected it; a failure the API described in its body is reported in the
/// API's own words; anything else falls back to the status and the body.
///
/// # Arguments
///
/// * `api` - Which API answered, which decides how errors name it.
/// * `response` - The response to read.
///
/// # Returns
///
/// The deserialized body.
///
/// # Errors
///
/// Returns an error if the body cannot be read, if the status is not a
/// success, or if the body is not the JSON `T` expects.
pub async fn read_json_response<T>(api: Api, response: reqwest::Response) -> Result<T>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let body = response
        .text()
        .await
        .context("Failed to read response body")?;

    if !status.is_success() {
        return Err(response_error(api, status, &body));
    }

    serde_json::from_str(&body)
        .with_context(|| format!("Failed to parse {} response JSON: {body}", api.label()))
}

/// Build the error for a response whose status was not a success.
///
/// # Arguments
///
/// * `api` - Which API answered.
/// * `status` - The status it answered with.
/// * `body` - The response body, which may or may not be the API's JSON error
///   shape.
///
/// # Returns
///
/// The error to raise.
fn response_error(api: Api, status: StatusCode, body: &str) -> anyhow::Error {
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return anyhow::anyhow!(api.authentication_failure(status));
    }

    if let Ok(error) = serde_json::from_str::<ApiErrorBody>(body) {
        return anyhow::anyhow!("{} error: {} (HTTP {status})", api.label(), error.message);
    }

    anyhow::anyhow!("{} HTTP error {status}: {body}", api.label())
}
