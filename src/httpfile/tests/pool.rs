//! Tests of `httpfile::Pool` that need no file server: the handler is a
//! closure, so each test states the behavior of the pool alone.
//!
//! A real accept failure (EMFILE, for example) cannot be made safely in a
//! test, because a full file-descriptor table can stop other work on the same
//! machine. One `server.unblock()` with no `Pool::shutdown` behind it makes
//! the same event shape: `tiny_http` releases one worker with an `Err`, and
//! the state of the pool does not explain it. The pool must treat that
//! release as a failure.
//!
//! Every server binds `127.0.0.1:0`, and every wait is bounded.

mod common;
use common::within_timeout;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Bounds every read below, so a pool that stops answering fails the test.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The workers of a pool that must show "one released, the others still
/// blocked". At least two, so one release leaves a worker in `recv()`.
const WORKERS: usize = 3;

/// Binds a server on an ephemeral loopback port.
fn bind() -> Arc<tiny_http::Server> {
    Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"))
}

/// Answers each request with an empty `200`.
fn answer_ok(request: tiny_http::Request) -> std::io::Result<()> {
    request.respond(tiny_http::Response::empty(200))
}

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

    within_timeout(move || pool.shutdown()).expect("pool shuts down cleanly");
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

    within_timeout(move || pool.shutdown()).expect("pool shuts down cleanly");

    assert_eq!(Arc::strong_count(&witness), 1, "the handler is still alive");
}

/// One release that no shutdown asked for ends every worker, not only the
/// worker that got the `Err`, and `join` reports it as a failure.
#[test]
fn an_unexplained_release_ends_every_worker_and_join_reports_it() {
    let server = bind();
    let pool = httpfile::Pool::spawn(Arc::clone(&server), WORKERS, answer_ok);

    server.unblock();

    let result = within_timeout(move || pool.join());

    assert!(
        result.is_err(),
        "an unexplained release must make join() return Err, got Ok(())"
    );
}

/// A deliberate shutdown ends every worker and reports no error.
#[test]
fn a_deliberate_shutdown_ends_every_worker_and_reports_no_error() {
    let pool = httpfile::Pool::spawn(bind(), WORKERS, answer_ok);

    let result = within_timeout(move || pool.shutdown());

    assert!(
        result.is_ok(),
        "a deliberate shutdown must return Ok(()), got {result:?}"
    );
}

/// A handler that panics ends its worker, and the shutdown reports the panic.
/// The response goes out before the panic, so the client sees no failure, and
/// the result of the shutdown is the only place where the panic can show.
#[test]
fn a_worker_that_panics_makes_the_shutdown_report_an_error() {
    let server = bind();
    let addr = server.server_addr().to_ip().expect("ip addr");
    let pool = httpfile::Pool::spawn(server, WORKERS, |request| {
        request.respond(tiny_http::Response::empty(200))?;
        panic!("deliberate panic of the test handler, after the response");
    });

    assert_eq!(get_status(addr), 200, "the response did not go out");

    let result = within_timeout(move || pool.shutdown());

    assert!(
        result.is_err(),
        "a worker that panicked must make shutdown() return Err, got Ok(())"
    );
}

/// A pool of zero workers still spawns one, and that worker answers.
#[test]
fn zero_workers_still_spawns_one() {
    let server = bind();
    let addr = server.server_addr().to_ip().expect("ip addr");
    let pool = httpfile::Pool::spawn(server, 0, answer_ok);

    assert_eq!(get_status(addr), 200, "no worker answered the request");

    within_timeout(move || pool.shutdown()).expect("pool shuts down cleanly");
}
