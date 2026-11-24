//! The cache entry of a read that GitHub answered with an `ETag`.

use crate::Response;

/// The name of the request header that makes GitHub answer `304` when the
/// page did not change.
const IF_NONE_MATCH: &str = "If-None-Match";

/// One answer of GitHub that a caller keeps for the next read of the same path.
///
/// The entry keeps the path of the request that gave the answer, as well as the
/// `ETag` and the body. A caller can key its cache by something coarser than
/// the path, for example by a repository and a page number, when the path
/// holds a value that changes, such as a start time. The cache then keeps one
/// entry for each key and not one entry for each value of the path. The `ETag`
/// goes out only when the path of the new request is the same as the kept path,
/// thus an `ETag` never meets a request for another page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EtagEntry {
    path: String,
    etag: String,
    body: String,
}

impl EtagEntry {
    /// Keeps the answer to a request for `path`.
    ///
    /// Returns `None` when the answer has no `ETag` header, because such an
    /// answer cannot be asked for again.
    #[must_use]
    pub fn from_response(path: &str, response: Response) -> Option<EtagEntry> {
        let etag = response.header("etag")?.to_owned();
        Some(EtagEntry {
            path: path.to_owned(),
            etag,
            body: response.into_body(),
        })
    }

    /// The whole request header line for a read of `path`, for example
    /// `If-None-Match: W/"4c00158b"`.
    ///
    /// Returns `None` when the kept path is not `path`.
    #[must_use]
    pub fn condition_for(&self, path: &str) -> Option<String> {
        (self.path == path).then(|| format!("{IF_NONE_MATCH}: {}", self.etag))
    }

    /// The kept body, for the `304` answer to a read of `path`.
    ///
    /// Returns `None` when the kept path is not `path`.
    #[must_use]
    pub fn body_for(&self, path: &str) -> Option<&str> {
        (self.path == path).then_some(self.body.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_response;

    const PATH: &str = "repos/o/r/actions/runs?page=1&created=>=2026-11-10T10";

    fn answer(headers: &str, body: &str) -> Response {
        parse_response(&format!("HTTP/2.0 200 OK\n{headers}\r\n\r\n{body}")).unwrap()
    }

    fn entry() -> EtagEntry {
        EtagEntry::from_response(PATH, answer("Etag: W/\"abc\"", "{\"a\":1}")).unwrap()
    }

    #[test]
    fn an_answer_with_an_etag_gives_an_entry() {
        assert!(EtagEntry::from_response(PATH, answer("Etag: \"x\"", "{}")).is_some());
    }

    #[test]
    fn an_answer_with_no_etag_gives_no_entry() {
        assert_eq!(
            EtagEntry::from_response(PATH, answer("X-Other: 1", "{}")),
            None
        );
    }

    #[test]
    fn the_condition_is_a_whole_header_line_for_the_same_path() {
        assert_eq!(
            entry().condition_for(PATH).as_deref(),
            Some("If-None-Match: W/\"abc\"")
        );
    }

    #[test]
    fn the_condition_is_none_for_another_path() {
        assert_eq!(entry().condition_for("repos/o/r/actions/runs?page=2"), None);
    }

    #[test]
    fn the_body_is_kept_for_the_same_path() {
        assert_eq!(entry().body_for(PATH), Some("{\"a\":1}"));
    }

    #[test]
    fn the_body_is_none_for_another_path() {
        assert_eq!(entry().body_for("repos/o/r/actions/runs?page=2"), None);
    }

    #[test]
    fn a_path_that_differs_only_in_case_is_another_path() {
        assert_eq!(entry().condition_for(&PATH.to_uppercase()), None);
    }
}
