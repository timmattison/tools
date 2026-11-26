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
    let keys = new_keys(&span.events, 1.0 / rate);
    let length = new_time(span.length, keys.last().copied(), None);
    let mut upcoming = keys.into_iter().peekable();
    let mut before = None;
    let events = span
        .events
        .into_iter()
        .map(|event| {
            // `new_keys` gives one key for each input event, in order.
            let key = if event.event_type == EventType::Input {
                upcoming.next()
            } else {
                None
            };
            let time = match key {
                Some(key) => {
                    before = Some(key);
                    key.new
                }
                None => new_time(event.time, before, upcoming.peek().copied()),
            };
            Event { time, ..event }
        })
        .collect();
    Ok(Span { events, length })
}

/// A key of the region: its time in the recording, and its new time.
#[derive(Debug, Clone, Copy)]
struct Key {
    old: f64,
    new: f64,
}

/// The keys of `events`, in order. The first key keeps its time, and each
/// next key comes `interval` after the key before it.
fn new_keys(events: &[Event], interval: f64) -> Vec<Key> {
    let mut keys: Vec<Key> = Vec::new();
    for event in events {
        if event.event_type != EventType::Input {
            continue;
        }
        let new = keys
            .last()
            .map_or(event.time, |before| before.new + interval);
        keys.push(Key {
            old: event.time,
            new,
        });
    }
    keys
}

/// The new time of an event at `time` that is not a key. `before` is the last
/// key before it, and `after` is the first key after it.
///
/// An event before the first key keeps its time. Each other event keeps its
/// delay after the key before it. When the gap between `before` and `after`
/// becomes shorter, the delay becomes shorter by the same ratio, so the event
/// stays between the two keys.
fn new_time(time: f64, before: Option<Key>, after: Option<Key>) -> f64 {
    let Some(before) = before else {
        return time;
    };
    let delay = time - before.old;
    let Some(after) = after else {
        return before.new + delay;
    };
    let old_gap = after.old - before.old;
    let new_gap = after.new - before.new;
    let ratio = if old_gap > 0.0 && new_gap < old_gap {
        (new_gap / old_gap).max(0.0)
    } else {
        1.0
    };
    // `max` and `min`, not `clamp`: `clamp` panics when its bounds cross.
    (before.new + delay * ratio).max(before.new).min(after.new)
}

#[cfg(test)]
mod tests;
