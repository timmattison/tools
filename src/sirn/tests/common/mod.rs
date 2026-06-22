#![allow(
    dead_code,
    reason = "shared test helpers: each test binary compiles this module independently and may not use every helper"
)]
//! Shared HTTP helpers for the `sirn` integration tests.
//!
//! This lives in a `tests/` SUBDIRECTORY, so it is a shared module compiled into
//! each test binary that `mod common;`s it — not a test binary of its own. Each
//! binary compiles this module independently and may not use every helper, so the
//! top-level `#![allow(dead_code)]` keeps `-D warnings` happy.
//!
//! A raw HTTP/1.0 client over `std::net::TcpStream` exercises the server with no
//! async/reqwest dependency. Callers bind on `127.0.0.1:0` (OS-assigned ephemeral
//! port) and use a unique `tempfile::TempDir`, so the suite is parallel-safe: a
//! second concurrent copy (e.g. a `bacon` loop) cannot clobber a fixed port or
//! filename.

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
pub fn start(
    routes: BTreeMap<String, PathBuf>,
) -> (SocketAddr, Arc<tiny_http::Server>, Vec<JoinHandle<()>>) {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));
    let addr = server.server_addr().to_ip().expect("ip addr");
    let handles = sirn::serve(
        Arc::clone(&server),
        sirn::ServeMode::Files(Arc::new(routes)),
        2,
    );
    (addr, server, handles)
}

/// Starts a directory-mode server rooted at `root` (caller canonicalizes it).
pub fn start_dir(root: PathBuf) -> (SocketAddr, Arc<tiny_http::Server>, Vec<JoinHandle<()>>) {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));
    let addr = server.server_addr().to_ip().expect("ip addr");
    let handles = sirn::serve(
        Arc::clone(&server),
        sirn::ServeMode::Directory(Arc::new(root)),
        2,
    );
    (addr, server, handles)
}

/// Unblocks `server` and joins every worker so threads don't linger after a test.
///
/// `tiny_http::Server::unblock` releases exactly one `recv()`-blocked thread per
/// call, so it must be invoked once per worker for the whole pool to exit.
pub fn stop(server: &Arc<tiny_http::Server>, handles: Vec<JoinHandle<()>>) {
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
pub fn http_get(addr: SocketAddr, path: &str) -> (u16, HashMap<String, String>, Vec<u8>) {
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

    parse_response(&raw)
}

/// Like [`http_get`] but with a read timeout, so a server that hangs mid-response
/// fails the test promptly instead of blocking forever. Panics (failing the test)
/// if the read times out.
pub fn http_get_with_timeout(
    addr: SocketAddr,
    path: &str,
    timeout: std::time::Duration,
) -> (u16, HashMap<String, String>, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).expect("connect to server");
    // A hung server never sends EOF, so bound the read: on timeout the
    // `.expect("read response")` below panics with a TimedOut error and the test
    // fails fast instead of blocking the whole suite forever.
    stream
        .set_read_timeout(Some(timeout))
        .expect("set read timeout");
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
