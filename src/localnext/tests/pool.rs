//! Exercises the ambiguity in `tiny_http::Server::recv()`'s `Err` result: it
//! fires both for a deliberate `unblock()` and for a dead accept thread
//! reporting a real failure (an `accept()` error, then one pushed
//! `Message::Error`, then the accept loop exits). Verified against the
//! `tiny_http` 0.12 source: both paths release exactly one `recv()`-blocked
//! worker per event, through the crate's internal message queue, and both
//! surface to that one worker as an `Err` — nothing in the error's `Display`
//! text or its `ErrorKind` tells the two apart (`ErrorKind::Other` covers
//! both), so this suite never inspects either.
//!
//! A real accept failure (EMFILE, say) can't be triggered safely or reliably
//! in a test — exhausting the file-descriptor table would be a hostile test
//! that could take down unrelated work sharing this machine. Calling
//! `server.unblock()` exactly once reproduces the identical event shape
//! instead: one worker released with an `Err`, and nothing explains why. An
//! unexplained release like that is, by design, indistinguishable from a real
//! dead acceptor — treating it as one is the conservative reading the fix
//! this file drives commits to.

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
fn canonical_root() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path().canonicalize().expect("canonicalize root");
    (dir, root)
}

/// The load-bearing red: one release with nothing accounting for it must
/// bring down every worker, not just the one that saw it.
///
/// Today, `serve()`'s workers loop on `server.recv()` and treat any `Err` as
/// "the pool is shutting down", so the one worker `server.unblock()` releases
/// exits quietly while the other `WORKERS - 1` stay parked in `recv()`
/// forever — nothing will ever wake them, because a real dead acceptor would
/// never call `unblock()` again either. This test's own wait is bounded, but
/// the leaked worker threads are not reaped until the test binary's process
/// exits; that is unavoidable without the fix, since nothing outside a fixed
/// process exit can force a thread out of a blocking `recv()`.
#[test]
fn an_unexplained_release_brings_down_every_worker() {
    let (_dir, root) = canonical_root();
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));

    let handles = localnext::serve(Arc::clone(&server), Arc::new(root), WORKERS);

    // One release, with nothing accounting for it — see the module doc for
    // why this reproduces a dead acceptor's failure shape without an actual
    // EMFILE.
    server.unblock();

    // Join every worker on its own thread so this test's own wait is bounded:
    // a bare `for handle in handles { handle.join() }` on the test thread
    // blocks forever today, because only the one released worker ever exits.
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for handle in handles {
            let _ = handle.join();
        }
        let _ = sender.send(());
    });

    assert!(
        receiver.recv_timeout(JOIN_TIMEOUT).is_ok(),
        "every worker should exit within {JOIN_TIMEOUT:?} after one unexplained release; \
         at least one is still blocked in recv() forever"
    );
}

/// The regression guard: a release fully accounted for — once per worker —
/// must still end the pool cleanly. This already holds today; it exists so
/// the fix that makes the test above pass cannot do so by breaking this one.
#[test]
fn a_release_accounted_for_once_per_worker_ends_the_pool_cleanly() {
    let (_dir, root) = canonical_root();
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind ephemeral port"));

    let handles = localnext::serve(Arc::clone(&server), Arc::new(root), WORKERS);

    for _ in 0..WORKERS {
        server.unblock();
    }

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for handle in handles {
            let _ = handle.join();
        }
        let _ = sender.send(());
    });

    assert!(
        receiver.recv_timeout(JOIN_TIMEOUT).is_ok(),
        "every worker should exit within {JOIN_TIMEOUT:?} after a release per worker"
    );
}
