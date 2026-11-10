use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One answer of `gh api -i`, split into its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The status code of the HTTP status line.
    status: u16,
    /// The reason phrase of the HTTP status line. It can be empty.
    reason: String,
    /// The headers, in the order and the case that `gh` printed them.
    headers: Vec<(String, String)>,
    /// Everything after the empty line that ends the headers.
    body: String,
}

impl Response {
    /// The status code of the HTTP status line.
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The reason phrase of the HTTP status line. It can be empty.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// The headers, in the order and the case that `gh` printed them.
    #[must_use]
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// Everything after the empty line that ends the headers.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Gives the body away, with no copy.
    #[must_use]
    pub fn into_body(self) -> String {
        self.body
    }

    /// The value of the first header with this name. The name match ignores case.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// How long to wait after GitHub refused a read because of a rate limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitWait {
    /// The reply states the wait.
    Stated(Duration),
    /// The reply is a rate limit, but it gives no usable wait. The caller picks one.
    Unstated,
}

impl Response {
    /// Reads a rate limit refusal out of the reply.
    ///
    /// Returns `None` unless the status is `403` or `429`. For those two
    /// statuses the rules run in this order:
    ///
    /// 1. A `retry-after` header that holds whole seconds gives
    ///    `Stated` with those seconds.
    /// 2. A `retry-after` header that does not parse gives `Unstated`.
    /// 3. `x-ratelimit-remaining: 0` with an `x-ratelimit-reset` header in
    ///    epoch seconds gives `Stated` with the time from `now` to the reset.
    ///    A reset in the past gives a wait of zero.
    /// 4. `x-ratelimit-remaining: 0` with no usable reset gives `Unstated`.
    /// 5. Any other reply gives `None`. A `403` with no rate limit header is a
    ///    plain refusal, for example a missing permission.
    #[must_use]
    pub fn rate_limit_wait(&self, now: SystemTime) -> Option<RateLimitWait> {
        if self.status != 403 && self.status != 429 {
            return None;
        }
        if let Some(value) = self.header("retry-after") {
            return Some(match value.parse::<u64>() {
                Ok(seconds) => RateLimitWait::Stated(Duration::from_secs(seconds)),
                Err(_) => RateLimitWait::Unstated,
            });
        }
        if self.header("x-ratelimit-remaining") != Some("0") {
            return None;
        }
        let reset = self
            .header("x-ratelimit-reset")
            .and_then(|value| value.parse::<u64>().ok());
        Some(match reset {
            Some(reset) => {
                let reset = UNIX_EPOCH + Duration::from_secs(reset);
                RateLimitWait::Stated(reset.duration_since(now).unwrap_or(Duration::ZERO))
            }
            None => RateLimitWait::Unstated,
        })
    }
}

/// Splits `text` at the first empty line. The first part is the status line
/// and the headers, line by line. The second part is the body. A text with no
/// empty line has an empty body.
fn split_head(text: &str) -> (Vec<&str>, &str) {
    let mut head = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let (line, after) = rest.split_once('\n').unwrap_or((rest, ""));
        rest = after;
        if line.strip_suffix('\r').unwrap_or(line).is_empty() {
            return (head, rest);
        }
        head.push(line);
    }
    (head, "")
}

/// Splits the output of `gh api -i` into status, headers, and body.
///
/// Returns `None` when the first line is not an HTTP status line.
#[must_use]
pub fn parse_response(text: &str) -> Option<Response> {
    let (head, body) = split_head(text);
    let mut lines = head
        .iter()
        .map(|line| line.strip_suffix('\r').unwrap_or(line));
    let status_line = lines.next()?;
    let mut parts = status_line.splitn(3, ' ');
    if !parts.next()?.starts_with("HTTP/") {
        return None;
    }
    let status = parts.next()?.parse().ok()?;
    let reason = parts.next().unwrap_or_default().trim().to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    Some(Response {
        status,
        reason,
        headers,
        body: body.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The output of `gh api -i` for a `200`: a status line that ends in a line
    /// feed, headers that end in CRLF, an empty line, and the body.
    fn ok_output(body: &str) -> String {
        format!(
            "HTTP/2.0 200 OK\n\
             Etag: W/\"4c00158b\"\r\n\
             X-Ratelimit-Limit: 5000\r\n\
             X-Ratelimit-Remaining: 4866\r\n\
             X-Ratelimit-Reset: 1791977652\r\n\
             \r\n{body}"
        )
    }

    #[test]
    fn a_response_splits_into_status_headers_and_body() {
        let response = parse_response(&ok_output("{\"a\":1}")).expect("a response");
        assert_eq!(response.status(), 200);
        assert_eq!(response.reason(), "OK");
        assert_eq!(response.body(), "{\"a\":1}");
        assert_eq!(response.headers().len(), 4);
        assert_eq!(response.header("etag"), Some("W/\"4c00158b\""));
        assert_eq!(response.header("X-RATELIMIT-REMAINING"), Some("4866"));
        assert_eq!(response.header("x-ratelimit-reset"), Some("1791977652"));
        assert_eq!(response.header("missing"), None);
    }

    #[test]
    fn a_response_with_no_body_and_a_long_reason_parses() {
        let response =
            parse_response("HTTP/2.0 304 Not Modified\nEtag: \"x\"\r\n\r\n").expect("a response");
        assert_eq!(response.status(), 304);
        assert_eq!(response.reason(), "Not Modified");
        assert_eq!(response.body(), "");
        assert_eq!(response.header("etag"), Some("\"x\""));
    }

    #[test]
    fn a_body_keeps_its_own_line_breaks_and_colons() {
        let response = parse_response("HTTP/2.0 200 OK\r\nA: b\r\n\r\n{\"x\":\r\n\"y: z\"}\n")
            .expect("a response");
        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), "{\"x\":\r\n\"y: z\"}\n");
        assert_eq!(response.headers(), [("A".to_owned(), "b".to_owned())]);
    }

    #[test]
    fn output_with_no_status_line_gives_none() {
        assert_eq!(parse_response(""), None);
        assert_eq!(parse_response("not http\r\n\r\nbody"), None);
        assert_eq!(parse_response("HTTP/2.0 abc OK\n\n"), None);
        assert_eq!(parse_response("{\"message\":\"x\"}"), None);
    }

    /// A reply with this status line and these header lines.
    fn reply(status: &str, headers: &str) -> Response {
        parse_response(&format!("HTTP/2.0 {status}\n{headers}\r\n\r\n")).expect("a response")
    }

    /// The instant 1000 seconds after the epoch.
    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1000)
    }

    #[test]
    fn retry_after_gives_its_seconds_on_a_429() {
        let response = reply("429 Too Many Requests", "Retry-After: 30");
        assert_eq!(
            response.rate_limit_wait(now()),
            Some(RateLimitWait::Stated(Duration::from_secs(30)))
        );
    }

    #[test]
    fn retry_after_wins_over_the_reset_on_a_403() {
        let response = reply(
            "403 Forbidden",
            "Retry-After: 7\r\nX-Ratelimit-Remaining: 0\r\nX-Ratelimit-Reset: 1120",
        );
        assert_eq!(
            response.rate_limit_wait(now()),
            Some(RateLimitWait::Stated(Duration::from_secs(7)))
        );
    }

    #[test]
    fn a_retry_after_that_does_not_parse_is_unstated() {
        let response = reply(
            "403 Forbidden",
            "Retry-After: Wed, 21 Oct 2026 07:28:00 GMT",
        );
        assert_eq!(
            response.rate_limit_wait(now()),
            Some(RateLimitWait::Unstated)
        );
    }

    #[test]
    fn an_empty_quota_gives_the_time_to_the_reset() {
        let response = reply(
            "403 Forbidden",
            "X-Ratelimit-Remaining: 0\r\nX-Ratelimit-Reset: 1120",
        );
        assert_eq!(
            response.rate_limit_wait(now()),
            Some(RateLimitWait::Stated(Duration::from_secs(120)))
        );
    }

    #[test]
    fn a_reset_in_the_past_gives_a_wait_of_zero() {
        let response = reply(
            "429 Too Many Requests",
            "X-Ratelimit-Remaining: 0\r\nX-Ratelimit-Reset: 900",
        );
        assert_eq!(
            response.rate_limit_wait(now()),
            Some(RateLimitWait::Stated(Duration::ZERO))
        );
    }

    #[test]
    fn an_empty_quota_with_no_usable_reset_is_unstated() {
        for headers in [
            "X-Ratelimit-Remaining: 0",
            "X-Ratelimit-Remaining: 0\r\nX-Ratelimit-Reset: soon",
        ] {
            let response = reply("403 Forbidden", headers);
            assert_eq!(
                response.rate_limit_wait(now()),
                Some(RateLimitWait::Unstated),
                "{headers}"
            );
        }
    }

    #[test]
    fn a_403_with_quota_left_or_no_header_is_no_rate_limit() {
        for headers in [
            "X-Ratelimit-Remaining: 12\r\nX-Ratelimit-Reset: 1120",
            "X-Other: 1",
        ] {
            let response = reply("403 Forbidden", headers);
            assert_eq!(response.rate_limit_wait(now()), None, "{headers}");
        }
    }

    #[test]
    fn other_statuses_are_no_rate_limit() {
        for status in ["200 OK", "304 Not Modified", "500 Server Error"] {
            let response = reply(
                status,
                "Retry-After: 30\r\nX-Ratelimit-Remaining: 0\r\nX-Ratelimit-Reset: 1120",
            );
            assert_eq!(response.rate_limit_wait(now()), None, "{status}");
        }
    }
}
