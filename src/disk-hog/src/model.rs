/// Bytes per second rate (newtype for type safety).
///
/// Using a newtype prevents accidentally mixing raw byte counts with rates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct BytesPerSec(pub u64);

impl BytesPerSec {
    /// Creates a new rate from bytes and interval in seconds.
    ///
    /// Divides bytes by interval to get bytes/sec. If interval is 0, returns 0.
    pub fn from_bytes_and_interval(bytes: u64, interval_secs: u64) -> Self {
        if interval_secs == 0 {
            Self(0)
        } else {
            Self(bytes / interval_secs)
        }
    }

    /// Returns the inner value.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl std::ops::Add for BytesPerSec {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

/// Operations per second rate (newtype for type safety).
///
/// Using a newtype prevents accidentally mixing raw operation counts with rates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct OpsPerSec(pub u64);

impl OpsPerSec {
    /// Returns the inner value.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl std::ops::Add for OpsPerSec {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

/// Statistics for a single process's disk I/O.
#[derive(Debug, Clone)]
pub struct ProcessIOStats {
    /// Process ID.
    pub pid: u32,
    /// Process name.
    pub name: String,
    /// Bytes read per second.
    pub read_bytes_per_sec: BytesPerSec,
    /// Bytes written per second.
    pub write_bytes_per_sec: BytesPerSec,
    /// Read operations per second (None if not running with sudo).
    pub read_ops_per_sec: Option<OpsPerSec>,
    /// Write operations per second (None if not running with sudo).
    pub write_ops_per_sec: Option<OpsPerSec>,
}

impl ProcessIOStats {
    /// Creates a new `ProcessIOStats` with bandwidth data only.
    pub fn new_bandwidth_only(
        pid: u32,
        name: String,
        read_bytes_per_sec: BytesPerSec,
        write_bytes_per_sec: BytesPerSec,
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
    pub fn total_bandwidth(&self) -> BytesPerSec {
        self.read_bytes_per_sec + self.write_bytes_per_sec
    }

    /// Total IOPS (read + write) per second, if available.
    pub fn total_iops(&self) -> Option<OpsPerSec> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bytes_per_sec_from_bytes_and_interval() {
        assert_eq!(BytesPerSec::from_bytes_and_interval(1000, 1).as_u64(), 1000);
        assert_eq!(BytesPerSec::from_bytes_and_interval(1000, 2).as_u64(), 500);
        assert_eq!(BytesPerSec::from_bytes_and_interval(1000, 0).as_u64(), 0);
    }

    #[test]
    fn test_bytes_per_sec_add() {
        let a = BytesPerSec(100);
        let b = BytesPerSec(200);
        assert_eq!((a + b).as_u64(), 300);
    }

    #[test]
    fn test_ops_per_sec_add() {
        let a = OpsPerSec(10);
        let b = OpsPerSec(20);
        assert_eq!((a + b).as_u64(), 30);
    }

    #[test]
    fn test_process_io_stats_total_bandwidth() {
        let stats = ProcessIOStats::new_bandwidth_only(
            1234,
            "test".to_string(),
            BytesPerSec(100),
            BytesPerSec(200),
        );
        assert_eq!(stats.total_bandwidth().as_u64(), 300);
    }

    #[test]
    fn test_process_io_stats_total_iops_both() {
        let stats = ProcessIOStats {
            pid: 1234,
            name: "test".to_string(),
            read_bytes_per_sec: BytesPerSec(0),
            write_bytes_per_sec: BytesPerSec(0),
            read_ops_per_sec: Some(OpsPerSec(10)),
            write_ops_per_sec: Some(OpsPerSec(20)),
        };
        assert_eq!(stats.total_iops().unwrap().as_u64(), 30);
    }

    #[test]
    fn test_process_io_stats_total_iops_read_only() {
        let stats = ProcessIOStats {
            pid: 1234,
            name: "test".to_string(),
            read_bytes_per_sec: BytesPerSec(0),
            write_bytes_per_sec: BytesPerSec(0),
            read_ops_per_sec: Some(OpsPerSec(10)),
            write_ops_per_sec: None,
        };
        assert_eq!(stats.total_iops().unwrap().as_u64(), 10);
    }

    #[test]
    fn test_process_io_stats_total_iops_none() {
        let stats = ProcessIOStats::new_bandwidth_only(
            1234,
            "test".to_string(),
            BytesPerSec(100),
            BytesPerSec(200),
        );
        assert!(stats.total_iops().is_none());
    }

    #[test]
    fn test_iops_counter_reset() {
        let mut counter = IOPSCounter {
            read_ops: 10,
            write_ops: 20,
        };
        counter.reset();
        assert_eq!(counter.read_ops, 0);
        assert_eq!(counter.write_ops, 0);
    }

    #[test]
    fn test_iops_counter_total() {
        let counter = IOPSCounter {
            read_ops: 10,
            write_ops: 20,
        };
        assert_eq!(counter.total(), 30);
    }
}
