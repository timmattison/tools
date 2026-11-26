//! Integration test for the files-mode availability monitor: a file appearing
//! and disappearing while the server runs is reflected in fetch results, and the
//! monitor coexists with the worker pool and shuts down cleanly.
//!
//! Parallel-safe: a unique `tempfile::TempDir` and an OS-assigned `127.0.0.1:0`
//! port, so a concurrent second copy (e.g. a `bacon` loop) can't collide.

mod common;
use common::{http_get, stop};
use std::collections::BTreeMap;
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
    monitor.stop();
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
        monitor.stop();
        let _ = done_tx.send(());
    });
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("monitor did not stop within 5 seconds of the stop signal");
}
