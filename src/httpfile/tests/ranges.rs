//! Integration tests for `httpfile::serve_file`'s HTTP byte-range support.
//!
//! A raw HTTP/1.0 client over `std::net::TcpStream` exercises the server with no
//! async runtime and no `reqwest`, mirroring the shape of
//! `src/localnext/tests/common/mod.rs`. Every server binds `127.0.0.1:0` (an
//! OS-assigned ephemeral port) and every fixture is a fresh `tempfile::TempDir`,
//! so a second concurrent `cargo test` cannot collide with this one (see
//! `CLAUDE.md`'s parallel-safety rule).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use tempfile::TempDir;

/// How long a client waits for a response before treating it as hung.
///
/// A response that advertises a `Content-Length` it never delivers never
/// reaches EOF; this bounds that wait so a bug fails the test promptly instead
/// of hanging the whole suite forever.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Starts a server on an ephemeral loopback port that answers every request by
/// streaming `file` through [`httpfile::serve_file`].
///
/// Returns the bound address, the server handle (to `unblock()` it for
/// shutdown), and the worker thread's join handle.
fn start(file: PathBuf) -> (SocketAddr, Arc<tiny_http::Server>, JoinHandle<()>) {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));
    let addr = server.server_addr().to_ip().expect("ip addr");
    let handle = {
        let server = Arc::clone(&server);
        std::thread::spawn(move || {
            // `recv()` errors when the server is unblocked, ending the loop.
            while let Ok(request) = server.recv() {
                let _ = httpfile::serve_file(&file, request);
            }
        })
    };
    (addr, server, handle)
}

/// Unblocks `server` and joins its worker so the thread never outlives the test.
fn stop(server: &Arc<tiny_http::Server>, handle: JoinHandle<()>) {
    server.unblock();
    handle.join().expect("worker thread joins cleanly");
}

/// Issues `GET / HTTP/1.0` against `addr`, optionally carrying one extra header
/// line (e.g. `("Range", "bytes=2-5")`), and returns
/// `(status_code, headers_lowercased, body_bytes)`.
///
/// HTTP/1.0 + `Connection: close` means the server closes the socket after the
/// response, so EOF delimits the body. Header keys are lowercased and values
/// trimmed for case-insensitive lookup.
fn get(
    addr: SocketAddr,
    extra_header: Option<(&str, &str)>,
) -> (u16, HashMap<String, String>, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).expect("connect to server");
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .expect("set read timeout");

    let mut request = String::from("GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n");
    if let Some((name, value)) = extra_header {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");

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

/// Writes a 10-byte fixture file (`"0123456789"`) into a fresh temp dir and
/// returns the dir (kept alive for the caller, since dropping it deletes the
/// file) and the file's path.
fn ten_byte_fixture() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("clip.bin");
    std::fs::write(&path, b"0123456789").expect("write fixture");
    (dir, path)
}

#[test]
fn a_range_request_returns_206_and_only_the_bytes_it_asked_for() {
    let (_dir, file) = ten_byte_fixture();
    let (addr, server, handle) = start(file);

    let (status, headers, body) = get(addr, Some(("Range", "bytes=2-5")));

    stop(&server, handle);

    assert_eq!(status, 206);
    assert_eq!(
        headers.get("content-range").map(String::as_str),
        Some("bytes 2-5/10")
    );
    assert_eq!(body, b"2345");
}
