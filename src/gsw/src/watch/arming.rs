//! The arming of a key that needs a second press.
//!
//! This module holds `Arming`. A key that asks for a second press (`G` on a
//! remote shell, Esc off the start branch) puts a message on the row. That
//! message is the armed state, and `Arming` holds it.

use std::time::{Duration, Instant};

/// The armed state of a key that needs a second press.
///
/// **The message on the row is the armed state.** So this holds the instant
/// the message took the row, and not a flag. The message goes away by itself
/// after one lifetime, and the arming ends at the same instant. The screen and
/// the key then say one thing: a press a minute later asks again.
///
/// **The instant is the one the message took the row at, and not the one the
/// key was pressed at.** The row is not always free. A question and a push in
/// flight own it, and `crate::push::PushUi` holds a message that arrives then.
/// A held message is a message nobody has read, so an arming that started at
/// the press would offer a second press against a warning still sitting in the
/// queue. The caller therefore calls [`Arming::arm`] only for the answer that
/// says the words reached the screen.
///
/// The age is saturating for the reason the age of a status message in
/// `crate::push` is: `now` comes from the loop's injected clock, and a clock
/// that a test drives backwards reports the zero age it plainly has, and does
/// not underflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Arming {
    /// How long the message stays on the row, and so how long the arming lasts.
    lifetime: Duration,
    /// When the message took the row, or `None` when no message stands.
    posted_at: Option<Instant>,
}

impl Arming {
    /// An arming that is not armed and that lasts `lifetime` once armed.
    pub(super) const fn new(lifetime: Duration) -> Self {
        Self {
            lifetime,
            posted_at: None,
        }
    }

    /// The message took the row at `now`, so the key is armed from `now`.
    pub(super) fn arm(&mut self, now: Instant) {
        let _ = now;
    }

    /// Whether the message still stands at `now`.
    pub(super) fn is_armed(&self, now: Instant) -> bool {
        let _ = (now, self.lifetime, self.posted_at);
        false
    }

    /// Take the arming away at once.
    pub(super) fn disarm(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIFETIME: Duration = Duration::from_secs(3);
    const MILLISECOND: Duration = Duration::from_millis(1);

    /// A new arming is not armed.
    #[test]
    fn a_new_arming_is_not_armed() {
        let now = Instant::now();
        assert!(!Arming::new(LIFETIME).is_armed(now));
    }

    /// An armed arming is armed at the instant it was armed.
    #[test]
    fn an_arming_is_armed_at_the_instant_it_took_the_row() {
        let now = Instant::now();
        let mut arming = Arming::new(LIFETIME);
        arming.arm(now);
        assert!(arming.is_armed(now));
    }

    /// The arming lasts until one millisecond before its lifetime ends, and
    /// the lifetime itself is already past it.
    #[test]
    fn an_arming_ends_at_exactly_its_lifetime() {
        let now = Instant::now();
        let mut arming = Arming::new(LIFETIME);
        arming.arm(now);
        assert!(arming.is_armed(now + LIFETIME - MILLISECOND));
        assert!(!arming.is_armed(now + LIFETIME));
        assert!(!arming.is_armed(now + LIFETIME + MILLISECOND));
    }

    /// A disarm ends the arming at once, at the instant it was armed.
    #[test]
    fn a_disarm_ends_the_arming_at_once() {
        let now = Instant::now();
        let mut arming = Arming::new(LIFETIME);
        arming.arm(now);
        arming.disarm();
        assert!(!arming.is_armed(now));
    }

    /// A clock that a test drives backwards reads as the zero age it plainly
    /// has, so the arming still stands.
    #[test]
    fn a_clock_that_runs_backwards_reads_as_armed() {
        let now = Instant::now() + Duration::from_secs(60);
        let mut arming = Arming::new(LIFETIME);
        arming.arm(now);
        assert!(arming.is_armed(now - Duration::from_secs(30)));
    }

    /// The lifetime is a field: two armings end each at their own lifetime.
    #[test]
    fn each_arming_ends_at_its_own_lifetime() {
        let now = Instant::now();
        let long = Duration::from_secs(60);
        let mut short_arming = Arming::new(LIFETIME);
        let mut long_arming = Arming::new(long);
        short_arming.arm(now);
        long_arming.arm(now);
        assert!(short_arming.is_armed(now + LIFETIME - MILLISECOND));
        assert!(!short_arming.is_armed(now + LIFETIME));
        assert!(long_arming.is_armed(now + LIFETIME));
        assert!(long_arming.is_armed(now + long - MILLISECOND));
        assert!(!long_arming.is_armed(now + long));
    }
}
