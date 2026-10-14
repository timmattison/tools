//! What the status line of `ha` shows: the last refresh, the rate limit of
//! the GitHub API, and the errors that are not about one repository.

use chrono::{DateTime, Utc};

use crate::target::OwnerName;

/// The core rate limit of the GitHub API, as the last response stated it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    /// The number of requests that GitHub allows in one window.
    pub limit: u64,
    /// The number of requests that remain in this window.
    pub remaining: u64,
    /// When the window ends and the count resets.
    pub reset: DateTime<Utc>,
}

impl RateLimit {
    /// Returns true when fewer than 10 percent of the requests remain.
    ///
    /// A limit of zero is never low. A count that is larger than the limit
    /// is never low.
    #[must_use]
    pub fn is_low(&self) -> bool {
        self.remaining.saturating_mul(10) < self.limit
    }
}

/// The state that the status line shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// When the last refresh cycle ended. `None` before the first cycle ends.
    pub last_refresh: Option<DateTime<Utc>>,
    /// True while a refresh cycle runs.
    pub refreshing: bool,
    /// The last rate limit that GitHub stated.
    pub rate: Option<RateLimit>,
    /// When the worker stops to slow down. `Some` while it slows down.
    pub backoff_until: Option<DateTime<Utc>>,
    /// The owners whose repository list failed, with the error of each.
    pub owner_errors: Vec<(OwnerName, String)>,
    /// A one-off message, for example the address after a failed open.
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(limit: u64, remaining: u64) -> RateLimit {
        RateLimit {
            limit,
            remaining,
            reset: DateTime::<Utc>::UNIX_EPOCH,
        }
    }

    #[test]
    fn fewer_than_ten_percent_remaining_is_low() {
        assert!(rate(5000, 499).is_low());
        assert!(rate(5000, 0).is_low());
        assert!(rate(60, 5).is_low());
    }

    #[test]
    fn exactly_ten_percent_or_more_remaining_is_not_low() {
        assert!(!rate(5000, 500).is_low());
        assert!(!rate(5000, 5000).is_low());
    }

    #[test]
    fn a_limit_of_zero_is_not_low() {
        assert!(!rate(0, 0).is_low());
    }

    #[test]
    fn more_remaining_than_the_limit_is_not_low() {
        assert!(!rate(100, 101).is_low());
        assert!(!rate(0, 7).is_low());
    }
}
