use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct GitOpStats {
    count: u64,
    total_duration: Duration,
}

impl GitOpStats {
    pub fn new() -> Self {
        Self {
            count: 0,
            total_duration: Duration::new(0, 0),
        }
    }

    /// Record one operation that took `duration`.
    ///
    /// The count and the total stop at their largest values instead of
    /// overflowing.
    pub fn record(&mut self, duration: Duration) {
        self.count = self.count.saturating_add(1);
        self.total_duration = self.total_duration.saturating_add(duration);
    }

    /// Calculate the average duration per operation.
    ///
    /// Uses nanosecond arithmetic to correctly handle counts exceeding `u32::MAX`.
    /// This function does not panic, also for a total as large as `Duration::MAX`.
    pub fn average(&self) -> Duration {
        const NANOS_PER_SECOND: u128 = 1_000_000_000;
        if self.count == 0 {
            Duration::ZERO
        } else {
            // Use nanoseconds to avoid u32 overflow issues with Duration::div
            let average_nanos = self.total_duration.as_nanos() / u128::from(self.count);
            // The average is not more than the total, and the total is not more than
            // `Duration::MAX`, so the whole seconds fit in a `u64`.
            let seconds = u64::try_from(average_nanos / NANOS_PER_SECOND).unwrap_or(u64::MAX);
            // The remainder of a division by one billion is less than one billion.
            let subsecond_nanos = u32::try_from(average_nanos % NANOS_PER_SECOND).unwrap_or(0);
            Duration::new(seconds, subsecond_nanos)
        }
    }

    pub fn count(&self) -> u64 {
        self.count
    }
}

impl Default for GitOpStats {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct GitStats {
    pub get_git_dir: Arc<Mutex<GitOpStats>>,
    pub get_log: Arc<Mutex<GitOpStats>>,
    pub get_email: Arc<Mutex<GitOpStats>>,
}

impl GitStats {
    pub fn new() -> Self {
        Self {
            get_git_dir: Arc::new(Mutex::new(GitOpStats::new())),
            get_log: Arc::new(Mutex::new(GitOpStats::new())),
            get_email: Arc::new(Mutex::new(GitOpStats::new())),
        }
    }

    pub fn record_git_dir(&self, duration: Duration) {
        if let Ok(mut stats) = self.get_git_dir.lock() {
            stats.record(duration);
        }
    }

    pub fn record_log(&self, duration: Duration) {
        if let Ok(mut stats) = self.get_log.lock() {
            stats.record(duration);
        }
    }

    pub fn record_email(&self, duration: Duration) {
        if let Ok(mut stats) = self.get_email.lock() {
            stats.record(duration);
        }
    }
}

impl Default for GitStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Timer helper for measuring operation duration
pub struct Timer {
    start: Instant,
}

impl Timer {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_op_stats_average_empty() {
        let stats = GitOpStats::new();
        assert_eq!(stats.average(), Duration::ZERO);
    }

    #[test]
    fn git_op_stats_average_single() {
        let mut stats = GitOpStats::new();
        stats.record(Duration::from_millis(100));
        assert_eq!(stats.average(), Duration::from_millis(100));
    }

    #[test]
    fn git_op_stats_average_multiple() {
        let mut stats = GitOpStats::new();
        stats.record(Duration::from_millis(100));
        stats.record(Duration::from_millis(200));
        stats.record(Duration::from_millis(300));
        assert_eq!(stats.average(), Duration::from_millis(200));
    }

    #[test]
    fn git_op_stats_average_large_count() {
        // Simulate a scenario where count exceeds u32::MAX
        // We can't actually record 5 billion operations, but we can test
        // that the math works by creating a stats struct directly
        let stats = GitOpStats {
            count: 5_000_000_000, // > u32::MAX (4,294,967,295)
            total_duration: Duration::from_secs(10_000_000_000), // 10 billion seconds
        };
        // Average should be 2 seconds per operation
        assert_eq!(stats.average(), Duration::from_secs(2));
    }

    #[test]
    fn git_op_stats_average_sub_nanosecond_precision() {
        // Test that sub-nanosecond precision is truncated correctly
        let mut stats = GitOpStats::new();
        stats.record(Duration::from_nanos(10));
        stats.record(Duration::from_nanos(11));
        stats.record(Duration::from_nanos(12));
        // Average: 33 / 3 = 11 nanoseconds
        assert_eq!(stats.average(), Duration::from_nanos(11));
    }

    #[test]
    fn git_op_stats_average_of_one_max_duration_is_max() {
        let mut stats = GitOpStats::new();
        stats.record(Duration::MAX);
        assert_eq!(stats.average(), Duration::MAX);
    }

    #[test]
    fn git_op_stats_total_stops_at_max() {
        let mut stats = GitOpStats::new();
        stats.record(Duration::MAX);
        stats.record(Duration::MAX);
        assert_eq!(stats.count(), 2);
        assert_eq!(stats.average(), Duration::MAX / 2);
    }

    #[test]
    fn git_op_stats_count() {
        let mut stats = GitOpStats::new();
        assert_eq!(stats.count(), 0);
        stats.record(Duration::from_millis(100));
        assert_eq!(stats.count(), 1);
        stats.record(Duration::from_millis(100));
        assert_eq!(stats.count(), 2);
    }

    #[test]
    fn git_op_stats_default() {
        let stats = GitOpStats::default();
        assert_eq!(stats.count(), 0);
        assert_eq!(stats.average(), Duration::ZERO);
    }
}
