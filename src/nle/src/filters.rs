//! The built-in filters of the editor (issue #154).
//!
//! Each filter is registered with `FilterRegistry::register_typed`, so its
//! options have a type, and the render refuses options that break a rule of
//! that type. The keys of the options are the field names below, and they
//! are a part of the project file format.

use serde::Deserialize;

use crate::pipeline::{FilterRegistry, Span};
use crate::types::{Event, FilterKind, Seconds};

/// The kind of the filter that shortens the long pauses between events.
///
/// Options: `threshold` and `max_pause`, in seconds. Each gap between two
/// events that is longer than `threshold` becomes `max_pause` long.
/// `max_pause` must not be greater than `threshold`. A gap that crosses the
/// start or the end of the region stays, and so does the time after the last
/// event.
pub const PAUSE_REMOVAL: &str = "pause-removal";

/// The kind of the filter that puts a pause into the timeline.
///
/// Options: `at` and `duration`, in seconds. `at` is relative to the start of
/// the region. Each event at or after `at` moves later by `duration`, so a
/// pause at the time of a key comes before that key. The end of the timeline
/// moves later too when it is at or after `at`, also when no event moves.
/// The normal region is `All`. In a range, the events after the range move by
/// the shift, thus they move only when an event of the range moves.
pub const PAUSE_INSERTION: &str = "pause-insertion";

/// The kind of the filter that cuts the silence at the start and at the end.
pub const AUTO_TRIM: &str = "auto-trim";

/// A registry that holds each built-in filter under its kind.
#[must_use]
pub fn builtin() -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    registry.register_typed(FilterKind::new(PAUSE_REMOVAL), remove_pauses);
    registry.register_typed(FilterKind::new(PAUSE_INSERTION), insert_pause);
    registry
}

/// The options of pause removal, as the project file holds them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PauseRemovalFile {
    threshold: Seconds,
    max_pause: Seconds,
}

/// The options of pause removal, after the check that no pause gets longer.
#[derive(Deserialize)]
#[serde(try_from = "PauseRemovalFile")]
struct PauseRemoval {
    threshold: f64,
    max_pause: f64,
}

impl TryFrom<PauseRemovalFile> for PauseRemoval {
    type Error = String;

    fn try_from(file: PauseRemovalFile) -> Result<Self, String> {
        let (threshold, max_pause) = (file.threshold.get(), file.max_pause.get());
        if max_pause > threshold {
            return Err(format!(
                "max_pause {max_pause} s is greater than threshold {threshold} s, so it makes a pause longer"
            ));
        }
        Ok(Self {
            threshold,
            max_pause,
        })
    }
}

/// Shorten each gap between two events of `span` that is longer than the
/// threshold to the max pause.
///
/// The gaps are measured in time order. The first event keeps its time, and
/// each later event moves earlier by the time that the gaps before it lost.
/// The answer sets no end, so the end moves by the shift, and the time after
/// the last event stays.
fn remove_pauses(span: Span, options: &PauseRemoval) -> Span {
    let mut events = span.events;
    // A stable sort: events at one time keep their order.
    events.sort_by(|a, b| a.time.total_cmp(&b.time));
    let mut removed = 0.0;
    // The old and the new time of the event before.
    let mut previous: Option<(f64, f64)> = None;
    for event in &mut events {
        let old = event.time;
        let new = match previous {
            None => old,
            Some((previous_old, previous_new)) => {
                let gap = old - previous_old;
                if gap > options.threshold {
                    removed += gap - options.max_pause;
                }
                // A rounding error must not put an event before the event
                // that was ahead of it.
                (old - removed).max(previous_new)
            }
        };
        event.time = new;
        previous = Some((old, new));
    }
    Span { events, end: None }
}

/// The options of pause insertion.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PauseInsertion {
    at: Seconds,
    duration: Seconds,
}

/// Put a pause of the duration at the point `at` of `span`.
///
/// Each event at or after the point moves later by the duration. When the
/// filter sees the end of the timeline, and that end is at or after the
/// point, the answer sets the end later by the duration. Else the end stays,
/// or it moves by the shift when the filter does not see it.
fn insert_pause(span: Span, options: &PauseInsertion) -> Span {
    let (at, duration) = (options.at.get(), options.duration.get());
    let later = |time: f64| if time >= at { time + duration } else { time };
    Span {
        events: span
            .events
            .into_iter()
            .map(|event| Event {
                time: later(event.time),
                ..event
            })
            .collect(),
        end: span.end.map(later),
    }
}

#[cfg(test)]
mod tests;
