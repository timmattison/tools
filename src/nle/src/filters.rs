//! The built-in filters of the editor (issue #154).
//!
//! Each filter is registered with `FilterRegistry::register_typed`, so its
//! options have a type, and the render refuses options that break a rule of
//! that type. The keys of the options are the field names below, and they
//! are a part of the project file format.

use serde::Deserialize;

use crate::pipeline::{FilterRegistry, Span};
use crate::types::{FilterKind, Seconds};

/// The kind of the filter that shortens the long pauses between events.
///
/// Options: `threshold` and `max_pause`, in seconds. Each gap between two
/// events that is longer than `threshold` becomes `max_pause` long.
/// `max_pause` must not be greater than `threshold`. A gap that crosses the
/// start or the end of the region stays, and so does the time after the last
/// event.
pub const PAUSE_REMOVAL: &str = "pause-removal";

/// The kind of the filter that puts a pause into the timeline.
pub const PAUSE_INSERTION: &str = "pause-insertion";

/// A registry that holds each built-in filter under its kind.
#[must_use]
pub fn builtin() -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    registry.register_typed(FilterKind::new(PAUSE_REMOVAL), remove_pauses);
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

#[cfg(test)]
mod tests;
