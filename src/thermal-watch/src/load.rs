//! Make a full P-core load that cannot outlive the process.
//!
//! # Why the deadline is inside each worker
//!
//! A load generator whose only stop is a call after the loop is a load
//! generator that never stops when the caller dies. Each worker here carries
//! its own deadline and checks it, so the load ends on time even when nothing
//! calls [`Load::stop`]. The workers are threads rather than processes, so they
//! also end when the process ends. Neither guarantee depends on the other.

use std::ffi::CString;
use std::hint::black_box;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, available_parallelism, JoinHandle};
use std::time::Instant;

/// The sysctl that reports how many performance cores an Apple Silicon Mac has.
const PERFORMANCE_CORES: &str = "hw.perflevel0.physicalcpu";

/// The sysctl that reports the total count of physical cores, used when the
/// machine reports no performance level.
const PHYSICAL_CORES: &str = "hw.physicalcpu";

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
    pub fn start(threads: usize, deadline: Instant) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let workers = (0..threads)
            .map(|_| {
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    let mut accumulator = 1.0_f64;
                    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
                        for step in 0..BATCH {
                            accumulator = (accumulator * 1.000_000_1_f64 + f64::from(step)).sqrt();
                        }
                    }
                    // Keep the optimizer from removing the loop that is the
                    // whole point of this thread.
                    black_box(accumulator);
                })
            })
            .collect();
        Self { workers, stop }
    }

    /// How many workers are running.
    #[must_use]
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// Stop every worker and wait for it to end.
    pub fn stop(mut self) {
        self.shutdown();
    }

    /// Set the flag and join every worker that is still running.
    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for worker in self.workers.drain(..) {
            // A worker that panicked has already stopped burning cycles, which
            // is the only thing this call is here to guarantee.
            drop(worker.join());
        }
    }
}

impl Drop for Load {
    /// Stop the workers even when the caller never reaches [`Self::stop`].
    ///
    /// This runs on the panic path too. It is a convenience rather than the
    /// guarantee: the deadline inside each worker is what bounds the load when
    /// nothing runs at all.
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// How many performance cores this machine has.
///
/// A machine that reports no performance core level, such as an Intel Mac,
/// gives its total count of cores instead.
#[must_use]
pub fn performance_core_count() -> usize {
    sysctl_usize(PERFORMANCE_CORES)
        .or_else(|| sysctl_usize(PHYSICAL_CORES))
        .unwrap_or_else(|| available_parallelism().map_or(1, NonZeroUsize::get))
}

/// Read one integer sysctl by name.
///
/// A name the kernel does not publish gives `None`, which is how a machine with
/// no performance core level reports itself.
fn sysctl_usize(name: &str) -> Option<usize> {
    let key = CString::new(name).ok()?;
    let mut value: u32 = 0;
    let mut size = size_of::<u32>();

    // SAFETY: `key` is a NUL-terminated C string that outlives the call.
    // `value` and `size` are live locals, and `size` holds the size of `value`,
    // so the kernel writes at most that many bytes. Both output pointers are
    // non-null and the two input pointers are null, which this interface
    // accepts for a read by name.
    let result = unsafe {
        libc::sysctlbyname(
            key.as_ptr(),
            std::ptr::from_mut(&mut value).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };

    if result != 0 || size != size_of::<u32>() || value == 0 {
        return None;
    }
    Some(value as usize)
}
