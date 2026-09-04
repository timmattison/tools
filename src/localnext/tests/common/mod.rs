#![allow(
    dead_code,
    reason = "shared test helpers: each test binary compiles this module independently and may not use every helper"
)]
//! Shared HTTP helpers for the `localnext` integration tests.
//!
//! This lives in a `tests/` SUBDIRECTORY, so it is a shared module compiled into
//! each test binary that `mod common;`s it — not a test binary of its own. Each
//! binary compiles this module independently and may not use every helper, so the
//! top-level `#![allow(dead_code)]` keeps `-D warnings` happy.
//!
//! A raw HTTP/1.0 client over `std::net::TcpStream` exercises the server with no
//! async runtime and no `reqwest`. Callers bind on `127.0.0.1:0` (an OS-assigned
//! ephemeral port) and use a unique `tempfile::TempDir`, so the suite is
//! parallel-safe: a second concurrent copy (a `bacon` loop, the pre-commit hook
//! racing a hand-run `cargo test`) cannot clobber a fixed port or filename.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

/// How many worker threads each test server runs.
const WORKERS: usize = 2;

/// Starts a server bound to an ephemeral loopback port serving the export at
/// `root`.
///
/// `root` must already be canonical — path confinement compares canonical paths,
/// and on macOS a `TempDir` lives under `/var`, a symlink to `/private/var`.
///
/// Returns the bound address and the [`localnext::Pool`] serving it; pass the
/// pool to [`stop`] when the test is done with it.
pub fn start(root: PathBuf) -> (SocketAddr, localnext::Pool) {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));
    let addr = server.server_addr().to_ip().expect("ip addr");
    let pool = localnext::serve(server, Arc::new(root), WORKERS);
    (addr, pool)
}

/// Ends `pool` deliberately and joins every worker, so threads don't linger
/// after a test.
///
/// Asserts that shutdown reported no error: a broken shutdown path should fail
/// the test that exercises it, not leak a running server into the rest of the
/// session.
pub fn stop(pool: localnext::Pool) {
    pool.shutdown().expect("worker pool shuts down cleanly");
}

/// Issues `GET <path> HTTP/1.0` with `Connection: close`, reads to EOF, and
/// returns `(status_code, headers_lowercased, body_bytes)`.
///
/// HTTP/1.0 + `Connection: close` means the server closes the socket after the
/// response, so EOF delimits the body. Header keys are lowercased and values
/// trimmed for case-insensitive lookup.
pub fn http_get(addr: SocketAddr, path: &str) -> (u16, HashMap<String, String>, Vec<u8>) {
    read_response(connect_and_send(addr, path, None))
}

/// Like [`http_get`] but with a read timeout, so a server that hangs mid-response
/// fails the test promptly instead of blocking the whole suite forever.
///
/// A response that advertises a `Content-Length` it never delivers — what
/// streaming a directory would do — never reaches EOF. On timeout the read below
/// panics with a `TimedOut` error, which fails the test fast.
pub fn http_get_with_timeout(
    addr: SocketAddr,
    path: &str,
    timeout: std::time::Duration,
) -> (u16, HashMap<String, String>, Vec<u8>) {
    read_response(connect_and_send(addr, path, Some(timeout)))
}

/// Connects to `addr` and writes one HTTP/1.0 request for `path`, applying
/// `timeout` as the read timeout when given.
fn connect_and_send(
    addr: SocketAddr,
    path: &str,
    timeout: Option<std::time::Duration>,
) -> TcpStream {
    let mut stream = TcpStream::connect(addr).expect("connect to server");
    stream.set_read_timeout(timeout).expect("set read timeout");
    let request = format!("GET {path} HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush request");
    stream
}

/// Reads `stream` to EOF and parses the response it carried.
fn read_response(mut stream: TcpStream) -> (u16, HashMap<String, String>, Vec<u8>) {
    let mut raw = Vec::new();
    let mut buf = [0_u8; 8192];
    loop {
        let n = stream.read(&mut buf).expect("read response");
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }

    parse_response(&raw)
}

/// Parses a raw HTTP/1.0 response into `(status_code, headers_lowercased, body)`.
///
/// Header keys are lowercased and values trimmed for case-insensitive lookup.
fn parse_response(raw: &[u8]) -> (u16, HashMap<String, String>, Vec<u8>) {
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
