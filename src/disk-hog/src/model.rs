/// Statistics for a single process's disk I/O.
#[derive(Debug, Clone)]
pub struct ProcessIOStats {
    /// Process ID.
    pub pid: u32,
    /// Process name.
    pub name: String,
    /// Bytes read per second.
    pub read_bytes_per_sec: u64,
    /// Bytes written per second.
    pub write_bytes_per_sec: u64,
    /// Read operations per second (None if not running with sudo).
    pub read_ops_per_sec: Option<u64>,
    /// Write operations per second (None if not running with sudo).
    pub write_ops_per_sec: Option<u64>,
}

impl ProcessIOStats {
    /// Creates a new `ProcessIOStats` with bandwidth data only.
    pub fn new_bandwidth_only(
        pid: u32,
        name: String,
        read_bytes_per_sec: u64,
        write_bytes_per_sec: u64,
    ) -> Self {
        Self {
            pid,
            name,
            read_bytes_per_sec,
            write_bytes_per_sec,
            read_ops_per_sec: None,
            write_ops_per_sec: None,
        }
    }

    /// Total bandwidth (read + write) in bytes per second.
    pub fn total_bandwidth(&self) -> u64 {
        self.read_bytes_per_sec + self.write_bytes_per_sec
    }

    /// Total IOPS (read + write) per second, if available.
    pub fn total_iops(&self) -> Option<u64> {
        match (self.read_ops_per_sec, self.write_ops_per_sec) {
            (Some(r), Some(w)) => Some(r + w),
            (Some(r), None) => Some(r),
            (None, Some(w)) => Some(w),
            (None, None) => None,
        }
    }
}

/// IOPS counter for a single process, used during fs_usage parsing.
#[derive(Debug, Default, Clone)]
pub struct IOPSCounter {
    /// Read operations count.
    pub read_ops: u64,
    /// Write operations count.
    pub write_ops: u64,
}

impl IOPSCounter {
    /// Resets the counters to zero.
    pub fn reset(&mut self) {
        self.read_ops = 0;
        self.write_ops = 0;
    }

    /// Total operations (read + write).
    pub fn total(&self) -> u64 {
        self.read_ops + self.write_ops
    }
}
