//! End-to-end integration tests for the `sirn` directory-serving path.
//!
//! Each test starts its own directory-mode server on `127.0.0.1:0` (an
//! OS-assigned ephemeral port) rooted at a unique `tempfile::TempDir`, so the
//! suite is parallel-safe: a second concurrent copy (e.g. a `bacon` loop) cannot
//! clobber a fixed port or path. The temp root is canonicalized before serving
//! because on macOS `TempDir` lives under `/var` (a symlink to `/private/var`),
//! and directory-mode path confinement compares against the canonical root.

mod common;
use common::{http_get, start_dir, stop};

#[test]
fn root_request_returns_listing_with_entries() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("a.txt"), b"alpha").expect("write a.txt");
    std::fs::create_dir(dir.path().join("sub")).expect("create sub dir");
    let root = dir.path().canonicalize().expect("canonicalize root");

    let (addr, server, handles) = start_dir(root);

    let (status, headers, body) = http_get(addr, "/");
    assert_eq!(status, 200);
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("text/html; charset=utf-8")
    );
    let html = String::from_utf8_lossy(&body);
    assert!(
        html.contains("a.txt"),
        "listing should include the file entry, got:\n{html}"
    );
    assert!(
        html.contains("sub"),
        "listing should include the subdirectory entry, got:\n{html}"
    );

    stop(&server, handles);
}

#[test]
fn nested_file_fetch_returns_bytes_and_headers() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let contents = b"alpha";
    std::fs::write(dir.path().join("a.txt"), contents).expect("write a.txt");
    let root = dir.path().canonicalize().expect("canonicalize root");

    let (addr, server, handles) = start_dir(root);

    let (status, headers, body) = http_get(addr, "/a.txt");
    assert_eq!(status, 200);
    assert_eq!(body, contents);
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        headers.get("content-length").map(String::as_str),
        Some(body.len().to_string().as_str())
    );

    stop(&server, handles);
}

#[test]
fn subdir_file_fetch_works() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).expect("create sub dir");
    std::fs::write(sub.join("inner.txt"), b"deep").expect("write inner.txt");
    let root = dir.path().canonicalize().expect("canonicalize root");

    let (addr, server, handles) = start_dir(root);

    let (status, _headers, body) = http_get(addr, "/sub/inner.txt");
    assert_eq!(status, 200);
    assert_eq!(body, b"deep");

    stop(&server, handles);
}

#[test]
fn parent_traversal_is_forbidden() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let root = dir.path().canonicalize().expect("canonicalize root");

    let (addr, server, handles) = start_dir(root);

    let (status, _headers, _body) = http_get(addr, "/../../etc/passwd");
    assert_eq!(status, 403);

    stop(&server, handles);
}

#[test]
fn missing_path_returns_404() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let root = dir.path().canonicalize().expect("canonicalize root");

    let (addr, server, handles) = start_dir(root);

    let (status, _headers, _body) = http_get(addr, "/nope.txt");
    assert_eq!(status, 404);

    stop(&server, handles);
}
