//! Exercises the ambiguity in `tiny_http::Server::recv()`'s `Err` result: it
//! fires both for a deliberate `unblock()` and for a dead accept thread
//! reporting a real failure (an `accept()` error, then one pushed
//! `Message::Error`, then the accept loop exits). Verified against the
//! `tiny_http` 0.12 source: both paths release exactly one `recv()`-blocked
//! worker per event, through the crate's internal message queue, and both
//! surface to that one worker as an `Err` — nothing in the error's `Display`
//! text or its `ErrorKind` tells the two apart (`ErrorKind::Other` covers
//! both), so this suite never inspects either. `Pool` tells them apart with a
//! shutdown flag it owns instead.
//!
//! A real accept failure (EMFILE, say) can't be triggered safely or reliably
//! in a test — exhausting the file-descriptor table would be a hostile test
//! that could take down unrelated work sharing this machine. Calling
//! `server.unblock()` exactly once, on a pool whose `Pool::shutdown` was never
//! called, reproduces the identical event shape instead: one worker released
//! with an `Err`, and nothing in the pool's own state explains it. An
//! unexplained release like that is, by design, indistinguishable from a real
//! dead acceptor — treating it as one is the conservative reading the fix
//! commits to.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// Workers for a pool that must show "one released, the rest still blocked" —
/// at least two, so a single release leaves someone still parked in `recv()`
/// if nothing else unblocks them.
const WORKERS: usize = 3;

/// Bounds every wait below. A pool that never finishes releasing its workers
/// is exactly the defect under test, so the wait must fail this test rather
/// than hang the suite — the same technique `tests/cli.rs` uses to bound its
/// banner wait.
const JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// An export root with nothing in it — these tests never serve a request.
fn canonical_root() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path().canonicalize().expect("canonicalize root");
    (dir, root)
}

/// Runs a consuming `Pool` method on its own thread and returns its result
/// within [`JOIN_TIMEOUT`], or panics — a `Pool` that never finishes ending is
/// exactly the defect these tests exist to catch, and a test that can hang
/// forever alongside it would be worse than the bug.
fn within_timeout<F>(run: F) -> std::io::Result<()>
where
    F: FnOnce() -> std::io::Result<()> + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(run());
    });

    receiver
        .recv_timeout(JOIN_TIMEOUT)
        .unwrap_or_else(|_| panic!("did not finish within {JOIN_TIMEOUT:?}"))
}

/// The load-bearing red: one release with nothing accounting for it must
/// bring down every worker — not just the one that saw it — and `join()` must
/// report it as a failure.
#[test]
fn an_unexplained_release_brings_down_every_worker_and_surfaces() {
    let (_dir, root) = canonical_root();
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));

    let pool = localnext::serve(Arc::clone(&server), Arc::new(root), WORKERS);

    // One release, with `Pool`'s own shutdown flag still clear — see the
    // module doc for why this reproduces a dead acceptor's failure shape
    // without an actual EMFILE.
    server.unblock();

    let result = within_timeout(move || pool.join());

    assert!(
        result.is_err(),
        "an unexplained release should surface as a join() error, got Ok(())"
    );
}

/// The regression guard: a deliberate shutdown must still end every worker
/// cleanly and report no error, so the fix above cannot make this pass by
/// breaking the ordinary shutdown path.
#[test]
fn a_deliberate_shutdown_ends_every_worker_cleanly() {
    let (_dir, root) = canonical_root();
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));

    let pool = localnext::serve(server, Arc::new(root), WORKERS);

    let result = within_timeout(move || pool.shutdown());

    assert!(
        result.is_ok(),
        "a deliberate shutdown should report no error, got {result:?}"
    );
}
