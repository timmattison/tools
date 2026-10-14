//! What the status line of `ha` shows: the last refresh, the rate limit of
//! the GitHub API, and the errors that are not about one repository.

use std::time::Duration;

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

/// How long a [`Message`] stays on the status line. A key press removes it
/// sooner.
pub const MESSAGE_LIFETIME: Duration = Duration::from_secs(30);

/// A one-off message on the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The text that the status line shows.
    pub text: String,
    /// When the message was posted. The message ages from this instant.
    pub posted_at: DateTime<Utc>,
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
    /// The owners whose repository list failed or was cut, with the text to show
    /// for each: the error of a failed list, or the warning of a cut list.
    pub owner_errors: Vec<(OwnerName, String)>,
    /// A one-off message, for example the address after a failed open. It
    /// fades out over [`MESSAGE_LIFETIME`], then leaves. A key press removes
    /// it sooner. Read it with [`Status::message_at`].
    pub message: Option<Message>,
}

impl Status {
    /// Returns the message that is live at `now`, or `None`.
    ///
    /// There is no live message when none was posted, or when it is
    /// [`MESSAGE_LIFETIME`] old or older. A message that is posted in the
    /// future of `now` is live. Every reader of the message uses this method,
    /// so the rule for the end of its life is in one place.
    #[must_use]
    pub fn message_at(&self, now: DateTime<Utc>) -> Option<&Message> {
        self.message.as_ref().filter(|message| {
            (now - message.posted_at)
                .to_std()
                .map_or(true, |age| age < MESSAGE_LIFETIME)
        })
    }
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
