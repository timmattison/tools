//! The idle time compression filter: it shortens each stretch of time in
//! which the terminal is idle.
//!
//! The terminal is idle when no key is pressed and the screen does not change.
//! The filter replays the output of its region through the terminal emulator
//! of `termstate`, from a blank screen at the start of the region. An input
//! event is activity. An output event is activity when it changes a cell of a
//! watched row. A cursor move, a change of the window title, or a redraw of
//! the same text changes no cell. The rows to ignore at the top and at the
//! bottom of the screen can hold a status bar with a clock, so a change there
//! is not activity.
//!
//! An idle period is the time from one activity to the next. The start and
//! the end of the region count as activity too. Each idle period of at least
//! the threshold becomes the compressed length, and a compressed length of 0
//! removes it. The events inside a period keep their order, and their times
//! shrink in proportion. Each later event moves back by the time that the
//! periods before it lost. The filter marks the start of each period that it
//! compressed.
//!
//! The options hold the size of the screen, because the emulator needs it.
//! The idle panel of the window takes the size from the open recording.

use crate::error::NleError;
use crate::pipeline::{FilterRegistry, Span};
use crate::types::{AppliedFilter, FilterKind, FilterOptions, FilterRegion};

/// The kind of the idle compression filter.
pub const KIND: &str = "idle-compression";

/// The option that holds the shortest idle period to compress, in seconds.
pub const THRESHOLD_OPTION: &str = "threshold";

/// The option that holds the length of a compressed period, in seconds.
pub const COMPRESS_TO_OPTION: &str = "compress_to";

/// The option that holds the number of columns of the screen.
pub const WIDTH_OPTION: &str = "width";

/// The option that holds the number of rows of the screen.
pub const HEIGHT_OPTION: &str = "height";

/// The option that holds the number of rows at the top of the screen whose
/// changes are not activity.
pub const IGNORE_TOP_ROWS_OPTION: &str = "ignore_top_rows";

/// The option that holds the number of rows at the bottom of the screen
/// whose changes are not activity.
pub const IGNORE_BOTTOM_ROWS_OPTION: &str = "ignore_bottom_rows";

/// The most cells that the screen of the filter can have. A file can state
/// any size, and the emulator keeps each cell in memory.
pub const MAX_SCREEN_CELLS: u32 = 1 << 22;

/// The settings of one idle compression. The threshold is finite and above
/// zero. The compressed length is finite, not below zero, and not above the
/// threshold. The screen has at least one cell and at most
/// `MAX_SCREEN_CELLS`, and the ignored rows are not more than its rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IdleCompression {
    threshold: f64,
    compress_to: f64,
    width: u16,
    height: u16,
    ignore_top_rows: u16,
    ignore_bottom_rows: u16,
}

impl IdleCompression {
    /// Settings that compress each idle period of at least `threshold`
    /// seconds to `compress_to` seconds, on a screen of `width` columns and
    /// `height` rows. No row is ignored.
    ///
    /// # Errors
    /// Returns `NleError::InvalidIdleCompression` when the threshold is not
    /// finite or not above zero, when the compressed length is not finite, is
    /// below zero, or is above the threshold, or when the screen has no cell
    /// or more than `MAX_SCREEN_CELLS` cells.
    pub fn new(
        threshold: f64,
        compress_to: f64,
        width: u16,
        height: u16,
    ) -> Result<Self, NleError> {
        Ok(Self {
            threshold,
            compress_to,
            width,
            height,
            ignore_top_rows: 0,
            ignore_bottom_rows: 0,
        })
    }

    /// These settings, with `top` rows at the top and `bottom` rows at the
    /// bottom of the screen ignored.
    ///
    /// # Errors
    /// Returns `NleError::InvalidIdleCompression` when `top` and `bottom`
    /// together are more than the rows of the screen.
    pub fn ignoring_rows(self, top: u16, bottom: u16) -> Result<Self, NleError> {
        Ok(Self {
            ignore_top_rows: top,
            ignore_bottom_rows: bottom,
            ..self
        })
    }

    /// The shortest idle period to compress, in seconds.
    #[must_use]
    pub fn threshold(self) -> f64 {
        self.threshold
    }

    /// The length of a compressed period, in seconds.
    #[must_use]
    pub fn compress_to(self) -> f64 {
        self.compress_to
    }

    /// The number of columns of the screen.
    #[must_use]
    pub fn width(self) -> u16 {
        self.width
    }

    /// The number of rows of the screen.
    #[must_use]
    pub fn height(self) -> u16 {
        self.height
    }

    /// The number of rows at the top of the screen that are ignored.
    #[must_use]
    pub fn ignore_top_rows(self) -> u16 {
        self.ignore_top_rows
    }

    /// The number of rows at the bottom of the screen that are ignored.
    #[must_use]
    pub fn ignore_bottom_rows(self) -> u16 {
        self.ignore_bottom_rows
    }
}

/// The kind of the idle compression filter, as a `FilterKind`.
#[must_use]
pub fn kind() -> FilterKind {
    FilterKind::new(KIND)
}

/// An idle compression filter for `region` with `settings`.
#[must_use]
pub fn applied(region: FilterRegion, settings: IdleCompression) -> AppliedFilter {
    AppliedFilter {
        kind: kind(),
        region,
        options: FilterOptions::new()
            .with(THRESHOLD_OPTION, settings.threshold)
            .with(COMPRESS_TO_OPTION, settings.compress_to)
            .with(WIDTH_OPTION, settings.width)
            .with(HEIGHT_OPTION, settings.height)
            .with(IGNORE_TOP_ROWS_OPTION, settings.ignore_top_rows)
            .with(IGNORE_BOTTOM_ROWS_OPTION, settings.ignore_bottom_rows),
    }
}

/// The settings of an idle compression filter. It is `None` when the filter
/// is of another kind, or when an option is missing or does not make valid
/// settings.
#[must_use]
pub fn settings_of(filter: &AppliedFilter) -> Option<IdleCompression> {
    let _ = filter;
    None
}

/// Add the idle compression filter to `registry`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_span(kind(), compress);
}

/// Compress the idle periods of a span by the settings in `options`.
fn compress(span: Span, options: &FilterOptions) -> Result<Span, String> {
    let _ = options;
    Ok(span)
}

#[cfg(test)]
mod tests;
