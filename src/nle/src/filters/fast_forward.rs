//! The fast-forward filter: it compresses a region that holds a long burst of
//! command output, for example a build, an install, or a test run.
//!
//! Issue #162 names three modes. Skip to end jumps straight to the final state
//! of the region. Accelerate plays the region at 10 to 50 times its speed.
//! Summary shows the first lines, then "...", then the last lines. This module
//! gives all three.
//!
//! The summary drops the middle lines and each terminal change that they hold,
//! for example a color, a cursor movement, or a screen clear. It suits
//! line-oriented output, such as a build log. It does not suit a full-screen
//! application.

use crate::error::NleError;
use crate::filters::time_dilation::{self, Speed};
use crate::pipeline::{FilterRegistry, Span};
use crate::types::{AppliedFilter, Event, EventType, FilterKind, FilterOptions, FilterRegion};

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

/// The value of the `mode` option for summary.
pub const SUMMARY: &str = "summary";

/// The option that holds the number of lines that a summary keeps at the start.
pub const HEAD_OPTION: &str = "head";

/// The option that holds the number of lines that a summary keeps at the end.
pub const TAIL_OPTION: &str = "tail";

/// The line that stands for the dropped lines of a summary. Terminal output
/// ends a line with CR LF.
pub const SUMMARY_MARKER: &str = "...\r\n";

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

/// The values of the `mode` option that the filter knows.
const MODES: &[&str] = &[SKIP_TO_END, ACCELERATE, SUMMARY];

/// How a fast-forward filter compresses its region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// Jump straight to the final state of the region. Each event of the
    /// region moves to the start of the region, and the region has no length.
    SkipToEnd,
    /// Play the region at the given acceleration. The times and the length of
    /// the region divide by it.
    Accelerate(Acceleration),
    /// Keep the first lines and the last lines of the output of the region.
    /// A marker line stands for the lines between them.
    ///
    /// A line is a run of output text that ends with a line feed. A last run
    /// with no line feed is a line too. The output of the region has no
    /// dropped line when it has `head + tail` lines or fewer. Else the lines
    /// from `head` up to the last `tail` lines go away, each terminal change
    /// that they hold goes away, and each input event between the first and
    /// the last dropped output goes away. The marker appears where the first
    /// dropped line started. The time between the first and the last dropped
    /// output goes away, and the tail keeps its own timing.
    Summary {
        /// The number of lines that the summary keeps at the start of the
        /// output of the region.
        head: usize,
        /// The number of lines that the summary keeps at the end of the
        /// output of the region.
        tail: usize,
    },
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
        Mode::Summary { head, tail } => FilterOptions::new()
            .with(MODE_OPTION, SUMMARY)
            .with(HEAD_OPTION, line_count_option(head))
            .with(TAIL_OPTION, line_count_option(tail)),
    };
    AppliedFilter {
        kind: kind(),
        region,
        options,
    }
}

/// A line count as a JSON number. A count that does not fit in 64 bits cannot
/// occur on a supported target, and it would saturate.
fn line_count_option(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// The mode of a fast-forward filter. It is `None` when the filter is of
/// another kind, or when its options do not give a valid mode.
#[must_use]
pub fn mode_of(filter: &AppliedFilter) -> Option<Mode> {
    if filter.kind != kind() {
        return None;
    }
    mode_from(&filter.options).ok()
}

/// Add the fast-forward filter to `registry`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_span(kind(), fast_forward);
}

/// Compress a span in the mode of its options.
fn fast_forward(span: Span, options: &FilterOptions) -> Result<Span, String> {
    Ok(match mode_from(options)? {
        Mode::SkipToEnd => skip_to_end(span),
        Mode::Accelerate(acceleration) => time_dilation::scale(span, acceleration.speed()),
        Mode::Summary { head, tail } => summarize(span, head, tail),
    })
}

/// Read the mode from the options. The error is a message for the pipeline.
fn mode_from(options: &FilterOptions) -> Result<Mode, String> {
    let name = options
        .get(MODE_OPTION)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("the option \"{MODE_OPTION}\" is missing or is not a string"))?;
    match name {
        SKIP_TO_END => Ok(Mode::SkipToEnd),
        ACCELERATE => {
            let value = options
                .get(SPEED_OPTION)
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| {
                    format!("the option \"{SPEED_OPTION}\" is missing or is not a number")
                })?;
            let acceleration = Acceleration::new(value).map_err(|error| {
                format!("the option \"{SPEED_OPTION}\" is not an acceleration: {error}")
            })?;
            Ok(Mode::Accelerate(acceleration))
        }
        SUMMARY => Ok(Mode::Summary {
            head: line_count_from(options, HEAD_OPTION)?,
            tail: line_count_from(options, TAIL_OPTION)?,
        }),
        other => Err(format!(
            "the mode \"{other}\" is not one of {}",
            MODES.join(", ")
        )),
    }
}

/// Read a line count from the option `name`. The error is a message for the
/// pipeline.
fn line_count_from(options: &FilterOptions, name: &str) -> Result<usize, String> {
    options
        .get(name)
        .and_then(serde_json::Value::as_u64)
        .and_then(|count| usize::try_from(count).ok())
        .ok_or_else(|| {
            format!("the option \"{name}\" is missing or is not a whole number of 0 or more")
        })
}

/// The pieces of the data of one output event, sorted by the part of the
/// summary that they belong to.
#[derive(Default)]
struct Pieces {
    /// The text of the lines before the dropped lines.
    head: String,
    /// The text of the lines after the dropped lines.
    tail: String,
    /// Whether the event holds text of a dropped line.
    drops: bool,
}

/// Count the lines of the output stream of a span. A line ends with a line
/// feed. A last run with no line feed is a line too, when it is not empty.
fn count_lines(events: &[Event]) -> usize {
    let mut lines = 0_usize;
    let mut open = false;
    for event in events.iter().filter(|e| e.event_type == EventType::Output) {
        for piece in event.data.split_inclusive('\n') {
            if piece.ends_with('\n') {
                lines = lines.saturating_add(1);
                open = false;
            } else {
                open = true;
            }
        }
    }
    lines.saturating_add(usize::from(open))
}

/// Keep the first `head` lines and the last `tail` lines of the output of a
/// span, and put the marker line in place of the lines between them.
///
/// The output stream is the data of the output events, joined in order. A line
/// is a run of text that ends with a line feed, and a last run with no line
/// feed is a line too. Input events hold no lines. The span stays as it is
/// when the stream has `head + tail` lines or fewer.
///
/// Else this function drops the lines from `head` up to the last `tail` lines.
/// The marker takes the time of the first output event that holds a dropped
/// line, together with the head text of that event. Each event between the
/// first and the last output event that hold a dropped line goes away, input
/// events included. The tail text of the last such event takes the time of the
/// first. Each later event moves back by the time between the first and the
/// last such event, and the length of the span shrinks by the same time. The
/// summary drops each terminal change that a dropped line holds.
fn summarize(span: Span, head: usize, tail: usize) -> Span {
    let lines = count_lines(&span.events);
    if lines <= head.saturating_add(tail) {
        return span;
    }
    let tail_start = lines.saturating_sub(tail);

    let mut line = 0_usize;
    let mut pieces: Vec<Option<Pieces>> = Vec::with_capacity(span.events.len());
    for event in &span.events {
        if event.event_type != EventType::Output {
            pieces.push(None);
            continue;
        }
        let mut split = Pieces::default();
        for piece in event.data.split_inclusive('\n') {
            if line < head {
                split.head.push_str(piece);
            } else if line >= tail_start {
                split.tail.push_str(piece);
            } else {
                split.drops = true;
            }
            if piece.ends_with('\n') {
                line = line.saturating_add(1);
            }
        }
        pieces.push(Some(split));
    }

    let drops = |split: &Option<Pieces>| split.as_ref().is_some_and(|p| p.drops);
    let (Some(first), Some(last)) = (
        pieces.iter().position(drops),
        pieces.iter().rposition(drops),
    ) else {
        return span;
    };
    let start = span.events.get(first).map_or(0.0, |e| e.time);
    let end = span.events.get(last).map_or(start, |e| e.time);
    let shift = end - start;

    let output = |time: f64, data: String| Event {
        time,
        event_type: EventType::Output,
        data,
    };
    let mut events = Vec::with_capacity(span.events.len());
    for (index, (event, split)) in span.events.into_iter().zip(pieces).enumerate() {
        if index < first {
            events.push(event);
        } else if index == first || index == last {
            let split = split.unwrap_or_default();
            if index == first {
                if !split.head.is_empty() {
                    events.push(output(start, split.head));
                }
                events.push(output(start, SUMMARY_MARKER.to_string()));
            }
            if index == last && !split.tail.is_empty() {
                events.push(output(start, split.tail));
            }
        } else if index > last {
            events.push(Event {
                time: (event.time - shift).max(start),
                ..event
            });
        }
    }
    let latest = events.iter().map(|e| e.time).fold(0.0_f64, f64::max);
    Span {
        events,
        length: (span.length - shift).max(latest),
    }
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
