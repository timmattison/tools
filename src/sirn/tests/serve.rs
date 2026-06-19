//! End-to-end integration tests for the `sirn` HTTP serving path.
//!
//! Each test binds its own server on `127.0.0.1:0` (an OS-assigned ephemeral
//! port) and uses a unique `tempfile::TempDir`, so the suite is parallel-safe: a
//! second concurrent copy (e.g. a `bacon` loop) cannot clobber a fixed port or
//! filename. The `start`/`stop`/`http_get` helpers live in the shared
//! [`common`] module so they can also back the `monitor` integration test.

mod common;
use common::{http_get, http_get_with_timeout, start, stop};
use std::time::Duration;

#[test]
fn existing_file_serves_bytes_content_type_and_length() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("hello.txt");
    let contents = b"hello, world\n";
    std::fs::write(&path, contents).expect("write hello.txt");

    let routes = sirn::build_routes(std::slice::from_ref(&path)).expect("routes build");
    let (addr, server, handles) = start(routes);

    let (status, headers, body) = http_get(addr, "/hello.txt");
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
fn large_multi_chunk_file_streams_correctly() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("big.bin");
    // Deterministic ~256 KiB payload, generated without any `as` cast.
    let data: Vec<u8> = (0_u8..=255).cycle().take(256 * 1024).collect();
    std::fs::write(&path, &data).expect("write big.bin");

    let routes = sirn::build_routes(std::slice::from_ref(&path)).expect("routes build");
    let (addr, server, handles) = start(routes);

    let (status, headers, body) = http_get(addr, "/big.bin");
    assert_eq!(status, 200);
    assert_eq!(body, data);
    assert_eq!(
        headers.get("content-length").map(String::as_str),
        Some(data.len().to_string().as_str())
    );

    stop(&server, handles);
}

#[test]
fn empty_file_serves_200_with_empty_body() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("empty.txt");
    std::fs::write(&path, b"").expect("write empty.txt");

    let routes = sirn::build_routes(std::slice::from_ref(&path)).expect("routes build");
    let (addr, server, handles) = start(routes);

    let (status, headers, body) = http_get(addr, "/empty.txt");
    assert_eq!(status, 200);
    assert!(body.is_empty(), "empty file should serve an empty body");
    assert_eq!(headers.get("content-length").map(String::as_str), Some("0"));

    stop(&server, handles);
}

#[test]
fn registered_route_with_missing_file_returns_404() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("gone.txt");
    // Create the file so it has a basename route, then remove it from disk.
    std::fs::write(&path, b"temporary").expect("write gone.txt");
    let routes = sirn::build_routes(std::slice::from_ref(&path)).expect("routes build");
    std::fs::remove_file(&path).expect("remove gone.txt");

    let (addr, server, handles) = start(routes);

    let (status, _headers, body) = http_get(addr, "/gone.txt");
    assert_eq!(status, 404);
    assert!(body.is_empty(), "404 should have an empty body");

    stop(&server, handles);
}

/// A files-mode route whose target is a DIRECTORY must return `404` and must not
/// hang. Before the directory guard existed, streaming a directory advertised its
/// metadata length and then never produced a body, hanging the client forever.
/// The timeout helper turns any regression here into a fast failure rather than a
/// suite-wide hang.
#[test]
fn route_to_a_directory_returns_404_without_hanging() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    // Routing a directory path produces `/<dirname>` -> the directory itself.
    let routes =
        sirn::build_routes(std::slice::from_ref(&dir.path().to_path_buf())).expect("routes build");
    let dir_name = dir
        .path()
        .file_name()
        .expect("temp dir has a basename")
        .to_string_lossy()
        .into_owned();

    let (addr, server, handles) = start(routes);

    let (status, _headers, body) =
        http_get_with_timeout(addr, &format!("/{dir_name}"), Duration::from_secs(5));
    assert_eq!(status, 404);
    assert!(body.is_empty(), "404 should have an empty body");

    stop(&server, handles);
}

#[test]
fn unregistered_route_returns_404() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("present.txt");
    std::fs::write(&path, b"present").expect("write present.txt");

    let routes = sirn::build_routes(std::slice::from_ref(&path)).expect("routes build");
    let (addr, server, handles) = start(routes);

    let (status, _headers, body) = http_get(addr, "/does-not-exist.txt");
    assert_eq!(status, 404);
    assert!(body.is_empty(), "404 should have an empty body");

    stop(&server, handles);
}
