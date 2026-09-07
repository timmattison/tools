//! Building the HTTP client that talks to a UniFi API, and turning what comes
//! back into a typed value or a useful error.
//!
//! Both APIs this CLI talks to -- the controller's local integration API and
//! the hosted Site Manager API -- answer failures the same way: a non-success
//! status, sometimes carrying a JSON body with a human-readable `message`.
//! The only thing that differs is how the API is named to the user, so that
//! is the only thing callers get to choose.

use crate::text::truncate_for_display;
use anyhow::{Context, Result};
use reqwest::{header, redirect, Client, StatusCode};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::time::Duration;

/// How long one request to a UniFi API has to finish.
///
/// A controller on the same network answers in milliseconds. This is generous
/// enough for a listing of thousands of clients over a slow link, and short
/// enough that a user who waits learns that something is wrong.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the connection alone has to be established.
///
/// A host that refuses the connection answers at once. This bound is for the
/// host that drops the packets instead and never answers at all.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The bounds a client puts on one request.
///
/// [`Timeouts::PRODUCTION`] is what every client that serves a command
/// carries. A parameter rather than two constants inside [`build_client`],
/// because a test of the bound must build the same client with a short one:
/// no test may wait for a production timeout.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// The whole request, from the first byte sent to the last byte read.
    pub request: Duration,
    /// The connection alone.
    pub connect: Duration,
}

impl Timeouts {
    /// The bounds every client that serves a command carries.
    pub const PRODUCTION: Self = Self {
        request: REQUEST_TIMEOUT,
        connect: CONNECT_TIMEOUT,
    };
}

/// The header both UniFi APIs take the user's key in.
///
/// Neither API uses `Authorization`, which is what makes a redirect dangerous:
/// see [`build_client`].
const API_KEY_HEADER: &str = "x-api-key";

/// The only media type either API answers in.
const JSON_MEDIA_TYPE: &str = "application/json";

/// How much of a response body an error quotes before cutting it short.
///
/// A server can answer with anything at all -- a stack trace, an HTML error
/// page, a dump of the record that failed -- and the whole of it in an error
/// floods the terminal and lands in whatever collects the user's logs. This
/// is enough to recognise what came back and to read a short message that was
/// not in the API's own error shape.
const ERROR_BODY_DISPLAY_CHARS: usize = 500;

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

/// Build the HTTP client that talks to `api`.
///
/// This is the one place a client that carries the user's API key is built.
/// Both APIs take the key in the same custom header and both answer JSON, so
/// every setting that guards the key is the same for both -- and a setting
/// that guards the key in one module guards nothing while a second builder in
/// another module leaves it out. `reqwest::Client::builder` is banned in
/// `src/ufa/clippy.toml` to keep this the only entrance.
///
/// # A redirect is refused rather than followed
///
/// reqwest follows up to ten redirects unless a policy says otherwise. When
/// the host changes it drops `Authorization`, `Cookie`, `Cookie2`,
/// `Proxy-Authorization` and `WWW-Authenticate`, and it keeps every other
/// header. [`API_KEY_HEADER`] is none of those, so a controller that answers
/// 3xx with a host of its choosing reads the user's key off the next request.
/// Under `--insecure` no certificate stands in the way of that host either.
///
/// A JSON API resource does not move, so nothing is lost by a refusal: the 3xx
/// comes back to the caller, and [`read_json_response`] reports it as the
/// failure it is.
///
/// # Every request is bounded
///
/// A host that accepts a connection and answers nothing holds a command for as
/// long as the user lets it, because reqwest puts no bound on a request of its
/// own. Two bounds go on instead. `timeout` covers the whole request, which is
/// what ends a wait for an answer that never comes, and `connect_timeout`
/// covers the connection alone, which ends much sooner against a host that
/// drops the packets rather than refuses them.
///
/// # Arguments
///
/// * `api` - Which API the client talks to, which names the key in any error.
/// * `api_key` - The user's key for that API.
/// * `insecure` - Whether to accept a certificate that does not verify.
/// * `timeouts` - The bounds to put on every request.
///
/// # Returns
///
/// The client, ready to send requests.
///
/// # Errors
///
/// Returns an error if `api_key` cannot be a header value, or if the client
/// cannot be built.
pub fn build_client(api: Api, api_key: &str, insecure: bool, timeouts: Timeouts) -> Result<Client> {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::HeaderName::from_static(API_KEY_HEADER),
        header::HeaderValue::from_str(api_key)
            .with_context(|| format!("Invalid {} key", api.label()))?,
    );
    headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static(JSON_MEDIA_TYPE),
    );

    #[expect(
        clippy::disallowed_methods,
        reason = "the one entrance the ban exists to hold: an #[expect] here also fails the build on the day the ban stops matching this call"
    )]
    let builder = Client::builder();

    builder
        .default_headers(headers)
        .redirect(redirect::Policy::none())
        .timeout(timeouts.request)
        .connect_timeout(timeouts.connect)
        .danger_accept_invalid_certs(insecure)
        .build()
        .context("Failed to create HTTP client")
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

    parse_body(api, &body)
}

/// Deserialize a successful response body as `T`.
///
/// # Arguments
///
/// * `api` - Which API answered, which decides how the error names it.
/// * `body` - The response body.
///
/// # Returns
///
/// The deserialized body.
///
/// # Errors
///
/// Returns an error if `body` is not the JSON `T` expects.
fn parse_body<T>(api: Api, body: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_str(body).with_context(|| {
        format!(
            "Failed to parse {} response JSON: {}",
            api.label(),
            truncate_for_display(body, ERROR_BODY_DISPLAY_CHARS)
        )
    })
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

    anyhow::anyhow!(
        "{} HTTP error {status}: {}",
        api.label(),
        truncate_for_display(body, ERROR_BODY_DISPLAY_CHARS)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::TestServer;
    use std::time::Instant;

    /// A response body big enough that reproducing it in an error would fill
    /// the terminal -- and, if the error is logged, the log.
    const HUGE_BODY_CHARS: usize = 100_000;

    /// The most an error message may reasonably grow to. Well above any
    /// sensible excerpt budget, so this asserts "the body was cut short at
    /// all" rather than pinning a particular budget.
    const REASONABLE_ERROR_CHARS: usize = 4_000;

    /// A controller that accepts the connection and never answers must not be
    /// able to hold a command forever.
    ///
    /// Every command this CLI runs makes a request through this client, and
    /// the worst case is the confirmed DELETE: a user who kills a request that
    /// hangs cannot tell whether the vouchers were destroyed first.
    #[tokio::test]
    async fn a_request_to_a_server_that_never_answers_ends_on_its_own_bound() {
        /// The bound the client under test carries. Far shorter than the
        /// production one, which no test may wait for.
        const CLIENT_BOUND: Duration = Duration::from_millis(250);
        /// How long the test waits before it calls the bound broken. Twenty
        /// times the bound above, so a loaded machine cannot fail this, and
        /// far under any production value, so a client with no bound at all
        /// cannot pass it.
        const TEST_PATIENCE: Duration = Duration::from_secs(5);

        let silent = TestServer::silent().await;
        let client = build_client(
            Api::Controller,
            "an-api-key",
            false,
            Timeouts {
                request: CLIENT_BOUND,
                connect: CLIENT_BOUND,
            },
        )
        .expect("a client with a short bound must build");

        let started = Instant::now();
        let outcome = tokio::time::timeout(TEST_PATIENCE, client.get(silent.origin()).send()).await;
        let elapsed = started.elapsed();

        let error = outcome
            .expect("the request must end on the client's own bound, not on the test's")
            .expect_err("a server that never answers cannot produce a response");

        assert!(
            error.is_timeout(),
            "the request must fail as a timeout rather than as anything else, got: {error}"
        );
        assert!(
            elapsed < TEST_PATIENCE,
            "the request ran for {elapsed:?}, which is no bound at all"
        );
    }

    /// A server can answer a failed request with anything at all -- a stack
    /// trace, an HTML error page, a database dump. Interpolating the lot into
    /// the error floods the user's terminal, and lands in whatever collects
    /// their logs.
    #[test]
    fn a_huge_error_body_is_quoted_only_in_part() {
        let body = "x".repeat(HUGE_BODY_CHARS);

        let rendered =
            response_error(Api::Controller, StatusCode::INTERNAL_SERVER_ERROR, &body).to_string();

        assert!(
            rendered.chars().count() < REASONABLE_ERROR_CHARS,
            "the error reproduced {} characters of a {HUGE_BODY_CHARS}-character body",
            rendered.chars().count()
        );
        assert!(
            rendered.contains("500"),
            "the error must still report the status, got: {rendered}"
        );
        assert!(
            rendered.contains("xxxx"),
            "the error must still quote the start of the body, got: {rendered}"
        );
    }

    /// The same, for the body of a response that succeeded but did not carry
    /// the JSON the caller expected.
    #[test]
    fn a_huge_unparseable_body_is_quoted_only_in_part() {
        let body = "x".repeat(HUGE_BODY_CHARS);

        let error = parse_body::<serde_json::Value>(Api::SiteManager, &body)
            .expect_err("a body of x's is not JSON");
        let rendered = format!("{error:#}");

        assert!(
            rendered.chars().count() < REASONABLE_ERROR_CHARS,
            "the error reproduced {} characters of a {HUGE_BODY_CHARS}-character body",
            rendered.chars().count()
        );
    }

    /// Cutting a body short must count characters, not bytes: a body of
    /// multi-byte characters would otherwise be split mid-character.
    #[test]
    fn a_multi_byte_error_body_is_cut_short_without_panicking() {
        let body = "日本語🎉café".repeat(HUGE_BODY_CHARS);

        let rendered = response_error(Api::SiteManager, StatusCode::BAD_GATEWAY, &body).to_string();

        assert!(
            rendered.chars().count() < REASONABLE_ERROR_CHARS,
            "the error reproduced {} characters of a multi-byte body",
            rendered.chars().count()
        );
        assert!(
            rendered.contains('日'),
            "the error must still quote the start of the body, got: {rendered}"
        );
    }

    /// A body short enough to read in full is still shown in full, with no
    /// ellipsis suggesting something was withheld.
    #[test]
    fn a_short_error_body_is_quoted_in_full() {
        let body = "upstream connect error";

        let rendered = response_error(Api::Controller, StatusCode::BAD_GATEWAY, body).to_string();

        assert!(
            rendered.contains(body),
            "a short body must survive intact, got: {rendered}"
        );
        assert!(
            !rendered.contains("..."),
            "a body that was not cut short must not read as if it were, got: {rendered}"
        );
    }
}
