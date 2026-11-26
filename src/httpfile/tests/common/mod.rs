//! Shared helpers for the `httpfile` integration tests.
//!
//! This module lives in a `tests/` subdirectory, so it is not a test binary of
//! its own. Each test binary that declares `mod common;` compiles its own copy.

use std::sync::mpsc;
use std::time::Duration;

/// Bounds each wait for a consuming `httpfile::Pool` method. A pool that does
/// not release all of its workers is a defect, and the test must fail on it,
/// not hang the suite.
pub const JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs `run` on its own thread, and returns its result if it ends within
/// [`JOIN_TIMEOUT`].
///
/// # Panics
///
/// Panics when `run` does not end within [`JOIN_TIMEOUT`]. The thread of
/// `run` stays blocked, but the test fails at once.
pub fn within_timeout<F>(run: F) -> std::io::Result<()>
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
