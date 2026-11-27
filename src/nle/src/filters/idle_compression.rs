//! The idle time compression filter: it shortens each stretch of time in
//! which the terminal is idle.
//!
//! The terminal is idle when no key is pressed and the screen does not change.
//! The filter replays the output through the terminal emulator of
//! `termstate`, on a screen of the size of the recording. It replays the
//! output before its region first, so the replay of the region starts from
//! the screen at its start. An input event is activity. An output event is activity when it changes a cell of a
//! watched row. A cursor move, a change of the window title, or a redraw of
//! the same text changes no cell. The rows to ignore at the top and at the
//! bottom of the screen can hold a status bar with a clock, so a change there
//! is not activity.
//!
//! An idle period is the time from one activity to the next. The start and
//! the end of the region count as activity too. Each idle period of at least
//! the threshold becomes the compressed length, and a compressed length of 0
//! removes it. The events inside a period keep their order, and their times
//! shrink in proportion. Each later event moves back by the time that the
//! periods before it lost. The filter marks the start of each period that it
//! compressed.
//!
//! The size of the screen comes from the `SpanContext` of the region, which
//! gives the size of the recording. The filter refuses a screen with no cell
//! or more than `MAX_SCREEN_CELLS` cells, and more rows to ignore than the
//! screen has.

use std::iter;

use termstate::{Cell, TerminalState, TerminalTheme};

use crate::error::NleError;
use crate::format::format_duration;
use crate::pipeline::{FilterRegistry, Mark, Span, SpanContext, TerminalSize};
use crate::types::{AppliedFilter, Event, EventType, FilterKind, FilterOptions, FilterRegion};

/// The kind of the idle compression filter.
pub const KIND: &str = "idle-compression";

/// The option that holds the shortest idle period to compress, in seconds.
pub const THRESHOLD_OPTION: &str = "threshold";

/// The option that holds the length of a compressed period, in seconds.
pub const COMPRESS_TO_OPTION: &str = "compress_to";

/// The option that holds the number of rows at the top of the screen whose
/// changes are not activity.
pub const IGNORE_TOP_ROWS_OPTION: &str = "ignore_top_rows";

/// The option that holds the number of rows at the bottom of the screen
/// whose changes are not activity.
pub const IGNORE_BOTTOM_ROWS_OPTION: &str = "ignore_bottom_rows";

/// The most cells that the screen of the filter can have. A recording can
/// state any size, and the emulator keeps each cell in memory.
pub const MAX_SCREEN_CELLS: u32 = 1 << 22;

/// The settings of one idle compression. The threshold is finite and above
/// zero. The compressed length is finite, not below zero, and not above the
/// threshold. The filter checks the ignored rows against the rows of the
/// recording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IdleCompression {
    threshold: f64,
    compress_to: f64,
    ignore_top_rows: u16,
    ignore_bottom_rows: u16,
}

impl IdleCompression {
    /// Settings that compress each idle period of at least `threshold`
    /// seconds to `compress_to` seconds. No row is ignored.
    ///
    /// # Errors
    /// Returns `NleError::InvalidIdleCompression` when the threshold is not
    /// finite or not above zero, or when the compressed length is not finite,
    /// is below zero, or is above the threshold.
    pub fn new(threshold: f64, compress_to: f64) -> Result<Self, NleError> {
        if !(threshold.is_finite() && threshold > 0.0) {
            return Err(NleError::InvalidIdleCompression(format!(
                "the threshold {threshold} s must be finite and above zero"
            )));
        }
        if !(compress_to.is_finite() && compress_to >= 0.0 && compress_to <= threshold) {
            return Err(NleError::InvalidIdleCompression(format!(
                "the compressed length {compress_to} s must be finite, not below zero, and not above the threshold {threshold} s"
            )));
        }
        Ok(Self {
            threshold,
            compress_to,
            ignore_top_rows: 0,
            ignore_bottom_rows: 0,
        })
    }

    /// These settings, with `top` rows at the top and `bottom` rows at the
    /// bottom of the screen ignored. The filter refuses them when `top` and
    /// `bottom` together are more than the rows of the recording.
    #[must_use]
    pub fn ignoring_rows(self, top: u16, bottom: u16) -> Self {
        Self {
            ignore_top_rows: top,
            ignore_bottom_rows: bottom,
            ..self
        }
    }

    /// The shortest idle period to compress, in seconds.
    #[must_use]
    pub fn threshold(self) -> f64 {
        self.threshold
    }

    /// The length of a compressed period, in seconds.
    #[must_use]
    pub fn compress_to(self) -> f64 {
        self.compress_to
    }

    /// The number of rows at the top of the screen that are ignored.
    #[must_use]
    pub fn ignore_top_rows(self) -> u16 {
        self.ignore_top_rows
    }

    /// The number of rows at the bottom of the screen that are ignored.
    #[must_use]
    pub fn ignore_bottom_rows(self) -> u16 {
        self.ignore_bottom_rows
    }
}

/// The kind of the idle compression filter, as a `FilterKind`.
#[must_use]
pub fn kind() -> FilterKind {
    FilterKind::new(KIND)
}

/// An idle compression filter for `region` with `settings`.
#[must_use]
pub fn applied(region: FilterRegion, settings: IdleCompression) -> AppliedFilter {
    AppliedFilter {
        kind: kind(),
        region,
        options: FilterOptions::new()
            .with(THRESHOLD_OPTION, settings.threshold)
            .with(COMPRESS_TO_OPTION, settings.compress_to)
            .with(IGNORE_TOP_ROWS_OPTION, settings.ignore_top_rows)
            .with(IGNORE_BOTTOM_ROWS_OPTION, settings.ignore_bottom_rows),
    }
}

/// The settings of an idle compression filter. It is `None` when the filter
/// is of another kind, or when an option is missing or does not make valid
/// settings.
#[must_use]
pub fn settings_of(filter: &AppliedFilter) -> Option<IdleCompression> {
    if filter.kind != kind() {
        return None;
    }
    settings_from(&filter.options).ok()
}

/// The settings that `options` hold, or the reason why they do not make
/// valid settings.
fn settings_from(options: &FilterOptions) -> Result<IdleCompression, String> {
    let seconds = |name: &str| {
        options
            .get(name)
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| format!("the option \"{name}\" is missing or is not a number"))
    };
    let count = |name: &str| {
        options
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| {
                format!(
                    "the option \"{name}\" is missing or is not a whole number from 0 to {}",
                    u16::MAX
                )
            })
    };
    let (top, bottom) = (
        count(IGNORE_TOP_ROWS_OPTION)?,
        count(IGNORE_BOTTOM_ROWS_OPTION)?,
    );
    IdleCompression::new(seconds(THRESHOLD_OPTION)?, seconds(COMPRESS_TO_OPTION)?)
        .map(|settings| settings.ignoring_rows(top, bottom))
        .map_err(|error| error.to_string())
}

/// Add the idle compression filter to `registry`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_span(kind(), compress);
}

/// Compress the idle periods of a span by the settings in `options`: the
/// steps in the documentation of this module. `context` gives the size of the
/// screen and the events before the span.
fn compress(span: Span, context: SpanContext<'_>, options: &FilterOptions) -> Result<Span, String> {
    let settings = settings_from(options)?;
    let activity = activity_times(context, &span.events, settings)?;
    let last_event = span.events.iter().map(|e| e.time).fold(0.0, f64::max);
    let end = span.length.max(last_event);
    let map = TimeMap::new(idle_periods(&activity, end, settings), settings.compress_to);

    let events: Vec<Event> = span
        .events
        .into_iter()
        .map(|event| Event {
            time: map.apply(event.time),
            ..event
        })
        .collect();
    let mut marks: Vec<Mark> = span
        .marks
        .into_iter()
        .map(|mark| Mark {
            time: map.apply(mark.time),
            ..mark
        })
        .collect();
    marks.extend(map.marks());
    // The map never decreases, so the end stays the largest time. The max
    // keeps it so when the rounding of two times differs.
    let length = events
        .iter()
        .map(|e| e.time)
        .chain(marks.iter().map(|mark| mark.time))
        .fold(map.apply(end), f64::max);
    Ok(Span {
        events,
        length,
        marks,
    })
}

/// The times of the activity of `events`, in time order: each input event,
/// and each output event that changes a cell of a watched row. The emulator
/// has the size of `context`, and the output of `context.before` replays into
/// it first. The output replays in time order, and events at one time keep
/// their order.
fn activity_times(
    context: SpanContext<'_>,
    events: &[Event],
    settings: IdleCompression,
) -> Result<Vec<f64>, String> {
    let (width, height) = screen_of(context.size, settings)?;
    let watched = usize::from(settings.ignore_top_rows)
        ..height.saturating_sub(usize::from(settings.ignore_bottom_rows));
    let mut terminal = TerminalState::new(width, height, TerminalTheme::auto());
    for event in in_time_order(context.before) {
        if event.event_type == EventType::Output {
            feed(&mut terminal, event)?;
        }
    }
    // The watched rows as the last activity left them.
    let mut shown: Vec<Vec<Cell>> = watched_rows(&terminal, watched.clone()).to_vec();

    let mut times = Vec::new();
    for event in in_time_order(events) {
        let active = match event.event_type {
            EventType::Input => true,
            EventType::Output => {
                feed(&mut terminal, event)?;
                update(&mut shown, watched_rows(&terminal, watched.clone()))
            }
        };
        if active {
            times.push(event.time);
        }
    }
    Ok(times)
}

/// The columns and the rows of a screen of `size`, or the reason why the
/// filter refuses it: the screen has no cell or more than `MAX_SCREEN_CELLS`
/// cells, or the rows that `settings` ignore are more than its rows.
fn screen_of(size: TerminalSize, settings: IdleCompression) -> Result<(usize, usize), String> {
    let TerminalSize { width, height } = size;
    let cells = u32::from(width) * u32::from(height);
    if cells == 0 || cells > MAX_SCREEN_CELLS {
        return Err(format!(
            "the screen of {width}x{height} cells must have at least one column and one row, and at most {MAX_SCREEN_CELLS} cells"
        ));
    }
    let (top, bottom) = (settings.ignore_top_rows, settings.ignore_bottom_rows);
    if u32::from(top) + u32::from(bottom) > u32::from(height) {
        return Err(format!(
            "the {top} top and {bottom} bottom rows to ignore are more than the {height} rows of the screen"
        ));
    }
    Ok((usize::from(width), usize::from(height)))
}

/// `events` in time order. Events at one time keep their order.
fn in_time_order(events: &[Event]) -> Vec<&Event> {
    let mut ordered: Vec<&Event> = events.iter().collect();
    ordered.sort_by(|a, b| a.time.total_cmp(&b.time));
    ordered
}

/// Replay the output of `event` into `terminal`.
fn feed(terminal: &mut TerminalState, event: &Event) -> Result<(), String> {
    terminal
        .process_output(&event.data)
        .map_err(|error| error.to_string())
}

/// The rows of the screen of `terminal` in `rows`, or no row when the range
/// is not on the screen.
fn watched_rows(terminal: &TerminalState, rows: std::ops::Range<usize>) -> &[Vec<Cell>] {
    terminal.get_grid().get(rows).unwrap_or_default()
}

/// Copy each row of `rows` that differs into `shown`. Returns whether a row
/// differed.
fn update(shown: &mut [Vec<Cell>], rows: &[Vec<Cell>]) -> bool {
    let mut changed = false;
    for (kept, row) in shown.iter_mut().zip(rows) {
        if kept != row {
            kept.clone_from(row);
            changed = true;
        }
    }
    changed
}

/// An idle period: the time from one activity to the next, before the
/// filter.
#[derive(Debug, Clone, Copy)]
struct Period {
    start: f64,
    end: f64,
}

impl Period {
    fn length(self) -> f64 {
        self.end - self.start
    }
}

/// The idle periods to compress: the gaps between two activities, or
/// between an activity and the start at 0 or `end`, that are at least the
/// threshold and longer than the compressed length.
fn idle_periods(activity: &[f64], end: f64, settings: IdleCompression) -> Vec<Period> {
    let mut periods = Vec::new();
    let mut previous = 0.0_f64;
    for time in activity.iter().copied().chain(iter::once(end)) {
        let period = Period {
            start: previous,
            end: time,
        };
        if period.length() >= settings.threshold && period.length() > settings.compress_to {
            periods.push(period);
        }
        previous = previous.max(time);
    }
    periods
}

/// The map from a time of the region before the filter to the time after it.
struct TimeMap {
    periods: Vec<Period>,
    compress_to: f64,
    /// The time that the periods before each period lose, and at the end the
    /// time that all of them lose.
    lost_before: Vec<f64>,
}

impl TimeMap {
    fn new(periods: Vec<Period>, compress_to: f64) -> Self {
        let mut lost_before = Vec::with_capacity(periods.len() + 1);
        let mut lost = 0.0;
        lost_before.push(lost);
        for period in &periods {
            lost += period.length() - compress_to;
            lost_before.push(lost);
        }
        Self {
            periods,
            compress_to,
            lost_before,
        }
    }

    /// The time after the filter of `time`. A time inside a period shrinks in
    /// proportion. A later time moves back by the time that the periods
    /// before it lose.
    fn apply(&self, time: f64) -> f64 {
        // The first period that ends after `time`.
        let index = self.periods.partition_point(|period| period.end <= time);
        let lost = self.lost_before.get(index).copied().unwrap_or_default();
        let moved = match self.periods.get(index) {
            Some(period) if time > period.start => {
                period.start - lost + (time - period.start) * self.compress_to / period.length()
            }
            _ => time - lost,
        };
        moved.max(0.0)
    }

    /// A mark at the start of each period, after the filter.
    fn marks(&self) -> impl Iterator<Item = Mark> + '_ {
        self.periods.iter().map(|period| Mark {
            time: self.apply(period.start),
            label: label(period.length(), self.compress_to),
        })
    }
}

/// The text of the mark of a period of `length` seconds that became
/// `compress_to` seconds.
fn label(length: f64, compress_to: f64) -> String {
    if compress_to > 0.0 {
        format!(
            "Idle time: {} compressed to {}",
            format_duration(length),
            format_duration(compress_to)
        )
    } else {
        format!("Idle time: {} removed", format_duration(length))
    }
}

#[cfg(test)]
mod tests;
