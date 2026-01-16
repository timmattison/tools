use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use regex::Regex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

use crate::model::IOPSCounter;

/// Atomic counter for lock-free IOPS tracking.
///
/// Using atomics instead of a mutex reduces contention when the fs_usage parser
/// is rapidly updating counters while the main loop periodically reads them.
#[derive(Default)]
pub struct AtomicIOPSCounter {
    /// Read operations count.
    pub read_ops: AtomicU64,
    /// Write operations count.
    pub write_ops: AtomicU64,
}

impl AtomicIOPSCounter {
    /// Increments the read operation counter.
    pub fn increment_read(&self) {
        self.read_ops.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the write operation counter.
    pub fn increment_write(&self) {
        self.write_ops.fetch_add(1, Ordering::Relaxed);
    }

    /// Takes a snapshot and resets the counters atomically.
    ///
    /// Returns the values that were present before the reset.
    pub fn snapshot_and_reset(&self) -> IOPSCounter {
        IOPSCounter {
            read_ops: self.read_ops.swap(0, Ordering::Relaxed),
            write_ops: self.write_ops.swap(0, Ordering::Relaxed),
        }
    }

    /// Returns true if both counters are currently zero.
    ///
    /// This is used for race-safe cleanup: after taking a snapshot that showed zero,
    /// we double-check the live counters before removing the entry, in case new
    /// operations arrived between snapshot and cleanup.
    pub fn is_zero(&self) -> bool {
        self.read_ops.load(Ordering::Relaxed) == 0
            && self.write_ops.load(Ordering::Relaxed) == 0
    }
}

/// Shared state for IOPS data collected from fs_usage.
///
/// Uses a parking_lot RwLock for efficient concurrent access - the parser
/// needs write access to add new PIDs, while reads are frequent from the main loop.
pub type IOPSData = Arc<parking_lot::RwLock<HashMap<u32, Arc<AtomicIOPSCounter>>>>;

/// IOPS collector that parses fs_usage output.
pub struct IOPSCollector {
    child: Option<Child>,
    data: IOPSData,
    /// Flag indicating if the parser encountered an error.
    parser_error: Arc<AtomicBool>,
    /// Handle to the parser task for cleanup and error checking.
    parser_handle: Option<JoinHandle<()>>,
}

impl IOPSCollector {
    /// Creates a new IOPS collector.
    ///
    /// Note: `start()` must be called to begin collecting data.
    pub fn new() -> Self {
        Self {
            child: None,
            data: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            parser_error: Arc::new(AtomicBool::new(false)),
            parser_handle: None,
        }
    }

    /// Returns whether we're running as root (required for fs_usage).
    pub fn is_root() -> bool {
        // SAFETY: geteuid is a standard POSIX function that returns a uid_t.
        // It has no preconditions and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }

    /// Returns whether the parser has encountered an error.
    ///
    /// Check this periodically to detect if IOPS collection has stopped working.
    pub fn has_parser_error(&self) -> bool {
        self.parser_error.load(Ordering::Relaxed)
    }

    /// Starts the fs_usage process and begins parsing its output.
    ///
    /// Returns the shared data handle that can be used to read current IOPS.
    ///
    /// # Errors
    ///
    /// Returns an error if fs_usage cannot be started (e.g., not running as root).
    pub async fn start(&mut self) -> Result<IOPSData> {
        if !Self::is_root() {
            anyhow::bail!(
                "IOPS collection requires root privileges. Run with: sudo disk-hog"
            );
        }

        // Spawn fs_usage with diskio filter
        // -w forces wide output, -f diskio filters to disk I/O events only
        let mut child = Command::new("fs_usage")
            .args(["-w", "-f", "diskio"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null()) // Suppress DTrace warnings
            .kill_on_drop(true)
            .spawn()
            .context("Failed to start fs_usage")?;

        let stdout = child.stdout.take().context("Failed to get stdout")?;
        self.child = Some(child);

        // Clone handles for the parsing task
        let data = Arc::clone(&self.data);
        let parser_error = Arc::clone(&self.parser_error);

        // Spawn async task to parse fs_usage output, storing handle for cleanup
        let handle = tokio::spawn(async move {
            if let Err(e) = parse_fs_usage(stdout, data).await {
                // Signal error to main loop and log
                parser_error.store(true, Ordering::Relaxed);
                eprintln!("fs_usage parser error: {e}");
            }
        });
        self.parser_handle = Some(handle);

        Ok(Arc::clone(&self.data))
    }

    /// Gets a snapshot of current IOPS data and resets counters.
    ///
    /// Call this periodically (e.g., every second) to get IOPS rates.
    /// This operation is lock-free for the counter reads themselves.
    ///
    /// Also cleans up entries for processes that had no I/O activity since the last
    /// snapshot (zero counts), preventing unbounded memory growth from dead processes.
    /// The cleanup uses a race-safe double-check: PIDs are only removed if their
    /// counters are still zero at removal time.
    pub fn snapshot_and_reset(&self) -> HashMap<u32, IOPSCounter> {
        // First, take snapshots with a read lock (fast path)
        let snapshots: HashMap<u32, IOPSCounter> = {
            let data = self.data.read();
            data.iter()
                .map(|(pid, counter)| (*pid, counter.snapshot_and_reset()))
                .collect()
        };

        // Collect PIDs with zero counts (dead or idle processes)
        let zero_pids: Vec<u32> = snapshots
            .iter()
            .filter(|(_, counter)| counter.total() == 0)
            .map(|(pid, _)| *pid)
            .collect();

        // Remove zero-count entries if any exist (requires write lock)
        // Use race-safe removal: double-check the live counter before removing,
        // in case new operations arrived between snapshot and cleanup.
        if !zero_pids.is_empty() {
            let mut data = self.data.write();
            for pid in zero_pids {
                // Only remove if the counter is still zero (race-safe check)
                if let Some(counter) = data.get(&pid) {
                    if counter.is_zero() {
                        data.remove(&pid);
                    }
                }
            }
        }

        snapshots
    }

    /// Stops the fs_usage process and waits for the parser task to complete.
    ///
    /// This ensures clean shutdown and logs any parser task panics.
    pub async fn stop(&mut self) {
        // Kill the fs_usage process first - this will cause the parser to exit
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
        }

        // Wait for the parser task to complete and check for panics
        if let Some(handle) = self.parser_handle.take() {
            match handle.await {
                Ok(()) => {}
                Err(e) if e.is_panic() => {
                    eprintln!("fs_usage parser task panicked: {e}");
                }
                Err(e) => {
                    eprintln!("fs_usage parser task error: {e}");
                }
            }
        }
    }
}

impl Default for IOPSCollector {
    fn default() -> Self {
        Self::new()
    }
}

// Note: No explicit Drop impl needed - child process is killed on drop due to kill_on_drop(true)

/// Parses fs_usage output and updates IOPS counters.
async fn parse_fs_usage(stdout: tokio::process::ChildStdout, data: IOPSData) -> Result<()> {
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();

    // Regex to extract process name and PID from end of line
    // Format: ProcessName.PID or ProcessName.ThreadID (we want PID)
    // The pattern is: non-whitespace followed by dot followed by digits at end of line
    let proc_regex = Regex::new(r"(\S+)\.(\d+)\s*$")?;

    while let Some(line) = lines.next_line().await? {
        // Parse the operation type and process info
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 2 {
            continue;
        }

        let operation = fields[1];

        // Only count actual disk I/O operations
        let is_read = operation.starts_with("Rd");
        let is_write = operation.starts_with("Wr");

        if !is_read && !is_write {
            continue;
        }

        // Extract process.PID from end of line
        if let Some(caps) = proc_regex.captures(&line) {
            let pid: u32 = match caps.get(2).and_then(|m| m.as_str().parse().ok()) {
                Some(pid) => pid,
                None => continue,
            };

            // Get or create counter for this PID
            // First try a read lock (fast path for existing PIDs)
            let counter = {
                let read_guard = data.read();
                read_guard.get(&pid).cloned()
            };

            let counter = match counter {
                Some(c) => c,
                None => {
                    // Need to insert a new counter - upgrade to write lock
                    let mut write_guard = data.write();
                    // Double-check in case another thread inserted
                    write_guard
                        .entry(pid)
                        .or_insert_with(|| Arc::new(AtomicIOPSCounter::default()))
                        .clone()
                }
            };

            // Update counter (lock-free)
            if is_read {
                counter.increment_read();
            } else if is_write {
                counter.increment_write();
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_iops_counter_increment() {
        let counter = AtomicIOPSCounter::default();
        counter.increment_read();
        counter.increment_read();
        counter.increment_write();

        let snapshot = counter.snapshot_and_reset();
        assert_eq!(snapshot.read_ops, 2);
        assert_eq!(snapshot.write_ops, 1);
    }

    #[test]
    fn test_atomic_iops_counter_reset() {
        let counter = AtomicIOPSCounter::default();
        counter.increment_read();
        counter.increment_write();

        // First snapshot should have values
        let snapshot1 = counter.snapshot_and_reset();
        assert_eq!(snapshot1.read_ops, 1);
        assert_eq!(snapshot1.write_ops, 1);

        // Second snapshot should be zeroed
        let snapshot2 = counter.snapshot_and_reset();
        assert_eq!(snapshot2.read_ops, 0);
        assert_eq!(snapshot2.write_ops, 0);
    }

    #[test]
    fn test_is_root_returns_bool() {
        // Just verify it doesn't panic and returns a bool
        let _is_root = IOPSCollector::is_root();
    }

    #[test]
    fn test_parser_error_flag() {
        let collector = IOPSCollector::new();
        assert!(!collector.has_parser_error());
    }

    #[test]
    fn test_snapshot_and_reset_cleans_up_zero_entries() {
        let collector = IOPSCollector::new();

        // Manually insert some counters
        {
            let mut data = collector.data.write();
            let counter1 = Arc::new(AtomicIOPSCounter::default());
            counter1.increment_read();
            data.insert(1001, counter1);

            let counter2 = Arc::new(AtomicIOPSCounter::default());
            counter2.increment_write();
            data.insert(1002, counter2);
        }

        // First snapshot should return both entries and reset them
        let snapshot1 = collector.snapshot_and_reset();
        assert_eq!(snapshot1.len(), 2);
        assert_eq!(snapshot1.get(&1001).unwrap().read_ops, 1);
        assert_eq!(snapshot1.get(&1002).unwrap().write_ops, 1);

        // Second snapshot should show zero counts and trigger cleanup
        let snapshot2 = collector.snapshot_and_reset();
        assert_eq!(snapshot2.len(), 2); // Still 2 entries in snapshot
        assert_eq!(snapshot2.get(&1001).unwrap().total(), 0);
        assert_eq!(snapshot2.get(&1002).unwrap().total(), 0);

        // But the internal data should now be empty (cleaned up)
        let data = collector.data.read();
        assert_eq!(data.len(), 0, "Zero-count entries should be cleaned up");
    }

    #[test]
    fn test_atomic_iops_counter_is_zero() {
        let counter = AtomicIOPSCounter::default();

        // Initially zero
        assert!(counter.is_zero());

        // After increment, not zero
        counter.increment_read();
        assert!(!counter.is_zero());

        // After reset, zero again
        let _ = counter.snapshot_and_reset();
        assert!(counter.is_zero());

        // Write also makes it non-zero
        counter.increment_write();
        assert!(!counter.is_zero());
    }

    #[test]
    fn test_cleanup_skips_non_zero_counters() {
        // This test verifies that cleanup double-checks the counter before removal
        let collector = IOPSCollector::new();

        // Insert a counter that starts with activity
        {
            let mut data = collector.data.write();
            let counter = Arc::new(AtomicIOPSCounter::default());
            counter.increment_read();
            data.insert(1001, counter);
        }

        // First snapshot - counter has activity, won't be in zero_pids
        let snapshot1 = collector.snapshot_and_reset();
        assert_eq!(snapshot1.get(&1001).unwrap().read_ops, 1);

        // Counter is now zero after reset
        // Get a reference to increment it during the "race window"
        let counter_ref = {
            let data = collector.data.read();
            Arc::clone(data.get(&1001).unwrap())
        };

        // Simulate: snapshot sees zero, but before cleanup, new activity arrives
        // We'll do this by incrementing after taking the snapshot data but before
        // the cleanup would run. Since snapshot_and_reset is atomic, we test the
        // is_zero check by verifying behavior.

        // Add activity to the counter
        counter_ref.increment_write();

        // Now take a snapshot - the snapshot will see zero (from previous reset)
        // but cleanup should skip because counter.is_zero() returns false
        let snapshot2 = collector.snapshot_and_reset();
        // snapshot2 captures the write we just did
        assert_eq!(snapshot2.get(&1001).unwrap().write_ops, 1);

        // After snapshot2, the counter is reset to zero again
        // So cleanup WILL remove it this time
        let snapshot3 = collector.snapshot_and_reset();
        assert_eq!(snapshot3.get(&1001).unwrap().total(), 0);

        // Now it should be cleaned up
        let data = collector.data.read();
        assert!(data.is_empty(), "Counter should be cleaned up after two zero snapshots");
    }
}
