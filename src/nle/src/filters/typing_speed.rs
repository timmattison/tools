//! The typing speed filter: it plays each burst of typing at one rate.

use crate::error::NleError;
use crate::pipeline::{FilterRegistry, Span};
use crate::types::{
    AppliedFilter, Event, EventType, FilterKind, FilterOptions, FilterRegion, Seconds,
};

/// The kind of the typing speed filter.
pub const KIND: &str = "typing-speed";

/// The option that holds the typing rate, in characters per second.
pub const RATE_OPTION: &str = "chars_per_second";

/// The option that holds the pause threshold, in seconds.
pub const PAUSE_OPTION: &str = "pause";

/// The option that holds the jitter.
pub const JITTER_OPTION: &str = "jitter";

/// A typing rate in characters per second.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct TypingRate(f64);

impl TypingRate {
    /// Make a typing rate from a number of characters per second.
    ///
    /// # Errors
    /// None yet.
    pub fn new(value: f64) -> Result<Self, NleError> {
        Ok(Self(value))
    }

    /// The rate as a number of characters per second.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

/// The jitter of the interval between two keystrokes.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Jitter(f64);

impl Jitter {
    /// No jitter: each interval is the same.
    pub const NONE: Self = Self(0.0);

    /// Make a jitter from a fraction of the interval.
    ///
    /// # Errors
    /// None yet.
    pub fn new(value: f64) -> Result<Self, NleError> {
        Ok(Self(value))
    }

    /// The jitter as a fraction of the interval.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

/// The settings of one typing speed filter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Normalization {
    /// The rate of the keystrokes of each burst.
    pub rate: TypingRate,
    /// The pause threshold.
    pub pause: Seconds,
    /// The jitter of each interval.
    pub jitter: Jitter,
}

/// The kind of the typing speed filter, as a `FilterKind`.
#[must_use]
pub fn kind() -> FilterKind {
    FilterKind::new(KIND)
}

/// A typing speed filter for `region` with `normalization`.
#[must_use]
pub fn applied(region: FilterRegion, normalization: Normalization) -> AppliedFilter {
    AppliedFilter {
        kind: kind(),
        region,
        options: FilterOptions::new()
            .with(RATE_OPTION, normalization.rate.get())
            .with(PAUSE_OPTION, normalization.pause.get())
            .with(JITTER_OPTION, normalization.jitter.get()),
    }
}

/// Add the typing speed filter to `registry`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_span(kind(), normalize);
}

/// Put each key after the first one interval after the key before it.
fn normalize(span: Span, options: &FilterOptions) -> Result<Span, String> {
    let rate = options
        .get(RATE_OPTION)
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| format!("the option \"{RATE_OPTION}\" is missing or is not a number"))?;
    let interval = 1.0 / rate;
    let mut previous: Option<f64> = None;
    // The change of the time of the last key so far.
    let mut shift = 0.0;
    let events = span
        .events
        .into_iter()
        .map(|event| {
            if event.event_type != EventType::Input {
                return Event {
                    time: event.time + shift,
                    ..event
                };
            }
            let time = previous.map_or(event.time, |before| before + interval);
            previous = Some(time);
            shift = time - event.time;
            Event { time, ..event }
        })
        .collect();
    Ok(Span {
        events,
        length: span.length + shift,
    })
}

#[cfg(test)]
mod tests;
