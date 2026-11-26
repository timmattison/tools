//! Exercises the ambiguity in `tiny_http::Server::recv()`'s `Err` result: it
//! fires both for a deliberate `unblock()` and for a dead accept thread
//! reporting a real failure (an `accept()` error, then one pushed
//! `Message::Error`, then the accept loop exits). Both paths release exactly
//! one `recv()`-blocked worker per event, and both surface to that worker as
//! an `Err`. Nothing in the error's `Display` text or its `ErrorKind` tells
//! the two apart, so this suite never inspects either. `sirn::Pool` tells them
//! apart with a shutdown flag it owns: `Pool::shutdown` sets the flag before it
//! releases any worker, and `Pool::join` reports an error when a worker woke
//! to a flag that was still clear.
//!
//! A real accept failure (EMFILE, say) cannot be triggered safely in a test,
//! because exhausting the file-descriptor table could take down unrelated
//! work on the same machine. Calling `server.unblock()` exactly once, with no
//! deliberate shutdown behind it, reproduces the same event shape: one worker
//! released with an `Err`, and nothing in the pool's own state explains it.
//! `Pool::join` must report that as an `Err`.
//!
//! Every fixture is a unique `tempfile::TempDir`, every server binds
//! `127.0.0.1:0`, and every wait is bounded.

mod common;
use common::{http_get_with_timeout, stop, within_timeout};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// Workers for a pool that must show "one released, the rest still blocked".
/// At least two, so a single release leaves someone parked in `recv()`.
const WORKERS: usize = 3;

/// Bounds every HTTP read below.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The size of the file that a client hangs up on: far more than the socket
/// buffers hold, so the server is still writing when the client leaves.
const BIG_FILE_BYTES: usize = 8 * 1024 * 1024;

/// The known contents of the small file.
const SMALL_BYTES: &[u8] = b"still serving\n";

/// Binds a server on an ephemeral loopback port.
fn bind() -> Arc<tiny_http::Server> {
    Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"))
}

/// Writes `small.txt` into `dir` and routes it.
fn routes_for(dir: &TempDir) -> BTreeMap<String, std::path::PathBuf> {
    let small = dir.path().join("small.txt");
    std::fs::write(&small, SMALL_BYTES).expect("write small file");

    let mut routes = BTreeMap::new();
    routes.insert("/small.txt".to_string(), small);
    routes
}

/// The load-bearing red: one release with nothing accounting for it must
/// bring down every worker, not just the one that saw it.
#[test]
fn an_unexplained_release_brings_down_every_worker_and_surfaces() {
    let dir = TempDir::new().expect("temp dir");
    let server = bind();
    let mode = sirn::ServeMode::Files(Arc::new(routes_for(&dir)));
    let pool = sirn::serve(Arc::clone(&server), mode, WORKERS);

    server.unblock();

    let result = within_timeout(move || pool.join());

    assert!(
        result.is_err(),
        "an unexplained release should surface as a join() error, got Ok(())"
    );
}

/// The regression guard: a deliberate shutdown ends every worker.
#[test]
fn a_deliberate_shutdown_ends_every_worker_cleanly() {
    let dir = TempDir::new().expect("temp dir");
    let server = bind();
    let mode = sirn::ServeMode::Files(Arc::new(routes_for(&dir)));
    let pool = sirn::serve(server, mode, WORKERS);

    let result = within_timeout(move || pool.shutdown());

    assert!(
        result.is_ok(),
        "a deliberate shutdown should report no error, got {result:?}"
    );
}

/// A pool whose workers all panicked stops, but the accept thread still runs.
/// Thus the message that `sirn` prints must be true for that cause too: it
/// must not say that `sirn` stopped accepting connections.
///
/// `httpfile::Pool::join` waits for every worker. In a pool of more than
/// one worker, one panic ends only that worker, and the others keep serving.
/// This test builds a pool of one worker, thus its one panic ends every
/// worker of the pool.
#[test]
fn the_stop_message_is_true_when_a_worker_panicked() {
    let server = bind();
    let addr = server.server_addr().to_ip().expect("ip addr");
    let pool = httpfile::Pool::spawn(server, 1, |request| {
        request.respond(tiny_http::Response::empty(200))?;
        panic!("deliberate panic of the test handler, after the response");
    });

    let (status, _headers, _body) = http_get_with_timeout(addr, "/", READ_TIMEOUT);
    assert_eq!(status, 200, "the response did not go out");

    let result = within_timeout(move || sirn::wait_until_stopped(pool, "127.0.0.1", 1234));

    let message =
        result.expect_err("the one worker panicked, thus the pool must stop with an error");
    assert!(
        message.starts_with("sirn stopped serving on 127.0.0.1:1234: "),
        "the message must name the address and no single cause, got {message:?}"
    );
}

/// The regression guard: a client that leaves mid-response must not end the
/// worker. The pool has one worker, so the same worker serves the next request.
/// `tiny_http` itself turns a closed-connection error into `Ok`, so this test
/// guards the worker against a panic, not the `Err` path of the handler.
/// `a_handler_error_leaves_the_worker_serving` in `src/httpfile/tests/pool.rs`
/// covers that path: its handler returns `Err`, and the worker answers the
/// next request.
#[test]
fn a_client_that_hangs_up_mid_response_leaves_the_pool_serving() {
    let dir = TempDir::new().expect("temp dir");
    let mut routes = routes_for(&dir);
    let big = dir.path().join("big.bin");
    std::fs::write(&big, vec![0xAB_u8; BIG_FILE_BYTES]).expect("write big file");
    routes.insert("/big.bin".to_string(), big);
    let server = bind();
    let addr = server.server_addr().to_ip().expect("ip addr");
    let mode = sirn::ServeMode::Files(Arc::new(routes));
    let pool = sirn::serve(Arc::clone(&server), mode, 1);

    let mut client = TcpStream::connect(addr).expect("connect to server");
    client
        .set_read_timeout(Some(READ_TIMEOUT))
        .expect("set read timeout");
    client
        .write_all(b"GET /big.bin HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write request");
    let mut chunk = [0_u8; 1024];
    let read = client
        .read(&mut chunk)
        .expect("read the start of the response");
    assert!(read > 0, "the response did not start");
    drop(client);

    let (status, _headers, body) = http_get_with_timeout(addr, "/small.txt", READ_TIMEOUT);
    assert_eq!(status, 200, "the worker did not survive the IO error");
    assert_eq!(body, SMALL_BYTES);

    stop(pool);
}

/// The regression guard: a pool of zero workers still spawns one.
#[test]
fn zero_workers_still_spawns_one() {
    let dir = TempDir::new().expect("temp dir");
    let server = bind();
    let addr = server.server_addr().to_ip().expect("ip addr");
    let mode = sirn::ServeMode::Files(Arc::new(routes_for(&dir)));
    let pool = sirn::serve(Arc::clone(&server), mode, 0);

    let (status, _headers, body) = http_get_with_timeout(addr, "/small.txt", READ_TIMEOUT);
    assert_eq!(status, 200);
    assert_eq!(body, SMALL_BYTES);

    stop(pool);
}
