//! The filters that ship with the app.
//!
//! The filters of issue #154 are span filters with typed options: each one is
//! registered with `register_with_options`, so the render refuses options that
//! break a rule of its type. The keys of the options are the field names
//! below, and they are a part of the project file format.

pub mod backspace;
pub mod cursor;
pub mod keystroke_hud;
pub mod redaction;
pub mod time_dilation;
pub mod typing_speed;
pub mod viewport;

use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::pipeline::{FilterRegistry, Span, SpanContext};
use crate::types::{Event, EventType, FilterKind, FilterOptions, Seconds};

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
/// pause at the time of a key comes before that key. The region gets longer
/// by `duration`, so the events after a range and the end of the timeline move
/// later by `duration` too, also when no event of the region moves. The normal
/// region is `All`.
///
/// `at` must not be past the end of the region: the end of a range, or the end
/// of the timeline when it comes first. The events after the region and the
/// end of the timeline are before such a point, but they move, so the render
/// refuses the point. A point past the end by `END_TOLERANCE` or less is the
/// end of the region, because the length of a range is the difference of its
/// bounds: 1.3 - 1.1 is 0.19999999999999996, not 0.2.
pub const PAUSE_INSERTION: &str = "pause-insertion";

/// How far past the end of its region, in seconds, the point of a pause
/// insertion can be before the render refuses it.
///
/// A microsecond is much larger than the rounding error of a time in a
/// recording, and much smaller than a pause that a person can see.
const END_TOLERANCE: f64 = 1e-6;

/// The kind of the filter that cuts the silence at the start and at the end.
///
/// Options: `leading` and `trailing`, each `true` when the options do not name
/// it. The leading silence is the time before the first input event: the
/// first input event moves to the start of the region, each later event moves
/// back by the same time, and each output event before it moves to the start,
/// in its order. Thus the screen that the user saw before the first key is
/// still there. The trailing silence is the time after the last output event:
/// each event after it moves back to it, and the end of the region comes back
/// to it. The end of the region is the end of a range, or the end of the
/// timeline when it comes first. A region with no input event keeps its
/// start, and a region with no output event keeps its end. The region gets
/// shorter by the silence, so on `All` the end of the timeline comes back, and
/// on a range the events after the range keep their gap from its end.
pub const AUTO_TRIM: &str = "auto-trim";

/// A registry that holds every filter that ships with the app, under its
/// kind.
#[must_use]
pub fn registry() -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    backspace::register(&mut registry);
    cursor::register(&mut registry);
    time_dilation::register(&mut registry);
    keystroke_hud::register(&mut registry);
    redaction::register(&mut registry);
    typing_speed::register(&mut registry);
    viewport::register(&mut registry);
    register_with_options(&mut registry, PAUSE_REMOVAL, |span, options| {
        Ok(remove_pauses(span, options))
    });
    register_with_options(&mut registry, PAUSE_INSERTION, insert_pause);
    register_with_options(&mut registry, AUTO_TRIM, |span, options| {
        Ok(trim_silence(span, options))
    });
    registry
}

/// Register `filter` under `kind` as a span filter whose options have the
/// type `O`.
///
/// Before each call, the `FilterOptions` of the `AppliedFilter` are parsed as
/// `O`. Thus `O` holds the rules of the options: a missing key, a value of the
/// wrong type, and, with `#[serde(deny_unknown_fields)]`, a key that `O` does
/// not know. The filter answers `Ok` with a span, or `Err` with a reason when
/// its options do not fit its region. Options that do not parse, and an
/// `Err`, each fail the render with a render error that names the kind, says
/// that the options are invalid, and holds the reason.
///
/// The filter does not get the `SpanContext` of its region, because no filter
/// of this form reads the events outside its region or the size of the
/// terminal.
fn register_with_options<O, F>(registry: &mut FilterRegistry, kind: &str, filter: F)
where
    O: DeserializeOwned + 'static,
    F: Fn(Span, &O) -> Result<Span, String> + Send + Sync + 'static,
{
    registry.register_span(
        FilterKind::new(kind),
        move |span, _context: SpanContext<'_>, options: &FilterOptions| {
            options
                .parse::<O>()
                .map_err(|error| error.to_string())
                .and_then(|options| filter(span, &options))
                .map_err(|reason| format!("invalid options: {reason}"))
        },
    );
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
/// The answer keeps the time from the last event to the end of the region, so
/// the region gets shorter by the time that the gaps lost.
fn remove_pauses(span: Span, options: &PauseRemoval) -> Span {
    let Span { mut events, length } = span;
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
    // The time after the last event, added to its new time. It is never
    // below zero, so the new length is never below the new last time.
    let length = previous.map_or(length, |(old, new)| (length - old) + new);
    Span { events, length }
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
/// Each event at or after the point moves later by the duration, and the
/// length of the region gets longer by the duration. Thus the pipeline moves
/// each event after the region and the end of the timeline later by the
/// duration, also when no event of the region moves. A point past the length
/// of the region by more than `END_TOLERANCE` is an error, because that shift
/// also moves the events after the region and the end of the timeline, which
/// are before the point. A point past the length by `END_TOLERANCE` or less
/// is the length, so an event at the end of the region moves.
fn insert_pause(span: Span, options: &PauseInsertion) -> Result<Span, String> {
    let (at, duration) = (options.at.get(), options.duration.get());
    if at > span.length + END_TOLERANCE {
        return Err(format!(
            "at {at} s is past the end of the region, which is {} s long",
            span.length
        ));
    }
    let at = at.min(span.length);
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
        length: span.length + duration,
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
/// The leading cut makes the length shorter by the time of the first input
/// event. The trailing cut makes the length the time of the last output
/// event.
fn trim_silence(span: Span, options: &AutoTrim) -> Span {
    let Span {
        mut events,
        mut length,
    } = span;
    if options.leading {
        if let Some(first_input) = times_of(&events, &EventType::Input).min_by(f64::total_cmp) {
            for event in &mut events {
                event.time = (event.time - first_input).max(0.0);
            }
            length = (length - first_input).max(0.0);
        }
    }
    if options.trailing {
        if let Some(last_output) = times_of(&events, &EventType::Output).max_by(f64::total_cmp) {
            for event in &mut events {
                event.time = event.time.min(last_output);
            }
            length = last_output;
        }
    }
    Span { events, length }
}

#[cfg(test)]
mod tests;
