//! End-to-end integration tests for the `sirn` HTTP serving path.
//!
//! Each test binds its own server on `127.0.0.1:0` (an OS-assigned ephemeral
//! port) and uses a unique `tempfile::TempDir`, so the suite is parallel-safe: a
//! second concurrent copy (e.g. a `bacon` loop) cannot clobber a fixed port or
//! filename. A raw HTTP/1.0 client over `std::net::TcpStream` exercises the
//! server with no async/reqwest dependency.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;

/// Starts a server bound to an ephemeral loopback port serving `routes`.
///
/// Returns the bound address, the server handle (so the caller can `unblock()`
/// it for shutdown), and the worker-thread join handles.
fn start(
    routes: BTreeMap<String, PathBuf>,
) -> (SocketAddr, Arc<tiny_http::Server>, Vec<JoinHandle<()>>) {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));
    let addr = server.server_addr().to_ip().expect("ip addr");
    let handles = sirn::serve(Arc::clone(&server), Arc::new(routes), 2);
    (addr, server, handles)
}

/// Unblocks `server` and joins every worker so threads don't linger after a test.
///
/// `tiny_http::Server::unblock` releases exactly one `recv()`-blocked thread per
/// call, so it must be invoked once per worker for the whole pool to exit.
fn stop(server: &Arc<tiny_http::Server>, handles: Vec<JoinHandle<()>>) {
    for _ in &handles {
        server.unblock();
    }
    for handle in handles {
        handle.join().expect("worker thread joins cleanly");
    }
}

/// Issues `GET <path> HTTP/1.0` with `Connection: close`, reads to EOF, and
/// returns `(status_code, headers_lowercased, body_bytes)`.
///
/// HTTP/1.0 + `Connection: close` means the server closes the socket after the
/// response, so EOF delimits the body. Header keys are lowercased and values
/// trimmed for case-insensitive lookup.
fn http_get(addr: SocketAddr, path: &str) -> (u16, HashMap<String, String>, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).expect("connect to server");
    let request = format!("GET {path} HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush request");

    let mut raw = Vec::new();
    let mut buf = [0_u8; 8192];
    loop {
        let n = stream.read(&mut buf).expect("read response");
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }

    let separator = b"\r\n\r\n";
    let head_end = raw
        .windows(separator.len())
        .position(|window| window == separator)
        .expect("response has a header/body separator");
    let head = &raw[..head_end];
    let body = raw[head_end + separator.len()..].to_vec();

    let head_text = String::from_utf8_lossy(head);
    let mut lines = head_text.lines();
    let status_line = lines.next().expect("status line present");
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .expect("status code token")
        .parse::<u16>()
        .expect("status code parses");

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    (status_code, headers, body)
}

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
