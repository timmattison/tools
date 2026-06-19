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

#[test]
fn file_created_then_deleted_is_reflected_in_fetches() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("late.txt");

    let mut routes = BTreeMap::new();
    routes.insert("/late.txt".to_string(), path.clone());
    let routes = Arc::new(routes);

    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));
    let addr = server.server_addr().to_ip().expect("ip addr");
    let handles = sirn::serve(
        Arc::clone(&server),
        sirn::ServeMode::Files(Arc::clone(&routes)),
        2,
    );

    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    let monitor = sirn::spawn_monitor(Arc::clone(&routes), shutdown_rx);

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

    // Clean teardown: stop the monitor (dropping the sender) and the server.
    drop(shutdown_tx);
    monitor.join().expect("monitor thread joins cleanly");
    stop(&server, handles);
}

#[test]
fn monitor_joins_promptly_after_shutdown_signal() {
    // Even with no served files, dropping the sender must end the poll loop so
    // the join returns rather than blocking for a poll interval.
    let routes = Arc::new(BTreeMap::new());
    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    let monitor = sirn::spawn_monitor(routes, shutdown_rx);
    drop(shutdown_tx);
    monitor
        .join()
        .expect("monitor thread joins cleanly after shutdown");
}
