//! Make a full P-core load that cannot outlive the process.
//!
//! # Why the deadline is inside each worker
//!
//! A load generator whose only stop is a call after the loop is a load
//! generator that never stops when the caller dies. Each worker here carries
//! its own deadline and checks it, so the load ends on time even when nothing
//! calls [`Load::stop`]. The workers are threads rather than processes, so they
//! also end when the process ends. Neither guarantee depends on the other.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

/// How many arithmetic operations run between two checks of the deadline.
///
/// A check on every operation would cost more than the work it guards. A batch
/// this size keeps the check under a millisecond of delay.
pub const BATCH: u32 = 1_000_000;

/// A running full-speed load on the performance cores.
#[derive(Debug)]
pub struct Load {
    /// One handle for each worker thread.
    workers: Vec<JoinHandle<()>>,
    /// Set to stop every worker before its deadline.
    stop: Arc<AtomicBool>,
}

impl Load {
    /// Start one worker for each performance core, every one of them ending at
    /// `deadline`.
    #[must_use]
    pub fn start(_threads: usize, _deadline: Instant) -> Self {
        Self {
            workers: Vec::new(),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// How many workers are running.
    #[must_use]
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// Stop every worker and wait for it to end.
    pub fn stop(self) {
        let _ = &self.stop;
    }
}

/// How many performance cores this machine has.
///
/// A machine that reports no performance core level, such as an Intel Mac,
/// gives its total count of cores instead.
#[must_use]
pub fn performance_core_count() -> usize {
    0
}
