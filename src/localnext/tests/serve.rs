//! End-to-end integration tests for the `localnext` serving path.
//!
//! Each test starts its own server on `127.0.0.1:0` (an OS-assigned ephemeral
//! port) rooted at a unique `tempfile::TempDir`, so the suite is parallel-safe: a
//! second concurrent copy cannot clobber a fixed port or path. The temp root is
//! canonicalized before serving because on macOS a `TempDir` lives under `/var`
//! (a symlink to `/private/var`), and path confinement compares canonical paths.

mod common;
use common::{http_get, http_get_with_timeout, start, stop};

use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

/// The export's shell page — what every unmatched route falls back to.
const INDEX_HTML: &[u8] = b"<!doctype html><title>home</title><div id=app></div>";

/// A stylesheet under `/static/`. Its content type is the defect this port fixes:
/// served as `text/plain` the browser refuses to apply it and the page renders
/// unstyled, with nothing in the console to explain why.
const APP_CSS: &[u8] = b"body{margin:0;font-family:system-ui}";

/// A route whose on-disk name holds multi-byte characters.
const CAFE_HTML: &[u8] = b"<!doctype html><title>caf\xc3\xa9</title>";

/// The bytes of the extensionless `about` file, distinct from `about.html`'s.
const ABOUT_EXACT: &[u8] = b"exact file, no extension";

/// The bytes of `about.html`, distinct from the extensionless `about` file's.
const ABOUT_HTML: &[u8] = b"<!doctype html><title>about</title>";

/// A read timeout generous enough for loopback yet short enough that a hung
/// response fails this one test instead of blocking the suite.
const HANG_TIMEOUT: Duration = Duration::from_secs(5);

/// Creates an empty export root: a fresh `TempDir` and its canonical path.
///
/// The `TempDir` is returned alongside the path because dropping it deletes the
/// tree; a test must hold it for as long as the server serves from it.
fn empty_export() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path().canonicalize().expect("canonicalize root");
    (dir, root)
}

/// Builds a realistic static-export tree in a fresh `TempDir`.
///
/// The tree holds the shell page, a stylesheet under `static/`, and a route whose
/// name is multi-byte. Deliberately absent are `about` and `about.html`, so the
/// resolution-order tests can add exactly the ones they mean to test.
fn export() -> (TempDir, PathBuf) {
    let (dir, root) = empty_export();
    write_file(&root.join("index.html"), INDEX_HTML);
    write_file(&root.join("static").join("app.css"), APP_CSS);
    write_file(&root.join("café.html"), CAFE_HTML);
    (dir, root)
}

/// Writes `contents` to `path`, creating every parent directory first.
fn write_file(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent directories");
    }
    std::fs::write(path, contents).expect("write file");
}

/// Reads the `Content-Type` of a response, for readability at the call sites.
fn content_type(headers: &std::collections::HashMap<String, String>) -> Option<&str> {
    headers.get("content-type").map(String::as_str)
}

#[test]
fn the_root_request_serves_index_html_as_html() {
    let (_dir, root) = export();
    let (addr, pool) = start(root);

    let (status, headers, body) = http_get(addr, "/");

    assert_eq!(status, 200);
    assert_eq!(body, INDEX_HTML);
    assert_eq!(content_type(&headers), Some("text/html; charset=utf-8"));

    stop(pool);
}

#[test]
fn an_exact_file_beats_the_html_file_of_the_same_name() {
    let (_dir, root) = export();
    write_file(&root.join("about"), ABOUT_EXACT);
    write_file(&root.join("about.html"), ABOUT_HTML);
    let (addr, pool) = start(root);

    let (status, _headers, body) = http_get(addr, "/about");

    assert_eq!(status, 200);
    assert_eq!(body, ABOUT_EXACT);

    stop(pool);
}

#[test]
fn the_html_file_serves_when_no_exact_file_exists() {
    let (_dir, root) = export();
    write_file(&root.join("about.html"), ABOUT_HTML);
    let (addr, pool) = start(root);

    let (status, headers, body) = http_get(addr, "/about");

    assert_eq!(status, 200);
    assert_eq!(body, ABOUT_HTML);
    assert_eq!(content_type(&headers), Some("text/html; charset=utf-8"));

    stop(pool);
}

#[test]
fn an_unmatched_route_falls_back_to_index_html_with_a_200() {
    let (_dir, root) = export();
    let (addr, pool) = start(root);

    let (status, headers, body) = http_get(addr, "/about");

    assert_eq!(status, 200);
    assert_eq!(body, INDEX_HTML);
    assert_eq!(content_type(&headers), Some("text/html; charset=utf-8"));

    stop(pool);
}

#[test]
fn a_static_stylesheet_serves_its_bytes_as_css() {
    let (_dir, root) = export();
    let (addr, pool) = start(root);

    let (status, headers, body) = http_get(addr, "/static/app.css");

    assert_eq!(status, 200);
    assert_eq!(body, APP_CSS);
    // The whole point of the content-type table: `tiny_http` sends no type of
    // its own, and a stylesheet served as anything but `text/css` is ignored.
    assert_eq!(content_type(&headers), Some("text/css; charset=utf-8"));
    assert_eq!(
        headers.get("content-length").map(String::as_str),
        Some(APP_CSS.len().to_string().as_str())
    );

    stop(pool);
}

#[test]
fn a_missing_static_asset_is_not_found_rather_than_the_fallback() {
    let (_dir, root) = export();
    let (addr, pool) = start(root);

    let (status, _headers, body) = http_get(addr, "/static/missing.css");

    assert_eq!(status, 404);
    assert_ne!(body, INDEX_HTML);

    stop(pool);
}

#[test]
fn a_traversal_attempt_is_forbidden_with_an_empty_body() {
    let (_dir, root) = export();
    let (addr, pool) = start(root);

    let (status, _headers, body) = http_get(addr, "/../../etc/passwd");

    assert_eq!(status, 403);
    assert!(body.is_empty(), "403 carries no body, got {body:?}");

    stop(pool);
}

#[test]
fn an_encoded_traversal_attempt_is_forbidden() {
    let (_dir, root) = export();
    let (addr, pool) = start(root);

    let (status, _headers, _body) = http_get(addr, "/%2e%2e%2fetc/passwd");

    assert_eq!(status, 403);

    stop(pool);
}

#[test]
fn a_double_encoded_traversal_never_leaves_the_root() {
    let (_dir, root) = export();
    let (addr, pool) = start(root);

    // Decoded exactly once, `%252e%252e%252f` becomes the literal text
    // `%2e%2e%2f` — an ordinary path component matching no file, never `../`.
    let (status, _headers, body) = http_get(addr, "/%252e%252e%252fetc/passwd");

    assert_eq!(status, 200);
    assert_eq!(body, INDEX_HTML);

    stop(pool);
}

#[test]
fn a_multi_byte_name_is_reachable_through_its_encoded_form() {
    let (_dir, root) = export();
    let (addr, pool) = start(root);

    let (status, headers, body) = http_get(addr, "/caf%C3%A9");

    assert_eq!(status, 200);
    assert_eq!(body, CAFE_HTML);
    assert_eq!(content_type(&headers), Some("text/html; charset=utf-8"));

    stop(pool);
}

#[test]
fn a_fallback_with_no_index_html_is_not_found_and_does_not_hang() {
    // The fallback path is the one path that is NOT canonicalized before it is
    // served, because `<root>/index.html` need not exist. A missing one must
    // 404: advertising a length and then producing no bytes would hang the
    // client forever, so the read timeout below is the actual assertion.
    let (_dir, root) = empty_export();
    let (addr, pool) = start(root);

    let (status, _headers, body) = http_get_with_timeout(addr, "/nothing/here", HANG_TIMEOUT);

    assert_eq!(status, 404);
    assert!(body.is_empty(), "404 carries no body, got {body:?}");

    stop(pool);
}

#[test]
fn a_static_directory_request_is_not_found_rather_than_a_listing() {
    let (_dir, root) = export();
    let (addr, pool) = start(root);

    let (status, _headers, body) = http_get_with_timeout(addr, "/static/", HANG_TIMEOUT);

    assert_eq!(status, 404);
    assert!(body.is_empty(), "404 carries no body, got {body:?}");

    stop(pool);
}
