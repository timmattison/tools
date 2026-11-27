//! The time dilation filter: it plays a region of a recording faster or
//! slower.
//!
//! A speed of 2 plays the region twice as fast, so the region is half as long.
//! A speed of 0.5 plays it at half speed, so the region is twice as long. The
//! times inside the region scale from the start of the region. The pipeline
//! moves each later event by the change in length, so the recording stays in
//! step. Different regions can have different speeds.

use crate::error::NleError;
use crate::pipeline::{FilterRegistry, Mark, Span, SpanContext};
use crate::types::{AppliedFilter, Event, FilterKind, FilterOptions, FilterRegion};

/// The kind of the time dilation filter.
pub const KIND: &str = "time-dilation";

/// The option that holds the speed.
pub const SPEED_OPTION: &str = "speed";

/// A speed factor. It is finite and above zero. 2 plays a region twice as
/// fast, and 0.5 plays it at half speed.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Speed(f64);

impl Speed {
    /// Make a speed from a number.
    ///
    /// # Errors
    /// Returns `NleError::InvalidSpeed` when `value` is NaN, infinite, zero,
    /// or below zero.
    pub fn new(value: f64) -> Result<Self, NleError> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(NleError::InvalidSpeed(value))
        }
    }

    /// The speed as a number.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

/// The kind of the time dilation filter, as a `FilterKind`.
#[must_use]
pub fn kind() -> FilterKind {
    FilterKind::new(KIND)
}

/// A time dilation filter for `region` at `speed`.
#[must_use]
pub fn applied(region: FilterRegion, speed: Speed) -> AppliedFilter {
    AppliedFilter {
        kind: kind(),
        region,
        options: FilterOptions::new().with(SPEED_OPTION, speed.get()),
    }
}

/// The speed of a time dilation filter. It is `None` when the filter is of
/// another kind, or when its `speed` option is missing, is not a number, or is
/// not a valid speed.
#[must_use]
pub fn speed_of(filter: &AppliedFilter) -> Option<Speed> {
    if filter.kind != kind() {
        return None;
    }
    filter
        .options
        .get(SPEED_OPTION)
        .and_then(serde_json::Value::as_f64)
        .and_then(|value| Speed::new(value).ok())
}

/// Add the time dilation filter to `registry`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_span(kind(), dilate);
}

/// Divide the times of the events and the marks, and the length of a span, by
/// the speed option. Each event keeps its data, its type, and its place in the
/// list.
fn dilate(span: Span, _context: SpanContext<'_>, options: &FilterOptions) -> Result<Span, String> {
    let value = options
        .get(SPEED_OPTION)
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| format!("the option \"{SPEED_OPTION}\" is missing or is not a number"))?;
    let speed = Speed::new(value).map_err(|error| error.to_string())?.get();
    Ok(Span {
        events: span
            .events
            .into_iter()
            .map(|event| Event {
                time: event.time / speed,
                ..event
            })
            .collect(),
        length: span.length / speed,
        marks: span
            .marks
            .into_iter()
            .map(|mark| Mark {
                time: mark.time / speed,
                ..mark
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests;
