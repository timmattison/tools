use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use regex::Regex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::model::IOPSCounter;

/// Shared state for IOPS data collected from fs_usage.
pub type IOPSData = Arc<Mutex<HashMap<u32, IOPSCounter>>>;

/// IOPS collector that parses fs_usage output.
pub struct IOPSCollector {
    child: Option<Child>,
    data: IOPSData,
}

impl IOPSCollector {
    /// Creates a new IOPS collector.
    ///
    /// Note: `start()` must be called to begin collecting data.
    pub fn new() -> Self {
        Self {
            child: None,
            data: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Returns whether we're running as root (required for fs_usage).
    pub fn is_root() -> bool {
        // SAFETY: geteuid is a standard POSIX function that just returns a uid_t
        unsafe { libc::geteuid() == 0 }
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

        // Clone data handle for the parsing task
        let data = Arc::clone(&self.data);

        // Spawn async task to parse fs_usage output
        tokio::spawn(async move {
            if let Err(e) = parse_fs_usage(stdout, data).await {
                // Log error but don't crash - fs_usage might just exit
                eprintln!("fs_usage parser error: {e}");
            }
        });

        Ok(Arc::clone(&self.data))
    }

    /// Gets a snapshot of current IOPS data and resets counters.
    ///
    /// Call this periodically (e.g., every second) to get IOPS rates.
    pub async fn snapshot_and_reset(&self) -> HashMap<u32, IOPSCounter> {
        let mut data = self.data.lock().await;
        let snapshot = data.clone();

        // Reset all counters for next interval
        for counter in data.values_mut() {
            counter.reset();
        }

        snapshot
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
async fn parse_fs_usage(
    stdout: tokio::process::ChildStdout,
    data: IOPSData,
) -> Result<()> {
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

            // Update IOPS counter
            let mut data = data.lock().await;
            let counter = data.entry(pid).or_default();

            if is_read {
                counter.read_ops += 1;
            } else if is_write {
                counter.write_ops += 1;
            }
        }
    }

    Ok(())
}
