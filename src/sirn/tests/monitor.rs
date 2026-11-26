//! Integration test for the files-mode availability monitor: a file appearing
//! and disappearing while the server runs is reflected in fetch results, and the
//! monitor coexists with the worker pool and shuts down cleanly.
//!
//! Parallel-safe: a unique `tempfile::TempDir` and an OS-assigned `127.0.0.1:0`
//! port, so a concurrent second copy (e.g. a `bacon` loop) can't collide.

mod common;
use common::{http_get, stop, JOIN_TIMEOUT};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::Duration;

#[test]
fn file_created_then_deleted_is_reflected_in_fetches() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("late.txt");

    let mut routes = BTreeMap::new();
    routes.insert("/late.txt".to_string(), path.clone());
    let routes = Arc::new(routes);

    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));
    let addr = server.server_addr().to_ip().expect("ip addr");
    let pool = sirn::serve(server, sirn::ServeMode::Files(Arc::clone(&routes)), 2);

    let monitor = sirn::spawn_monitor(Arc::clone(&routes));

    // Absent at startup -> 404.
    let (status, _h, _b) = http_get(addr, "/late.txt");
    assert_eq!(status, 404, "missing file should 404");

    // Created after startup -> 200 with exact bytes.
    let contents = b"appeared\n";
    std::fs::write(&path, contents).expect("create late.txt");
    let (status, _h, body) = http_get(addr, "/late.txt");
    assert_eq!(status, 200, "created file should serve 200");
    assert_eq!(body, contents, "served bytes must match");

    // Deleted -> 404 again.
    std::fs::remove_file(&path).expect("remove late.txt");
    let (status, _h, _b) = http_get(addr, "/late.txt");
    assert_eq!(status, 404, "deleted file should 404");

    // Clean teardown: stop the monitor and the server.
    drop(monitor);
    stop(pool);
}

#[test]
fn monitor_joins_promptly_after_shutdown_signal() {
    // Even with no served files, stopping the monitor must end the poll loop so
    // the stop returns rather than blocking for a poll interval.
    let routes = Arc::new(BTreeMap::new());
    let monitor = sirn::spawn_monitor(routes);
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        drop(monitor);
        let _ = done_tx.send(());
    });
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("monitor did not stop within 5 seconds of the stop signal");
}

/// Workers for the pool in the failure test: at least two, so one release
/// with nothing behind it leaves the others to the pool's own release.
const WORKERS: usize = 3;

/// Mirrors the end of `main`: the monitor lives in a local, and the pool's
/// error leaves through `?`.
fn serve_until_the_pool_ends(
    server: Arc<tiny_http::Server>,
    routes: Arc<BTreeMap<String, PathBuf>>,
) -> std::io::Result<()> {
    let _monitor = sirn::spawn_monitor(Arc::clone(&routes));
    sirn::serve(server, sirn::ServeMode::Files(routes), WORKERS).join()?;
    Ok(())
}

#[test]
fn a_pool_that_ends_on_a_failure_still_stops_and_joins_the_monitor() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("a.txt");
    std::fs::write(&path, b"alpha").expect("write a.txt");

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut map = BTreeMap::new();
        map.insert("/a.txt".to_string(), path);
        let routes = Arc::new(map);
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));
        // One release with no shutdown behind it is the event shape of a dead
        // accept thread (see `tests/pool.rs`). tiny_http queues the release, so
        // a worker sees it when it reaches `recv()`.
        server.unblock();
        let result = serve_until_the_pool_ends(server, Arc::clone(&routes));
        // Read the count on this thread, at once after the run returns. A
        // monitor thread that is still alive holds a clone of `routes`.
        let _ = sender.send((result, Arc::strong_count(&routes)));
    });

    let (result, count) = receiver
        .recv_timeout(JOIN_TIMEOUT)
        .unwrap_or_else(|_| panic!("the run did not end within {JOIN_TIMEOUT:?}"));
    assert!(
        result.is_err(),
        "the failure did not reach the caller, got {result:?}"
    );
    assert_eq!(count, 1, "the monitor thread outlived the end of the pool");
    drop(dir);
}
