//! The timeline of the keystroke HUD: it groups the presses of keys into
//! boxes and gives the times of each box.

use super::keys::Key;

/// The number of fade steps for each second of fade.
pub(crate) const FADE_STEPS_PER_SECOND: f64 = 30.0;
/// The most fade steps of one fade, so that a long fade does not make a frame
/// list without end.
pub(crate) const MAX_FADE_STEPS: u32 = 300;
/// The label of the key that closes a box.
const CLOSING_KEY: &str = "Enter";
/// The label of a piece of text made only of spaces.
const SPACE_LABEL: &str = "Space";

/// One press of a key, at a time relative to the start of the region.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Press {
    /// The time of the press, in seconds.
    pub(crate) time: f64,
    /// The key that the user pressed.
    pub(crate) key: Key,
}

/// One piece of the label of a box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Piece {
    /// Typed text.
    Text(String),
    /// A special key, pressed `count` times in a row.
    Special {
        /// The label of the key.
        label: String,
        /// The number of presses in a row.
        count: u32,
    },
}

/// What the HUD shows at one time: the pieces of the label and the opacity,
/// above 0 and at most 1.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shown {
    /// The pieces of the label.
    pub(crate) pieces: Vec<Piece>,
    /// The opacity of the box.
    pub(crate) alpha: f64,
}

/// The timing of the HUD. `limit` is the length of the region: nothing shows
/// at or after it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Timing {
    /// How long a box stays after its last press, in seconds.
    pub(crate) duration: f64,
    /// How long a box takes to fade in and to fade out, in seconds.
    pub(crate) fade: f64,
    /// The length of the region, in seconds.
    pub(crate) limit: f64,
}

/// The boxes of the HUD and their times.
pub(crate) struct Timeline {
    timing: Timing,
}

impl Timeline {
    /// Make the timeline of `presses`.
    pub(crate) fn new(presses: Vec<Press>, timing: Timing) -> Self {
        let _ = presses;
        Self { timing }
    }

    /// What the HUD shows at `time`, or `None`.
    pub(crate) fn at(&self, time: f64) -> Option<Shown> {
        let _ = (time, self.timing);
        None
    }

    /// The times at which what the HUD shows changes, in time order, with no
    /// two equal.
    pub(crate) fn frames(&self) -> Vec<f64> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests;
