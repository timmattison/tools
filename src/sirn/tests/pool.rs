//! Exercises the ambiguity in `tiny_http::Server::recv()`'s `Err` result: it
//! fires both for a deliberate `unblock()` and for a dead accept thread
//! reporting a real failure (an `accept()` error, then one pushed
//! `Message::Error`, then the accept loop exits). Both paths release exactly
//! one `recv()`-blocked worker per event, and both surface to that worker as
//! an `Err`. Nothing in the error's `Display` text or its `ErrorKind` tells
//! the two apart, so this suite never inspects either.
//!
//! A real accept failure (EMFILE, say) cannot be triggered safely in a test,
//! because exhausting the file-descriptor table could take down unrelated
//! work on the same machine. Calling `server.unblock()` exactly once, with no
//! deliberate shutdown behind it, reproduces the same event shape: one worker
//! released with an `Err`, and nothing in the pool's own state explains it.
//!
//! Every fixture is a unique `tempfile::TempDir`, every server binds
//! `127.0.0.1:0`, and every wait is bounded.

mod common;
use common::{http_get_with_timeout, stop};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tempfile::TempDir;

/// Workers for a pool that must show "one released, the rest still blocked".
/// At least two, so a single release leaves someone parked in `recv()`.
const WORKERS: usize = 3;

/// Bounds every join below. A pool that never finishes releasing its workers
/// is the defect under test, so the wait must fail the test, not hang it.
const JOIN_TIMEOUT: Duration = Duration::from_secs(5);

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

/// Joins every handle on its own thread and reports whether that finished
/// within [`JOIN_TIMEOUT`].
fn joined_in_time(handles: Vec<JoinHandle<()>>) -> bool {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for handle in handles {
            let _ = handle.join();
        }
        let _ = sender.send(());
    });
    receiver.recv_timeout(JOIN_TIMEOUT).is_ok()
}

/// Writes `big.bin` and `small.txt` into `dir` and routes both.
fn routes_for(dir: &TempDir) -> BTreeMap<String, std::path::PathBuf> {
    let big = dir.path().join("big.bin");
    std::fs::write(&big, vec![0xAB_u8; BIG_FILE_BYTES]).expect("write big file");
    let small = dir.path().join("small.txt");
    std::fs::write(&small, SMALL_BYTES).expect("write small file");

    let mut routes = BTreeMap::new();
    routes.insert("/big.bin".to_string(), big);
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
    let handles = sirn::serve(Arc::clone(&server), mode, WORKERS);

    server.unblock();

    assert!(
        joined_in_time(handles),
        "an unexplained release left workers blocked in recv() for {JOIN_TIMEOUT:?}"
    );
}

/// The regression guard: a deliberate shutdown ends every worker.
#[test]
fn a_deliberate_shutdown_ends_every_worker_cleanly() {
    let dir = TempDir::new().expect("temp dir");
    let server = bind();
    let mode = sirn::ServeMode::Files(Arc::new(routes_for(&dir)));
    let handles = sirn::serve(Arc::clone(&server), mode, WORKERS);

    for _ in 0..WORKERS {
        server.unblock();
    }

    assert!(
        joined_in_time(handles),
        "a deliberate shutdown left workers running for {JOIN_TIMEOUT:?}"
    );
}

/// The regression guard: a client that leaves mid-response must not end the
/// worker. The pool has one worker, so the same worker serves the next request.
/// `tiny_http` itself turns a closed-connection error into `Ok`, so this test
/// guards the worker against a panic, not the `Err` path of the handler.
#[test]
fn a_client_that_hangs_up_mid_response_leaves_the_pool_serving() {
    let dir = TempDir::new().expect("temp dir");
    let server = bind();
    let addr = server.server_addr().to_ip().expect("ip addr");
    let mode = sirn::ServeMode::Files(Arc::new(routes_for(&dir)));
    let handles = sirn::serve(Arc::clone(&server), mode, 1);

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

    stop(&server, handles);
}

/// The regression guard: a pool of zero workers still spawns one.
#[test]
fn zero_workers_still_spawns_one() {
    let dir = TempDir::new().expect("temp dir");
    let server = bind();
    let addr = server.server_addr().to_ip().expect("ip addr");
    let mode = sirn::ServeMode::Files(Arc::new(routes_for(&dir)));
    let handles = sirn::serve(Arc::clone(&server), mode, 0);

    let (status, _headers, body) = http_get_with_timeout(addr, "/small.txt", READ_TIMEOUT);
    assert_eq!(status, 200);
    assert_eq!(body, SMALL_BYTES);

    stop(&server, handles);
}
