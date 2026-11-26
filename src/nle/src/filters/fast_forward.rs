//! The fast-forward filter: it compresses a region that holds a long burst of
//! command output, for example a build, an install, or a test run.
//!
//! Issue #162 names three modes. Skip to end jumps straight to the final state
//! of the region. Accelerate plays the region at 10 to 50 times its speed.
//! Summary shows the first lines, then "...", then the last lines. This module
//! gives skip to end and accelerate.

use crate::error::NleError;
use crate::filters::time_dilation::Speed;
use crate::pipeline::{FilterRegistry, Span};
use crate::types::{AppliedFilter, Event, FilterKind, FilterOptions, FilterRegion};

/// The kind of the fast-forward filter.
pub const KIND: &str = "fast-forward";

/// The option that holds the mode.
pub const MODE_OPTION: &str = "mode";

/// The option that holds the acceleration. The time dilation filter spells its
/// speed option the same way.
pub const SPEED_OPTION: &str = super::time_dilation::SPEED_OPTION;

/// The value of the `mode` option for skip to end.
pub const SKIP_TO_END: &str = "skip-to-end";

/// The value of the `mode` option for accelerate.
pub const ACCELERATE: &str = "accelerate";

/// An acceleration factor. It is finite and above 1. A fast-forward never
/// slows a region, because the time dilation filter does slow-downs.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Acceleration(Speed);

impl Acceleration {
    /// Make an acceleration from a number.
    ///
    /// # Errors
    /// Returns `NleError::InvalidAcceleration` when `value` is NaN, infinite,
    /// or not above 1.
    pub fn new(value: f64) -> Result<Self, NleError> {
        if value > 1.0 {
            Speed::new(value)
                .map(Self)
                .map_err(|_| NleError::InvalidAcceleration(value))
        } else {
            Err(NleError::InvalidAcceleration(value))
        }
    }

    /// The acceleration as a number.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0.get()
    }

    /// The acceleration as a `Speed`.
    #[must_use]
    pub fn speed(self) -> Speed {
        self.0
    }
}

/// How a fast-forward filter compresses its region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// Jump straight to the final state of the region. Each event of the
    /// region moves to the start of the region, and the region has no length.
    SkipToEnd,
    /// Play the region at the given acceleration. The times and the length of
    /// the region divide by it.
    Accelerate(Acceleration),
}

/// The kind of the fast-forward filter, as a `FilterKind`.
#[must_use]
pub fn kind() -> FilterKind {
    FilterKind::new(KIND)
}

/// A fast-forward filter for `region` in `mode`.
#[must_use]
pub fn applied(region: FilterRegion, mode: Mode) -> AppliedFilter {
    let options = match mode {
        Mode::SkipToEnd => FilterOptions::new().with(MODE_OPTION, SKIP_TO_END),
        Mode::Accelerate(acceleration) => FilterOptions::new()
            .with(MODE_OPTION, ACCELERATE)
            .with(SPEED_OPTION, acceleration.get()),
    };
    AppliedFilter {
        kind: kind(),
        region,
        options,
    }
}

/// Add the fast-forward filter to `registry`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_span(kind(), fast_forward);
}

/// Compress a span in the mode of its options.
fn fast_forward(span: Span, options: &FilterOptions) -> Result<Span, String> {
    if options.get(MODE_OPTION).and_then(serde_json::Value::as_str) == Some(SKIP_TO_END) {
        return Ok(skip_to_end(span));
    }
    Ok(span)
}

/// Move each event of a span to the start of the span, and remove the length.
/// Each event keeps its data, its type, and its place in the list.
fn skip_to_end(span: Span) -> Span {
    Span {
        events: span
            .events
            .into_iter()
            .map(|event| Event { time: 0.0, ..event })
            .collect(),
        length: 0.0,
    }
}

#[cfg(test)]
mod tests;
