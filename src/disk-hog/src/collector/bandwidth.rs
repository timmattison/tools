use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind};

use crate::model::{BytesPerSec, ProcessIOStats};

/// Previous disk usage readings for calculating deltas.
#[derive(Default)]
struct PreviousReading {
    read_bytes: u64,
    written_bytes: u64,
}

/// Collector for per-process disk bandwidth using sysinfo.
pub struct BandwidthCollector {
    system: System,
    previous_readings: HashMap<u32, PreviousReading>,
}

/// Returns the standard `ProcessRefreshKind` configuration for disk-hog.
///
/// This ensures consistent refresh behavior across the codebase and prevents
/// accidentally forgetting to request process names (cmd). Always includes:
/// - `with_disk_usage()` - to get read/write bytes
/// - `with_cmd(UpdateKind::OnlyIfNotSet)` - to get process names
#[inline]
pub fn process_refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_disk_usage()
        .with_cmd(UpdateKind::OnlyIfNotSet)
}

impl BandwidthCollector {
    /// Creates a new bandwidth collector.
    pub fn new() -> Self {
        let refresh_kind = RefreshKind::nothing().with_processes(process_refresh_kind());
        let system = System::new_with_specifics(refresh_kind);

        Self {
            system,
            previous_readings: HashMap::new(),
        }
    }

    /// Looks up a process name by PID.
    ///
    /// Returns the process name if found, or a fallback string like "pid:1234"
    /// if the process has exited or is otherwise unavailable. This fallback
    /// can occur when a short-lived process exits between when it was observed
    /// (e.g., by fs_usage) and when we look up its name.
    pub fn lookup_process_name(&self, pid: u32) -> String {
        self.system
            .process(sysinfo::Pid::from_u32(pid))
            .map(|p| p.name().to_string_lossy().to_string())
            .unwrap_or_else(|| format!("pid:{pid}"))
    }

    /// Collects current bandwidth stats for all processes.
    ///
    /// The `elapsed` parameter specifies the actual time since the last collection,
    /// used to calculate accurate bytes-per-second rates. Using `Duration` allows
    /// for sub-second precision and accounts for actual elapsed time rather than
    /// assuming the configured interval was exact.
    ///
    /// Returns a list of `ProcessIOStats` sorted by total bandwidth (descending).
    pub fn collect(&mut self, elapsed: Duration) -> Vec<ProcessIOStats> {
        // Refresh process disk usage
        self.system
            .refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh_kind());

        let mut stats = Vec::new();
        let mut current_pids = HashSet::new();

        for (pid, process) in self.system.processes() {
            let pid_u32 = pid.as_u32();
            current_pids.insert(pid_u32);

            let usage = process.disk_usage();

            // Get previous reading or create default
            let previous = self
                .previous_readings
                .entry(pid_u32)
                .or_insert_with(|| PreviousReading {
                    read_bytes: usage.total_read_bytes,
                    written_bytes: usage.total_written_bytes,
                });

            // Calculate bytes delta since last reading
            // Note: total_read_bytes and total_written_bytes are cumulative
            let read_delta = usage.total_read_bytes.saturating_sub(previous.read_bytes);
            let write_delta = usage.total_written_bytes.saturating_sub(previous.written_bytes);

            // Update previous reading
            previous.read_bytes = usage.total_read_bytes;
            previous.written_bytes = usage.total_written_bytes;

            // Only include processes with some I/O activity
            if read_delta > 0 || write_delta > 0 {
                let name = process.name().to_string_lossy().to_string();

                // Convert deltas to rates using actual elapsed time
                let read_rate = BytesPerSec::from_bytes_and_duration(read_delta, elapsed);
                let write_rate = BytesPerSec::from_bytes_and_duration(write_delta, elapsed);

                stats.push(ProcessIOStats::new_bandwidth_only(
                    pid_u32,
                    name,
                    read_rate,
                    write_rate,
                ));
            }
        }

        // Clean up previous readings for dead processes (O(1) lookup with HashSet)
        self.previous_readings
            .retain(|pid, _| current_pids.contains(pid));

        // Sort by total bandwidth, descending
        stats.sort_by_key(|s| Reverse(s.total_bandwidth()));

        stats
    }
}

impl Default for BandwidthCollector {
    fn default() -> Self {
        Self::new()
    }
}
