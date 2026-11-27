//! The clock of the preview: how the playhead moves while a recording plays.
//! The functions are pure, so a test needs no timer.

use std::time::Duration;

use crate::types::Seconds;

/// The playhead after `elapsed` of playback from `at` on a timeline that ends
/// at `end`, and `true` when it reached the end. The playhead never goes past
/// `end`.
#[must_use]
pub fn advance(at: Seconds, elapsed: Duration, end: Seconds) -> (Seconds, bool) {
    // A sum that is not a valid time cannot happen with finite inputs. It
    // gives the end, so the playback stops.
    let next = Seconds::new(at.get() + elapsed.as_secs_f64()).unwrap_or(end);
    if next >= end {
        (end, true)
    } else {
        (next, false)
    }
}

/// Where playback starts when Play is pressed at `at`: the start of the
/// timeline when `at` is at or past `end`, else `at`.
#[must_use]
pub fn play_from(at: Seconds, end: Seconds) -> Seconds {
    if at >= end {
        Seconds::ZERO
    } else {
        at
    }
}

#[cfg(test)]
mod tests;
