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

/// The pause threshold of a filter whose options give none, in seconds.
pub const DEFAULT_PAUSE: f64 = 2.0;

/// A typing rate in characters per second. It is finite and above zero.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct TypingRate(f64);

impl TypingRate {
    /// Make a typing rate from a number of characters per second.
    ///
    /// # Errors
    /// Returns `NleError::InvalidTypingRate` when `value` is NaN, infinite,
    /// zero, or below zero.
    pub fn new(value: f64) -> Result<Self, NleError> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(NleError::InvalidTypingRate(value))
        }
    }

    /// The rate as a number of characters per second.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

/// The jitter of the interval between two keys: the largest change of an
/// interval, as a fraction of the interval. It is from 0 up to, and not
/// including, 1, so each interval stays above zero.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Jitter(f64);

impl Jitter {
    /// No jitter: each interval is the same.
    pub const NONE: Self = Self(0.0);

    /// Make a jitter from a fraction of the interval.
    ///
    /// # Errors
    /// Returns `NleError::InvalidJitter` when `value` is NaN, below zero, or
    /// not below 1.
    pub fn new(value: f64) -> Result<Self, NleError> {
        if (0.0..1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(NleError::InvalidJitter(value))
        }
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

/// Read a number option. The answer is `None` when the option is missing, and
/// an error when it is not a number.
fn optional_number(options: &FilterOptions, key: &str) -> Result<Option<f64>, String> {
    options
        .get(key)
        .map(|value| {
            value
                .as_f64()
                .ok_or_else(|| format!("the option \"{key}\" is not a number"))
        })
        .transpose()
}

/// The text for an option whose value is not valid.
fn invalid(key: &str, error: &NleError) -> String {
    format!("the option \"{key}\" is not valid: {error}")
}

impl Normalization {
    /// Read the settings from the options of a filter. The rate is
    /// necessary. A missing pause threshold is `DEFAULT_PAUSE`, and a missing
    /// jitter is `Jitter::NONE`.
    fn from_options(options: &FilterOptions) -> Result<Self, String> {
        let rate = options
            .get(RATE_OPTION)
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| format!("the option \"{RATE_OPTION}\" is missing or is not a number"))?;
        let pause = optional_number(options, PAUSE_OPTION)?.unwrap_or(DEFAULT_PAUSE);
        let jitter = optional_number(options, JITTER_OPTION)?.unwrap_or(Jitter::NONE.get());
        Ok(Self {
            rate: TypingRate::new(rate).map_err(|error| invalid(RATE_OPTION, &error))?,
            pause: Seconds::new(pause).map_err(|error| invalid(PAUSE_OPTION, &error))?,
            jitter: Jitter::new(jitter).map_err(|error| invalid(JITTER_OPTION, &error))?,
        })
    }
}

/// Put the keys of each burst at the rate, and move the other events with
/// them.
fn normalize(span: Span, options: &FilterOptions) -> Result<Span, String> {
    let settings = Normalization::from_options(options)?;
    let keys = new_keys(&span.events, &settings);
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

/// A key of the region: its time in the recording, its new time, and whether
/// it holds a line end.
#[derive(Debug, Clone, Copy)]
struct Key {
    old: f64,
    new: f64,
    ends_line: bool,
}

/// The characters that end a line: Enter sends a carriage return in raw
/// mode, and a line feed in other modes.
const LINE_ENDS: [char; 2] = ['\r', '\n'];

/// The keys of `events`, in order. The first key keeps its time. A gap
/// longer than the pause threshold, and the gap after a key that holds a line
/// end, keep their length. Each other key comes one interval after the key
/// before it. The interval is one character at the rate, changed by the
/// jitter.
fn new_keys(events: &[Event], settings: &Normalization) -> Vec<Key> {
    let interval = 1.0 / settings.rate.get();
    let mut keys: Vec<Key> = Vec::new();
    for event in events {
        if event.event_type != EventType::Input {
            continue;
        }
        let new = keys.last().map_or(event.time, |before| {
            let gap = event.time - before.old;
            before.new
                + if before.ends_line || gap > settings.pause.get() {
                    gap
                } else {
                    interval * settings.jitter.get().mul_add(noise(keys.len()), 1.0)
                }
        });
        keys.push(Key {
            old: event.time,
            new,
            ends_line: event.data.contains(LINE_ENDS),
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

/// The multiplier of SplitMix64: the golden ratio in 64 bits.
const SPLITMIX_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// 2 to the power 32, the count of the values of a `u32`.
const TWO_TO_THE_32: f64 = 4_294_967_296.0;

/// A number from -1 up to, and not including, 1. It depends only on `index`.
///
/// The filter is a pure function, so its jitter comes from the position of a
/// key and not from a random source. Thus each render gives the same times.
/// The number is the output of SplitMix64 for the step `index`.
fn noise(index: usize) -> f64 {
    let step = u64::try_from(index).unwrap_or(u64::MAX).wrapping_add(1);
    let mut z = step.wrapping_mul(SPLITMIX_GAMMA);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // The top 32 bits, as a fraction from 0 up to, and not including, 1.
    let fraction = f64::from(u32::try_from(z >> 32).unwrap_or(u32::MAX)) / TWO_TO_THE_32;
    fraction.mul_add(2.0, -1.0)
}

#[cfg(test)]
mod tests;
