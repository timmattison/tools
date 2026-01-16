use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use regex::Regex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

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
            anyhow::bail!("IOPS collection requires root privileges");
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

        // Spawn async task to parse fs_usage output
        tokio::spawn(async move {
            if let Err(e) = parse_fs_usage(stdout, data).await {
                // Signal error to main loop and log
                parser_error.store(true, Ordering::Relaxed);
                eprintln!("fs_usage parser error: {e}");
            }
        });

        Ok(Arc::clone(&self.data))
    }

    /// Gets a snapshot of current IOPS data and resets counters.
    ///
    /// Call this periodically (e.g., every second) to get IOPS rates.
    /// This operation is lock-free for the counter reads themselves.
    pub fn snapshot_and_reset(&self) -> HashMap<u32, IOPSCounter> {
        let data = self.data.read();
        data.iter()
            .map(|(pid, counter)| (*pid, counter.snapshot_and_reset()))
            .collect()
    }

    /// Stops the fs_usage process.
    pub async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
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
}
