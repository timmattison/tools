//! The export of an edited project: the stages of an export, the settings of
//! a `beta export` run, and the read of the progress that `beta` reports.

use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;

use crate::error::NleError;

/// A stage of an export, for a progress display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportProgress {
    /// The editor renders the project into one recording.
    Rendering,
    /// The rendered recording goes to its file. For a web page or a video,
    /// `beta` runs, and it reported no frame yet.
    Writing,
    /// `beta` wrote `done` of `total` frames.
    Frames {
        /// The number of frames that `beta` wrote.
        done: u64,
        /// The number of frames in the video.
        total: u64,
    },
}

/// A theme of `beta export`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    /// `beta` picks the colors.
    #[default]
    Auto,
    /// The Dracula colors.
    Dracula,
    /// The Monokai colors.
    Monokai,
    /// The dark Solarized colors.
    SolarizedDark,
    /// The light Solarized colors.
    SolarizedLight,
}

impl Theme {
    /// Each theme, in the order of a menu.
    pub const ALL: [Theme; 5] = [
        Theme::Auto,
        Theme::Dracula,
        Theme::Monokai,
        Theme::SolarizedDark,
        Theme::SolarizedLight,
    ];

    /// The name that `beta --theme` takes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        ""
    }
}

impl FromStr for Theme {
    type Err = NleError;

    /// Read a theme from the name that `beta --theme` takes.
    ///
    /// # Errors
    /// Returns `NleError::ExportError` for any other text.
    fn from_str(_text: &str) -> Result<Self, Self::Err> {
        Err(NleError::ExportError(String::new()))
    }
}

/// A video size in pixels. Both numbers are above zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    width: NonZeroU32,
    height: NonZeroU32,
}

impl FromStr for Resolution {
    type Err = NleError;

    /// Read `WIDTHxHEIGHT`, with a lowercase `x`, whole numbers above zero,
    /// and no spaces.
    ///
    /// # Errors
    /// Returns `NleError::ExportError` for any other text.
    fn from_str(_text: &str) -> Result<Self, Self::Err> {
        Err(NleError::ExportError(String::new()))
    }
}

impl fmt::Display for Resolution {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ok(())
    }
}

/// The text that starts a frame line of `beta`.
const FRAME_LINE_PREFIX: &str = "Processed frame ";
/// The text between the two numbers of a frame line of `beta`.
const FRAME_LINE_SEPARATOR: &str = " / ";

/// Read one line of the stdout of `beta`.
///
/// `Processed frame N / M` gives `Frames { done: N, total: M }` when both
/// numbers are whole, `M` is above zero, and `N` is not above `M`. Trailing
/// white space, such as a `\r`, is ignored. Any other line gives `None`.
pub(crate) fn parse_progress_line(line: &str) -> Option<ExportProgress> {
    let numbers = line.trim_end().strip_prefix(FRAME_LINE_PREFIX)?;
    let (done, total) = numbers.split_once(FRAME_LINE_SEPARATOR)?;
    let done: u64 = done.parse().ok()?;
    let total: u64 = total.parse().ok()?;
    (total > 0 && done <= total).then_some(ExportProgress::Frames { done, total })
}

#[cfg(test)]
mod tests;
