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

    pub fn record(&mut self, duration: Duration) {
        self.count += 1;
        self.total_duration += duration;
    }

    /// Calculate the average duration per operation.
    ///
    /// Uses nanosecond arithmetic to correctly handle counts exceeding `u32::MAX`.
    pub fn average(&self) -> Duration {
        if self.count == 0 {
            Duration::ZERO
        } else {
            // Use nanoseconds to avoid u32 overflow issues with Duration::div
            let total_nanos = self.total_duration.as_nanos();
            let avg_nanos = total_nanos / u128::from(self.count);
            // Safe: average of durations can't exceed the max duration
            Duration::from_nanos(u64::try_from(avg_nanos).unwrap_or(u64::MAX))
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