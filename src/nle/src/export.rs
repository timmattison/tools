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
        match self {
            Self::Auto => "auto",
            Self::Dracula => "dracula",
            Self::Monokai => "monokai",
            Self::SolarizedDark => "solarized-dark",
            Self::SolarizedLight => "solarized-light",
        }
    }
}

impl FromStr for Theme {
    type Err = NleError;

    /// Read a theme from the name that `beta --theme` takes.
    ///
    /// # Errors
    /// Returns `NleError::ExportError` for any other text.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|theme| theme.as_str() == text)
            .ok_or_else(|| {
                NleError::ExportError(format!(
                    "the theme \"{text}\" is not one of auto, dracula, monokai, solarized-dark, solarized-light"
                ))
            })
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
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let parse_error = || {
            NleError::ExportError(format!(
                "the resolution \"{text}\" is not WIDTHxHEIGHT, for example 1920x1080"
            ))
        };
        let (width, height) = text.split_once('x').ok_or_else(parse_error)?;
        Ok(Self {
            width: parse_dimension(width).ok_or_else(parse_error)?,
            height: parse_dimension(height).ok_or_else(parse_error)?,
        })
    }
}

/// Read one side of a resolution: digits only, and above zero.
fn parse_dimension(text: &str) -> Option<NonZeroU32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl fmt::Display for Resolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
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
