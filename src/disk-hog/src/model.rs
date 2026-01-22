use std::time::Duration;

/// Bytes per second rate (newtype for type safety).
///
/// Using a newtype prevents accidentally mixing raw byte counts with rates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct BytesPerSec(pub u64);

impl BytesPerSec {
    /// Creates a new rate from bytes and a time interval.
    ///
    /// Uses floating-point arithmetic internally for precision, then rounds
    /// to the nearest whole number. This avoids precision loss when the interval
    /// is longer than 1 second (e.g., 1 byte over 2 seconds = 0 with integer division,
    /// but correctly rounds to 1 or 0 with this approach).
    ///
    /// If the interval is zero, returns 0 to avoid division by zero.
    ///
    /// # Precision Note
    ///
    /// The `u64 as f64` cast can lose precision for values exceeding 2^53
    /// (~9 petabytes). This is acceptable for disk I/O rates which are
    /// unlikely to reach such magnitudes in practice.
    pub fn from_bytes_and_duration(bytes: u64, interval: Duration) -> Self {
        let secs = interval.as_secs_f64();
        if secs == 0.0 {
            Self(0)
        } else {
            // Use f64 for precise division, then round to nearest integer
            #[expect(
                clippy::cast_precision_loss,
                reason = "Precision loss only occurs above 2^53 (~9 PB), far beyond realistic disk I/O rates"
            )]
            let bytes_f64 = bytes as f64;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "bytes/sec rate will always fit in u64 and be positive"
            )]
            let rate = (bytes_f64 / secs).round() as u64;
            Self(rate)
        }
    }

    /// Creates a new rate from bytes and interval in whole seconds.
    ///
    /// This is a convenience method that converts to Duration internally.
    /// For sub-second precision, use `from_bytes_and_duration` directly.
    #[cfg(test)]
    pub fn from_bytes_and_interval(bytes: u64, interval_secs: u64) -> Self {
        Self::from_bytes_and_duration(bytes, Duration::from_secs(interval_secs))
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
    /// Creates a new rate from operation count and a time interval.
    ///
    /// Uses floating-point arithmetic internally for precision, then rounds
    /// to the nearest whole number. This avoids precision loss when the interval
    /// is longer than 1 second (e.g., 1 op over 2 seconds = 0 with integer division,
    /// but correctly rounds to 1 or 0 with this approach).
    ///
    /// If the interval is zero, returns 0 to avoid division by zero.
    ///
    /// # Precision Note
    ///
    /// The `u64 as f64` cast can lose precision for values exceeding 2^53
    /// (~9 quadrillion ops). This is acceptable for IOPS rates which are
    /// unlikely to reach such magnitudes in practice.
    pub fn from_ops_and_duration(ops: u64, interval: Duration) -> Self {
        let secs = interval.as_secs_f64();
        if secs == 0.0 {
            Self(0)
        } else {
            // Use f64 for precise division, then round to nearest integer
            #[expect(
                clippy::cast_precision_loss,
                reason = "Precision loss only occurs above 2^53 (~9 quadrillion ops), far beyond realistic IOPS rates"
            )]
            let ops_f64 = ops as f64;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "ops/sec rate will always fit in u64 and be positive"
            )]
            let rate = (ops_f64 / secs).round() as u64;
            Self(rate)
        }
    }

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
    ///
    /// Returns `Some` if at least one of read or write ops is available, treating
    /// the missing component as zero. This graceful handling of partial data allows
    /// the UI to display something meaningful even if only reads or only writes
    /// were captured for a process.
    ///
    /// Returns `None` only when neither read nor write IOPS are available (i.e.,
    /// when running without sudo and IOPS collection is disabled).
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
    fn test_bytes_per_sec_from_duration() {
        // Test with Duration for more precision
        assert_eq!(
            BytesPerSec::from_bytes_and_duration(1000, Duration::from_secs(1)).as_u64(),
            1000
        );
        assert_eq!(
            BytesPerSec::from_bytes_and_duration(1000, Duration::from_millis(500)).as_u64(),
            2000
        );
        assert_eq!(
            BytesPerSec::from_bytes_and_duration(1, Duration::from_secs(2)).as_u64(),
            1 // Rounds to 1, not truncates to 0
        );
        assert_eq!(
            BytesPerSec::from_bytes_and_duration(0, Duration::from_secs(1)).as_u64(),
            0
        );
    }

    #[test]
    fn test_bytes_per_sec_precision_edge_cases() {
        // Small values over long intervals - previously would truncate to 0
        assert_eq!(
            BytesPerSec::from_bytes_and_duration(1, Duration::from_secs(3)).as_u64(),
            0 // 0.33... rounds to 0
        );
        assert_eq!(
            BytesPerSec::from_bytes_and_duration(2, Duration::from_secs(3)).as_u64(),
            1 // 0.66... rounds to 1
        );
        // Verify rounding behavior
        assert_eq!(
            BytesPerSec::from_bytes_and_duration(5, Duration::from_secs(2)).as_u64(),
            3 // 2.5 rounds to 3 (round half up)
        );
    }

    #[test]
    fn test_bytes_per_sec_add() {
        let a = BytesPerSec(100);
        let b = BytesPerSec(200);
        assert_eq!((a + b).as_u64(), 300);
    }

    #[test]
    fn test_ops_per_sec_from_duration() {
        // Test with Duration for precision
        assert_eq!(
            OpsPerSec::from_ops_and_duration(1000, Duration::from_secs(1)).as_u64(),
            1000
        );
        assert_eq!(
            OpsPerSec::from_ops_and_duration(1000, Duration::from_millis(500)).as_u64(),
            2000
        );
        assert_eq!(
            OpsPerSec::from_ops_and_duration(1, Duration::from_secs(2)).as_u64(),
            1 // Rounds to 1, not truncates to 0
        );
        assert_eq!(
            OpsPerSec::from_ops_and_duration(0, Duration::from_secs(1)).as_u64(),
            0
        );
        // Zero duration should return 0
        assert_eq!(
            OpsPerSec::from_ops_and_duration(100, Duration::ZERO).as_u64(),
            0
        );
    }

    #[test]
    fn test_ops_per_sec_precision_edge_cases() {
        // Small values over long intervals
        assert_eq!(
            OpsPerSec::from_ops_and_duration(1, Duration::from_secs(3)).as_u64(),
            0 // 0.33... rounds to 0
        );
        assert_eq!(
            OpsPerSec::from_ops_and_duration(2, Duration::from_secs(3)).as_u64(),
            1 // 0.66... rounds to 1
        );
        // Verify rounding behavior
        assert_eq!(
            OpsPerSec::from_ops_and_duration(5, Duration::from_secs(2)).as_u64(),
            3 // 2.5 rounds to 3 (round half up)
        );
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
    fn test_iops_counter_total() {
        let counter = IOPSCounter {
            read_ops: 10,
            write_ops: 20,
        };
        assert_eq!(counter.total(), 30);
    }
}
