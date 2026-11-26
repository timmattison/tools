//! The built-in filters of the editor (issue #154).
//!
//! Each filter is registered with `FilterRegistry::register_typed`, so its
//! options have a type, and the render refuses options that break a rule of
//! that type. The keys of the options are the field names below, and they
//! are a part of the project file format.

use serde::Deserialize;

use crate::pipeline::{FilterRegistry, Span};
use crate::types::{Event, EventType, FilterKind, Seconds};

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
/// The normal region is `All`. In a range, each event after the range moves
/// later by `duration`, also when no event of the range moves. Thus `at` must
/// not be past the end of a range. The events between the end of the range and
/// such a point are before the point, but they move, so the render refuses the
/// point.
pub const PAUSE_INSERTION: &str = "pause-insertion";

/// The kind of the filter that cuts the silence at the start and at the end.
///
/// Options: `leading` and `trailing`, each `true` when the options do not name
/// it. The leading silence is the time before the first input event: the
/// first input event moves to the start of the region, each later event moves
/// back by the same time, and each output event before it moves to the start,
/// in its order. Thus the screen that the user saw before the first key is
/// still there. The trailing silence is the time after the last output event:
/// each event after it moves back to it, and the end of the timeline comes
/// back to it. A region with no input event keeps its start, and a region
/// with no output event keeps its end. On a range, the events after the range
/// move by the shift of step 6 on `FilterFn`.
pub const AUTO_TRIM: &str = "auto-trim";

/// A registry that holds each built-in filter under its kind.
#[must_use]
pub fn builtin() -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    registry.register_typed(FilterKind::new(PAUSE_REMOVAL), |span, options| {
        Ok(remove_pauses(span, options))
    });
    registry.register_typed(FilterKind::new(PAUSE_INSERTION), insert_pause);
    registry.register_typed(FilterKind::new(AUTO_TRIM), |span, options| {
        Ok(trim_silence(span, options))
    });
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
    let length = span.length;
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
    Span {
        events,
        end: None,
        shift: None,
        length,
    }
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
/// or it moves by the shift when the filter does not see it. The answer sets
/// the shift to the duration, so each event after the region moves later by
/// it, also when no event of the region moves. A point past the length of the
/// region is an error, because the shift also moves the events after the region
/// that are before the point.
fn insert_pause(span: Span, options: &PauseInsertion) -> Result<Span, String> {
    let (at, duration) = (options.at.get(), options.duration.get());
    if at > span.length {
        return Err(format!(
            "at {at} s is past the end of the region, which is {} s long",
            span.length
        ));
    }
    let later = |time: f64| if time >= at { time + duration } else { time };
    Ok(Span {
        events: span
            .events
            .into_iter()
            .map(|event| Event {
                time: later(event.time),
                ..event
            })
            .collect(),
        end: span.end.map(later),
        shift: Some(duration),
        length: span.length,
    })
}

/// The options of auto-trim.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AutoTrim {
    #[serde(default = "on")]
    leading: bool,
    #[serde(default = "on")]
    trailing: bool,
}

/// The value of an auto-trim option that the options do not name.
fn on() -> bool {
    true
}

/// The times of the events of `events` that have the type `event_type`.
fn times_of<'a>(events: &'a [Event], event_type: &'a EventType) -> impl Iterator<Item = f64> + 'a {
    events
        .iter()
        .filter(move |event| event.event_type == *event_type)
        .map(|event| event.time)
}

/// Cut the silence at the two ends of `span`, by the rules on `AUTO_TRIM`.
///
/// When the filter sees the end of the timeline, the answer sets it: back by
/// the time of the first input event, then to the time of the last output
/// event. Else the end moves by the shift.
fn trim_silence(span: Span, options: &AutoTrim) -> Span {
    let Span {
        mut events,
        mut end,
        shift: _,
        length,
    } = span;
    if options.leading {
        if let Some(first_input) = times_of(&events, &EventType::Input).min_by(f64::total_cmp) {
            for event in &mut events {
                event.time = (event.time - first_input).max(0.0);
            }
            end = end.map(|end| (end - first_input).max(0.0));
        }
    }
    if options.trailing {
        if let Some(last_output) = times_of(&events, &EventType::Output).max_by(f64::total_cmp) {
            for event in &mut events {
                event.time = event.time.min(last_output);
            }
            end = end.map(|_| last_output);
        }
    }
    Span {
        events,
        end,
        shift: None,
        length,
    }
}

#[cfg(test)]
mod tests;
