//! Tests of `httpfile::Pool` that need no file server: the handler is a
//! closure, so each test states the behavior of the pool alone.
//!
//! Every server binds `127.0.0.1:0`, and every wait is bounded.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Bounds every read below, so a pool that stops answering fails the test.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Sends one `GET /` and returns the status code.
fn get_status(addr: std::net::SocketAddr) -> u16 {
    let mut stream = TcpStream::connect(addr).expect("connect to server");
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .expect("set read timeout");
    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write request");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read response");
    raw.split_whitespace()
        .nth(1)
        .expect("status code token")
        .parse()
        .expect("status code parses")
}

/// An `Err` from the handler is swallowed: the one worker answers the next
/// request, and the pool still ends cleanly.
#[test]
fn a_handler_error_leaves_the_worker_serving() {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));
    let addr = server.server_addr().to_ip().expect("ip addr");
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let pool = httpfile::Pool::spawn(server, 1, move |request| {
        let call = seen.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            // Fail on the first request, after answering it.
            request.respond(tiny_http::Response::empty(200))?;
            return Err(std::io::Error::other("handler failure"));
        }
        request.respond(tiny_http::Response::empty(204))
    });

    assert_eq!(get_status(addr), 200);
    assert_eq!(get_status(addr), 204, "the worker did not survive the Err");
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    pool.shutdown().expect("pool shuts down cleanly");
}

/// The pool keeps no reference to the handler after the workers end.
#[test]
fn the_handler_is_released_when_the_pool_ends() {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));
    let witness = Arc::new(());
    let held = Arc::clone(&witness);
    let pool = httpfile::Pool::spawn(server, 2, move |request| {
        let _held = &held;
        request.respond(tiny_http::Response::empty(200))
    });

    pool.shutdown().expect("pool shuts down cleanly");

    assert_eq!(Arc::strong_count(&witness), 1, "the handler is still alive");
}
