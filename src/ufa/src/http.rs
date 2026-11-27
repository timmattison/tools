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
use std::fmt;
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
/// is enough to recognise what came back and to read a short message. The
/// bound applies to a body that is not in the API's error shape and to the
/// message of a body that is.
const ERROR_BODY_DISPLAY_CHARS: usize = 500;

/// The most of one response body `ufa` reads.
///
/// Both APIs answer one JSON document, and what either document can hold is
/// bounded by the account behind it. The controller's collections arrive a
/// page at a time -- 200 items per request, per [`crate::pagination`] -- and
/// the largest item either API returns is a device record of a few kilobytes,
/// so a page stays under a megabyte. The Site Manager host listing arrives a
/// page at a time too, 25 hosts by default, so each answer of either API is a
/// page or one record.
///
/// 32 MiB is far above both. That distance matters more than the number
/// itself, because a ceiling a real answer trips is worse than no ceiling at
/// all. What it bounds is the rest: a server that answers with a body of no
/// stated end.
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

/// The error body a UniFi API answers a failed request with.
///
/// Both APIs use the same shape; any further fields they carry (an error
/// code, a request id) are not surfaced to the user, so they are ignored.
#[derive(Deserialize)]
struct ApiErrorBody {
    message: String,
}

/// Why a body could not be read, when a lower layer says why.
type ReadCause = Box<dyn std::error::Error + Send + Sync + 'static>;

/// The cause of an answer error, as the `source` of that error.
///
/// # Arguments
///
/// * `cause` - The cause that the error holds, if any.
///
/// # Returns
///
/// The cause, as a plain error reference.
fn source_of(cause: Option<&ReadCause>) -> Option<&(dyn std::error::Error + 'static)> {
    cause.map(|cause| {
        let cause: &(dyn std::error::Error + 'static) = cause.as_ref();
        cause
    })
}

/// A UniFi API answered with a status that is not a success.
///
/// The error keeps the status beside the message. A caller reads the status
/// back through [`answered_status`], and never from the text of the message.
#[derive(Debug)]
struct StatusError {
    /// The status the API answered with.
    status: StatusCode,
    /// What the user reads.
    message: String,
    /// Why the body could not be read, when the read broke.
    source: Option<ReadCause>,
}

impl fmt::Display for StatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for StatusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        source_of(self.source.as_ref())
    }
}

/// The status of the answer that `error` reports, when a UniFi API answered
/// a request with a status that is not a success. [`failed_answer`] tells the
/// other failures apart.
///
/// # Arguments
///
/// * `error` - An error from a request to a UniFi API.
///
/// # Returns
///
/// The status. `None` when the request failed before a status arrived, and
/// when a successful answer could not be read.
pub fn answered_status(error: &anyhow::Error) -> Option<StatusCode> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<StatusError>())
        .map(|answer| answer.status)
}

/// A UniFi API answered with a success status, and ufa could not read the
/// body.
///
/// The error keeps the status beside the message. A caller reads the status
/// back through [`failed_answer`], and never from the text of the message.
#[derive(Debug)]
struct UnreadableAnswer {
    /// The status the API answered with.
    status: StatusCode,
    /// What the user reads.
    message: String,
    /// Why the body could not be read, when a lower layer says why.
    source: Option<ReadCause>,
}

impl fmt::Display for UnreadableAnswer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for UnreadableAnswer {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        source_of(self.source.as_ref())
    }
}

/// How far a failed request to a UniFi API got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailedAnswer {
    /// No answer arrived: the connection failed or timed out before the
    /// status arrived.
    Missing,
    /// The API answered with a status that is not a success.
    Refused(StatusCode),
    /// The API answered with a success status, and ufa could not read the
    /// body. The body was too large, broke before its end, or was not the
    /// JSON that the caller expects.
    Unreadable(StatusCode),
}

/// How far the request that `error` reports got.
///
/// # Arguments
///
/// * `error` - An error from a request to a UniFi API.
///
/// # Returns
///
/// [`FailedAnswer::Refused`] when the API answered with a status that is not
/// a success. [`FailedAnswer::Unreadable`] when it answered with a success
/// status and ufa could not read the body. [`FailedAnswer::Missing`] when no
/// status arrived.
pub fn failed_answer(error: &anyhow::Error) -> FailedAnswer {
    if let Some(status) = answered_status(error) {
        return FailedAnswer::Refused(status);
    }

    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<UnreadableAnswer>())
        .map_or(FailedAnswer::Missing, |answer| {
            FailedAnswer::Unreadable(answer.status)
        })
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
                "Authentication failed (HTTP {status}). Please check your API key or generate a new one in Settings -> Control Plane -> Integrations."
            ),
            Api::SiteManager => format!(
                "Site Manager authentication failed (HTTP {status}). Please check your Site Manager API key."
            ),
        }
    }

    /// The advice about the key for a status that refuses it.
    ///
    /// # Arguments
    ///
    /// * `status` - The status this API answered with.
    ///
    /// # Returns
    ///
    /// The message of [`Api::authentication_failure`] for a 401 or a 403,
    /// whatever the body. `None` for any other status.
    fn key_refusal(self, status: StatusCode) -> Option<String> {
        (status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN)
            .then(|| self.authentication_failure(status))
    }
}

/// Give `builder` the transport that each client of `ufa` uses.
///
/// Both builders of the crate start here: [`build_client`], and the probe of
/// `crate::discovery`. This function names the TLS backend, and in a build for
/// the tests, it turns off each proxy of the environment.
///
/// # rustls, in each build
///
/// The features of reqwest in `src/ufa/Cargo.toml` do not decide the backend
/// in a build of the whole workspace. Cargo unifies the features of reqwest
/// over each package that it builds. `gitrdun`, `krt` and `tvfind` take the
/// default features, and `default-tls` is one of them. When no builder names a
/// backend, reqwest then picks native-tls. native-tls reports a refused
/// handshake as an error of its own, and `is_tls_failure` in `crate::client`
/// finds only the shape of a rustls refusal. Thus this function names rustls.
///
/// # No proxy in a build for the tests
///
/// reqwest reads `HTTP_PROXY`, `HTTPS_PROXY` and `ALL_PROXY` when it builds a
/// client. A test talks to a `TestServer` on the loopback address, so a proxy
/// that the shell of a developer exports gets each request of that test, and
/// the test fails. A build that is not a test keeps the proxy of the user.
///
/// # Arguments
///
/// * `builder` - The builder of a client.
///
/// # Returns
///
/// The builder, with rustls as its TLS backend, and with no proxy in a build
/// for the tests.
pub(crate) fn with_ufa_transport(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    let builder = builder.use_rustls_tls();
    #[cfg(test)]
    {
        builder.no_proxy()
    }
    #[cfg(not(test))]
    {
        builder
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

    // reqwest's own `Debug` prints every default header it carries, with no
    // condition on the name, so a client dumped while somebody debugs a request
    // prints the user's key in full. A header value marked sensitive prints as
    // `Sensitive` wherever it is printed from, which is the same answer the
    // `Secret` newtype gives everywhere else in the crate.
    let mut key = header::HeaderValue::from_str(api_key)
        .with_context(|| format!("Invalid {} key", api.label()))?;
    key.set_sensitive(true);

    headers.insert(header::HeaderName::from_static(API_KEY_HEADER), key);
    headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static(JSON_MEDIA_TYPE),
    );

    #[expect(
        clippy::disallowed_methods,
        reason = "the one entrance the ban exists to hold: an #[expect] here also fails the build on the day the ban stops matching this call"
    )]
    let builder = Client::builder();

    with_ufa_transport(builder)
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
    read_json_response_within(api, response, MAX_RESPONSE_BYTES).await
}

/// Read `response` for its status, and keep no document from its body.
///
/// The integration API documents the success answer of the actions of a
/// device and of a port with no content. A controller can thus send no body,
/// and an empty body is not JSON. A failure is reported as
/// [`read_json_response`] reports it.
///
/// # Arguments
///
/// * `api` - Which API answered, which decides how errors name it.
/// * `response` - The response to read.
///
/// # Errors
///
/// Returns an error if the body cannot be read, or if the status is not a
/// success.
pub async fn read_response_without_document(api: Api, response: reqwest::Response) -> Result<()> {
    read_success_body(api, response, MAX_RESPONSE_BYTES)
        .await
        .map(|_| ())
}

/// Read `response` as `T`, reading no more than `limit` bytes of its body.
///
/// The bound is a parameter rather than the constant itself, because a test
/// of the bound must trip it, and no test sends [`MAX_RESPONSE_BYTES`]. It is
/// not a parameter of [`read_json_response`], because how much of an answer
/// `ufa` reads is a property of `ufa` rather than of the command that asked.
///
/// # Arguments
///
/// * `api` - Which API answered, which decides how errors name it.
/// * `response` - The response to read.
/// * `limit` - The most of the body to read, in bytes.
///
/// # Returns
///
/// The deserialized body.
///
/// # Errors
///
/// Returns an error if the body is larger than `limit`, if the body cannot be
/// read, if the status is not a success, or if the body is not the JSON `T`
/// expects.
async fn read_json_response_within<T>(
    api: Api,
    response: reqwest::Response,
    limit: u64,
) -> Result<T>
where
    T: DeserializeOwned,
{
    let (status, body) = read_success_body(api, response, limit).await?;

    parse_body(api, status, &body)
}

/// Read the body of `response`, up to `limit` bytes, and refuse a status that
/// is not a success.
///
/// # Arguments
///
/// * `api` - Which API answered, which decides how errors name it.
/// * `response` - The response to read.
/// * `limit` - The most of the body to read, in bytes.
///
/// # Returns
///
/// The success status and the whole body, as text.
///
/// # Errors
///
/// Returns an error if the body is larger than `limit`, if the body cannot be
/// read, or if the status is not a success.
async fn read_success_body(
    api: Api,
    response: reqwest::Response,
    limit: u64,
) -> Result<(StatusCode, String)> {
    let status = response.status();
    let body = read_body(api, response, limit).await?;

    if !status.is_success() {
        return Err(response_error(api, status, &body));
    }

    Ok((status, body))
}

/// Read the whole body of `response`, up to `limit` bytes of it.
///
/// # Two checks, because one shape hides from each
///
/// A response that states a `Content-Length` over the bound is refused on that
/// statement alone, before a byte of the body arrives. This is the check that
/// costs nothing, and it catches the accident: an endpoint that answers with
/// far more than the command asked for.
///
/// A chunked response states no length at all, so nothing is there to refuse.
/// The read counts what arrives instead and stops at the same bound, which
/// costs the bound in memory and catches the rest, an adversary included. A
/// stated length can also be a lie, and the running count is what makes the
/// first check an early exit rather than the whole guard.
///
/// The bytes are read as UTF-8, which is what a JSON document is. A response
/// that states another character set is not decoded from it, and any byte that
/// is not UTF-8 arrives as the replacement character -- the same answer the
/// error path gave before, for a body neither API sends.
///
/// # Arguments
///
/// * `api` - Which API answered, which decides how the error names it.
/// * `response` - The response to read.
/// * `limit` - The most of the body to read, in bytes.
///
/// # Returns
///
/// The whole body, as text.
///
/// # Errors
///
/// Returns an error if the body is larger than `limit`, or if the connection
/// fails or times out before the body is complete. Each error carries the
/// status, as [`unread_answer`] says.
async fn read_body(api: Api, mut response: reqwest::Response, limit: u64) -> Result<String> {
    let status = response.status();
    let ceiling = usize::try_from(limit).unwrap_or(usize::MAX);

    if let Some(stated) = response.content_length() {
        if stated > limit {
            return Err(body_too_large(api, status, stated, limit));
        }
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|cause| broken_body(api, status, cause))?
    {
        body.extend_from_slice(&chunk);

        if body.len() > ceiling {
            let read = u64::try_from(body.len()).unwrap_or(u64::MAX);
            return Err(body_too_large(api, status, read, limit));
        }
    }

    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Build the error for a body this CLI refuses to hold.
///
/// # Arguments
///
/// * `api` - Which API answered.
/// * `status` - The status it answered with, which is the one thing about the
///   answer that is known when the body cannot be read.
/// * `size` - How large the body is known to be, in bytes.
/// * `limit` - The bound it passed, in bytes.
///
/// # Returns
///
/// The error to raise, from [`unread_answer`]. A 401 or a 403 starts its
/// message with the advice about the key that a short body gets, and then
/// names the size.
fn body_too_large(api: Api, status: StatusCode, size: u64, limit: u64) -> anyhow::Error {
    let size_text = format!(
        "{} answered HTTP {status} with a body of at least {size} bytes, over the {limit} bytes \
         ufa reads.",
        api.label()
    );
    let message = match api.key_refusal(status) {
        Some(advice) => format!("{advice} {size_text}"),
        None => format!(
            "{size_text} Ask for a smaller part of the collection, or check the server that \
             answered."
        ),
    };

    unread_answer(status, message, None)
}

/// Build the error for an answer whose status arrived and whose body ufa
/// could not read.
///
/// # Arguments
///
/// * `status` - The status the API answered with.
/// * `message` - What the user reads.
/// * `source` - Why the body could not be read, if a lower layer says why.
///
/// # Returns
///
/// The error to raise. It carries `status`. A success status gives an
/// [`UnreadableAnswer`]. Any other status gives a [`StatusError`], the same
/// as a refusal with a short body, so [`answered_status`] reads a 401 back
/// whatever the state of its body.
fn unread_answer(status: StatusCode, message: String, source: Option<ReadCause>) -> anyhow::Error {
    if status.is_success() {
        anyhow::Error::new(UnreadableAnswer {
            status,
            message,
            source,
        })
    } else {
        anyhow::Error::new(StatusError {
            status,
            message,
            source,
        })
    }
}

/// Build the error for a body whose read broke after the status arrived.
///
/// The connection closed or timed out before the body was complete.
///
/// # Arguments
///
/// * `api` - Which API answered.
/// * `status` - The status it answered with.
/// * `cause` - The error of the broken read. It stays the source of the error.
///
/// # Returns
///
/// The error to raise, from [`unread_answer`]. A 401 or a 403 starts its
/// message with the advice about the key. The message has no full stop at
/// its end, because `{:#}` puts the cause after it.
fn broken_body(api: Api, status: StatusCode, cause: reqwest::Error) -> anyhow::Error {
    let text = format!(
        "{} answered HTTP {status}, and ufa could not read the whole body",
        api.label()
    );
    let message = match api.key_refusal(status) {
        Some(advice) => format!("{advice} {text}"),
        None => text,
    };

    unread_answer(status, message, Some(Box::new(cause)))
}

/// Deserialize a successful response body as `T`.
///
/// # Arguments
///
/// * `api` - Which API answered, which decides how the error names it.
/// * `status` - The success status it answered with.
/// * `body` - The response body.
///
/// # Returns
///
/// The deserialized body.
///
/// # Errors
///
/// Returns an error if `body` is not the JSON `T` expects.
fn parse_body<T>(api: Api, status: StatusCode, body: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_str(body).map_err(|cause| {
        anyhow::Error::new(UnreadableAnswer {
            status,
            message: format!(
                "Failed to parse {} response JSON: {}",
                api.label(),
                truncate_for_display(body, ERROR_BODY_DISPLAY_CHARS)
            ),
            source: Some(Box::new(cause)),
        })
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
/// The error to raise. It carries `status`, which [`answered_status`] reads
/// back.
fn response_error(api: Api, status: StatusCode, body: &str) -> anyhow::Error {
    let message = if let Some(advice) = api.key_refusal(status) {
        advice
    } else if let Ok(error) = serde_json::from_str::<ApiErrorBody>(body) {
        format!(
            "{} error: {} (HTTP {status})",
            api.label(),
            truncate_for_display(&error.message, ERROR_BODY_DISPLAY_CHARS)
        )
    } else {
        format!(
            "{} HTTP error {status}: {}",
            api.label(),
            truncate_for_display(body, ERROR_BODY_DISPLAY_CHARS)
        )
    };

    anyhow::Error::new(StatusError {
        status,
        message,
        source: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{
        chunked_response, json_response, json_response_with_status, truncated_response, TestServer,
    };
    use std::time::Instant;

    /// What the `http` crate's own debug output says in place of a header
    /// value somebody marked sensitive.
    const SENSITIVE: &str = "Sensitive";

    /// A response body big enough that reproducing it in an error would fill
    /// the terminal -- and, if the error is logged, the log.
    const HUGE_BODY_CHARS: usize = 100_000;

    /// The most an error message may reasonably grow to. Well above any
    /// sensible excerpt budget, so this asserts "the body was cut short at
    /// all" rather than pinning a particular budget.
    const REASONABLE_ERROR_CHARS: usize = 4_000;

    /// The client carries the user's key in a default header, and reqwest's
    /// own debug output prints its default headers — every one of them, with
    /// no condition on the name. So a client dumped while somebody debugs a
    /// request prints the key in full, which is the one value in the whole
    /// program that must never be printed.
    ///
    /// The `http` crate offers the answer: a header value marked sensitive
    /// prints as `Sensitive` and nothing else, wherever it is printed from.
    #[test]
    fn a_debug_dump_of_the_client_does_not_carry_the_api_key() {
        use crate::test_support::FAKE_CONTROLLER_KEY;

        let client = build_client(
            Api::Controller,
            FAKE_CONTROLLER_KEY,
            false,
            Timeouts::PRODUCTION,
        )
        .expect("a client with a well-formed key must build");

        let dump = format!("{client:?}");

        assert!(
            !dump.contains(FAKE_CONTROLLER_KEY),
            "reqwest prints its default headers, and the API key is one of them, got {dump}"
        );
        assert!(
            dump.contains(SENSITIVE),
            "the key header must say {SENSITIVE} in place of what it holds, got {dump}"
        );
    }

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

    /// A failure answer in the API error shape quotes its message, and a huge
    /// message is cut short like any other body.
    #[test]
    fn a_huge_error_message_is_quoted_only_in_part() {
        let body = serde_json::json!({ "message": "x".repeat(HUGE_BODY_CHARS) }).to_string();

        let rendered =
            response_error(Api::Controller, StatusCode::INTERNAL_SERVER_ERROR, &body).to_string();

        assert!(
            rendered.chars().count() < REASONABLE_ERROR_CHARS,
            "the error reproduced {} characters of a {HUGE_BODY_CHARS}-character message",
            rendered.chars().count()
        );
        assert!(
            rendered.contains("API error: xxxx"),
            "the error must quote the start of the message, got: {}",
            rendered.chars().take(200).collect::<String>()
        );
    }

    /// The same, for the body of a response that succeeded but did not carry
    /// the JSON the caller expected.
    #[test]
    fn a_huge_unparseable_body_is_quoted_only_in_part() {
        let body = "x".repeat(HUGE_BODY_CHARS);

        let error = parse_body::<serde_json::Value>(Api::SiteManager, StatusCode::OK, &body)
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

    /// The bound the reads under test carry. Far below the production one,
    /// which no test sends.
    const TEST_LIMIT: u64 = 1_024;

    /// A document that is four times the bound above, and valid JSON, so a
    /// read that ignores the bound succeeds rather than fails on the parse.
    fn over_the_limit() -> String {
        format!("\"{}\"", "x".repeat(4 * usize_limit()))
    }

    /// [`TEST_LIMIT`] as a count of characters.
    fn usize_limit() -> usize {
        usize::try_from(TEST_LIMIT).expect("the test bound fits a usize")
    }

    /// Ask `server` for its one answer.
    ///
    /// The client is the production one, so what the test reads is what a
    /// command reads.
    async fn fetch(server: &TestServer) -> reqwest::Response {
        build_client(Api::Controller, "an-api-key", false, Timeouts::PRODUCTION)
            .expect("a client with a well-formed key must build")
            .get(server.origin())
            .send()
            .await
            .expect("the test server must answer")
    }

    /// A server that states an over-long body is refused on the statement
    /// alone, before the body is read. This is the accident: an endpoint that
    /// answers with far more than the command asked for.
    #[tokio::test]
    async fn a_declared_body_over_the_limit_is_refused() {
        let server = TestServer::replying(&json_response(&over_the_limit())).await;

        let error = read_json_response_within::<serde_json::Value>(
            Api::Controller,
            fetch(&server).await,
            TEST_LIMIT,
        )
        .await
        .expect_err("a body over the limit must not be read into memory");

        let rendered = format!("{error:#}");
        assert!(
            rendered.contains(&TEST_LIMIT.to_string()),
            "the error must name the limit the user tripped, got: {rendered}"
        );
    }

    /// A chunked answer states no length at all, so a check of the stated
    /// length sees nothing to refuse. The read counts what arrives instead.
    #[tokio::test]
    async fn a_chunked_body_over_the_limit_is_refused() {
        let server = TestServer::replying(&chunked_response(&over_the_limit())).await;
        let response = fetch(&server).await;

        assert_eq!(
            response.content_length(),
            None,
            "a chunked answer must state no length, or this tests the other guard"
        );

        let error =
            read_json_response_within::<serde_json::Value>(Api::Controller, response, TEST_LIMIT)
                .await
                .expect_err("a body that passes the limit as it arrives must be refused");

        let rendered = format!("{error:#}");
        assert!(
            rendered.contains(&TEST_LIMIT.to_string()),
            "the error must name the limit the user tripped, got: {rendered}"
        );
    }

    /// The bound must not cost the answers that are inside it. A ceiling a
    /// real answer trips is worse than no ceiling.
    #[tokio::test]
    async fn a_body_under_the_limit_is_read_in_full() {
        let document = format!("\"{}\"", "x".repeat(usize_limit() / 2));

        for answer in [json_response(&document), chunked_response(&document)] {
            let server = TestServer::replying(&answer).await;

            let value: serde_json::Value =
                read_json_response_within(Api::Controller, fetch(&server).await, TEST_LIMIT)
                    .await
                    .expect("a body inside the limit must be read");

            assert_eq!(
                serde_json::to_string(&value).expect("a JSON value renders"),
                document,
                "the whole body must survive the read, chunked or not"
            );
        }
    }

    /// Read the answer of `server` as JSON that no test body matches, and
    /// classify the error.
    async fn classify(server: &TestServer) -> (anyhow::Error, FailedAnswer) {
        let error = read_json_response_within::<Vec<String>>(
            Api::Controller,
            fetch(server).await,
            TEST_LIMIT,
        )
        .await
        .expect_err("the body is not a list of strings");
        let class = failed_answer(&error);
        (error, class)
    }

    /// The part of a JSON body that a broken read leaves behind.
    const BROKEN_BODY: &str = r#"{"vouchers":["#;

    /// A success answer whose body stops before its end is unreadable. The
    /// status arrived, so the answer is not a missing one, and the error keeps
    /// the cause of the broken read.
    #[tokio::test]
    async fn a_success_answer_whose_body_breaks_is_unreadable() {
        let server = TestServer::replying(&truncated_response("201 Created", BROKEN_BODY)).await;

        let (error, class) = classify(&server).await;

        assert_eq!(class, FailedAnswer::Unreadable(StatusCode::CREATED));
        assert!(
            error.chain().count() >= 2,
            "the error must keep the cause of the broken read, got: {error:#}"
        );
    }

    /// An error answer whose body stops before its end is still a refusal. A
    /// command reads the status of a 401 or a 403 to decide to read the key
    /// from 1Password again, and a broken body does not change what that
    /// status says about the key.
    #[tokio::test]
    async fn an_error_answer_whose_body_breaks_is_refused() {
        for (line, status) in [
            (
                "500 Internal Server Error",
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            ("401 Unauthorized", StatusCode::UNAUTHORIZED),
            ("403 Forbidden", StatusCode::FORBIDDEN),
        ] {
            let server = TestServer::replying(&truncated_response(line, BROKEN_BODY)).await;

            let (error, class) = classify(&server).await;

            assert_eq!(
                class,
                FailedAnswer::Refused(status),
                "status {status}, got: {error:#}"
            );
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                let rendered = format!("{error:#}");
                assert!(
                    rendered.starts_with(&Api::Controller.authentication_failure(status)),
                    "the message must give the advice about the key, got: {rendered}"
                );
            }
        }
    }

    /// A hang-up gives no answer at all.
    #[tokio::test]
    async fn a_hang_up_is_a_missing_answer() {
        let server = TestServer::hanging_up().await;

        let error = build_client(Api::Controller, "an-api-key", false, Timeouts::PRODUCTION)
            .expect("a client with a well-formed key must build")
            .get(server.origin())
            .send()
            .await
            .map_err(anyhow::Error::new)
            .expect_err("a server that hangs up gives no answer");

        assert_eq!(failed_answer(&error), FailedAnswer::Missing);
    }

    /// An answer with a status that is not a success is a refusal.
    #[tokio::test]
    async fn a_not_found_answer_is_refused() {
        let server = TestServer::replying(&json_response_with_status("404 Not Found", "{}")).await;

        let (_, class) = classify(&server).await;

        assert_eq!(class, FailedAnswer::Refused(StatusCode::NOT_FOUND));
    }

    /// A success answer with a body of another shape is unreadable, and the
    /// text of the error is the text it had before the class existed.
    #[tokio::test]
    async fn a_success_answer_of_another_shape_is_unreadable() {
        let server = TestServer::replying(&json_response(r#"{"deleted":true}"#)).await;

        let (error, class) = classify(&server).await;

        assert_eq!(class, FailedAnswer::Unreadable(StatusCode::OK));
        let rendered = format!("{error:#}");
        assert!(
            rendered.starts_with("Failed to parse"),
            "the text of the error must not change, got: {rendered}"
        );
        assert!(
            rendered.contains("invalid type"),
            "the text must still carry the parser error, got: {rendered}"
        );
    }

    /// A body over the limit comes with a status, and the class says so.
    #[tokio::test]
    async fn a_body_over_the_limit_is_unreadable() {
        let server = TestServer::replying(&json_response(&over_the_limit())).await;

        let (_, class) = classify(&server).await;

        assert_eq!(class, FailedAnswer::Unreadable(StatusCode::OK));
    }

    /// An error answer with a body over the limit is still an answer. The
    /// status arrived, and the read stopped at the limit, so the class is a
    /// refusal and not a missing answer.
    #[tokio::test]
    async fn an_error_answer_with_a_body_over_the_limit_is_refused() {
        let server = TestServer::replying(&json_response_with_status(
            "500 Internal Server Error",
            &over_the_limit(),
        ))
        .await;

        let (_, class) = classify(&server).await;

        assert_eq!(
            class,
            FailedAnswer::Refused(StatusCode::INTERNAL_SERVER_ERROR)
        );
    }

    /// A refusal of the key with a body over the limit still gives its
    /// status to `answered_status`. A command reads that status to decide to
    /// read the key from 1Password again, and the size of the body does not
    /// change what a 401 or a 403 says about the key. The message starts
    /// with the advice about the key, as it does for a short body, and then
    /// names the limit.
    #[tokio::test]
    async fn a_refused_key_with_a_body_over_the_limit_keeps_its_status_and_advice() {
        for (line, status) in [
            ("401 Unauthorized", StatusCode::UNAUTHORIZED),
            ("403 Forbidden", StatusCode::FORBIDDEN),
        ] {
            let server =
                TestServer::replying(&json_response_with_status(line, &over_the_limit())).await;

            let (error, _) = classify(&server).await;

            assert_eq!(answered_status(&error), Some(status), "got: {error:#}");
            let rendered = format!("{error:#}");
            assert!(
                rendered.starts_with(&Api::Controller.authentication_failure(status)),
                "the message must give the advice about the key, got: {rendered}"
            );
            assert!(
                rendered.contains(&TEST_LIMIT.to_string()),
                "the message must still name the limit, got: {rendered}"
            );
        }
    }

    /// The advice about the key and the size of the body are two sentences.
    /// A full stop must close the advice before the size text names the API,
    /// or the message joins them into one sentence that names a different
    /// API ("... Integrations API answered ...").
    #[test]
    fn a_refused_key_with_a_body_over_the_limit_names_the_size_in_its_own_sentence() {
        for api in [Api::Controller, Api::SiteManager] {
            for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
                let rendered = format!(
                    "{:#}",
                    body_too_large(api, status, 4 * TEST_LIMIT, TEST_LIMIT)
                );

                let boundary = format!(". {} answered HTTP {status}", api.label());
                assert!(
                    rendered.contains(&boundary),
                    "a full stop must close the advice before {boundary:?}, got: {rendered}"
                );
            }
        }
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
