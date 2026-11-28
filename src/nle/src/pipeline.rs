//! The filter pipeline: it runs filters over the events of a timeline, and it
//! flattens an edit project into one recording.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::error::NleError;
use crate::types::{
    AppliedFilter, EditProject, Event, FilterKind, FilterOptions, FilterRegion, Recording,
};

/// A filter: a pure function from the events of a region to new events.
///
/// This is the event form. `InPlaceFilterFn` is the in-place form, for a
/// filter that rewrites its region and moves nothing. `SpanFilterFn` is the
/// span form, for a filter that changes the length of its region.
///
/// The pipeline calls a filter once for each `AppliedFilter` that names it.
/// What a filter sees, and what the pipeline does with its answer:
///
/// 1. The filter is looked up by its kind. An unknown kind is a render error.
/// 2. The events are split, in order, into those before the region, those
///    inside it, and those after it. The region `All` starts at 0 and has no
///    end. A range holds its start and not its end.
/// 3. The filter sees only the events inside the region. Their times are
///    relative to the start of the region, so a filter works the same on
///    `All` and on a range. A region with no event still calls the filter,
///    with an empty slice.
/// 4. The filter answers with new events, also relative to the region start.
/// 5. Each answered time must be finite, and it must not be below zero on the
///    timeline (relative time plus region start). Else the pipeline fails
///    with a render error that names the kind.
/// 6. The shift is the last time of the answer minus the last time of the
///    events that the filter saw (0 for an empty list). The events after the
///    region move by the shift, so the gap between the last event of the
///    region and the next event stays the same.
/// 7. The result is the events before the region, then the answer moved back
///    onto the timeline, then the events after the region. It is then put in
///    time order with a stable sort: events at one time keep the order that
///    they had. Thus a filter need not return its events in order.
/// 8. The end of the timeline moves by the shift when it is at or after the
///    region start. It is never less than the time of the last event.
///
/// The region of a later filter is in the time that the earlier filters leave.
/// After the last filter, `FilterRegistry::run` fails when an event time or
/// the end of the timeline is not finite, because a shift can overflow.
pub type FilterFn = dyn Fn(&[Event], &FilterOptions) -> Vec<Event> + Send + Sync;

/// What an in-place filter knows about its region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionView {
    /// The length of the region in seconds: from its start to its end, or to
    /// the end of the timeline when that comes first. It is never below zero.
    pub length: f64,
    /// True when nothing of the timeline comes after the region: no event
    /// comes after it, and it reaches the end of the timeline. The region
    /// `All` is always open.
    pub open: bool,
}

/// An in-place filter: a pure function that rewrites the events of a region
/// and keeps the region in its place on the timeline.
///
/// Use an in-place filter when the filter changes what the events say and
/// not when they occur, for example a filter that rewrites escape sequences.
/// The pipeline calls it once for each `AppliedFilter` that names it. Steps 1
/// to 3 and step 7 of `FilterFn` are the same. The other steps are:
///
/// 1. The filter gets the events inside the region, relative to its start,
///    and a `RegionView`. The view tells the length of the region, and
///    whether anything of the timeline comes after it. Thus a filter that
///    sets a state of the terminal can set it back at the end of a range, and
///    leave it at the end of the recording.
/// 2. The filter answers `Ok` with new events, or `Err` with a message. An
///    `Err` fails the pipeline with a render error. Its text names the kind
///    and holds the message.
/// 3. Each answered time is checked as in step 5 of `FilterFn`, with the same
///    error. It must also be inside the region: from 0 to the length of the
///    region, both included. Else the pipeline fails with a render error that
///    names the kind.
/// 4. Nothing moves: the events after the region keep their times, and the
///    end of the timeline stays. An answered time goes back onto the timeline
///    as the relative time plus the region start, but never after the end of
///    the region, because that sum can round up. Thus an answered event at
///    the length lands on the end of the region, and it comes before an
///    event after the region at the same time.
pub type InPlaceFilterFn =
    dyn Fn(&[Event], RegionView, &FilterOptions) -> Result<Vec<Event>, String> + Send + Sync;

/// The size of a terminal, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    /// The number of columns.
    pub width: u16,
    /// The number of rows.
    pub height: u16,
}

impl TerminalSize {
    /// The size of a VT100: 80 columns and 24 rows.
    pub const VT100: Self = Self {
        width: 80,
        height: 24,
    };
}

/// What a span filter knows about the timeline outside its region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpanContext<'a> {
    /// The events before the region, at their times on the timeline, in the
    /// order that they play. A filter that keeps a model of the screen replays
    /// them to get the screen at the start of the region.
    pub before: &'a [Event],
    /// The events after the region, at their times on the timeline before
    /// the pipeline moves them, in the order that they play. A filter that
    /// reads the output of each input event finds from them if the end of the
    /// region cuts the output of its last input event.
    pub after: &'a [Event],
    /// The size of the terminal that the events draw on.
    pub size: TerminalSize,
}

/// A region of the timeline as a span filter sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    /// The events of the region, with times relative to its start.
    pub events: Vec<Event>,
    /// The length of the region in seconds.
    pub length: f64,
}

/// A span filter: a pure function from a region and its length to new events
/// and a new length, or an error message.
///
/// Use a span filter when the filter changes the length of its region, for
/// example a change of speed. The pipeline calls it once for each
/// `AppliedFilter` that names it. Steps 1 to 3 and 7 to 8 of `FilterFn` are the
/// same. The other steps are:
///
/// 1. The length of the region is from its start to its end, or to the end of
///    the timeline when the end of the timeline comes first. It is never below
///    zero, so a region that starts after the end of the timeline has the
///    length 0. For the region `All`, the length is the end of the timeline.
/// 2. The filter gets a `Span` with the events inside the region, relative to
///    its start, and the length. It also gets a `SpanContext`: the events
///    before the region, the events after it, and the size of the terminal.
///    It answers `Ok` with a new `Span`, or `Err` with a message.
/// 3. An `Err` fails the pipeline with a render error. Its text names the kind
///    and holds the message.
/// 4. Each answered time is checked as in step 5 of `FilterFn`, with the same
///    error.
/// 5. The answered length must be finite. It must not be below zero, and it
///    must not be below the largest answered time. Else the pipeline fails
///    with a render error that names the kind and the length.
/// 6. The shift is the answered length minus the length. The events after the
///    region move by the shift, also when the region holds no event. The end
///    of the timeline moves by the shift as in step 8 of `FilterFn`.
pub type SpanFilterFn =
    dyn Fn(Span, SpanContext<'_>, &FilterOptions) -> Result<Span, String> + Send + Sync;

/// A filter in one of its three forms.
#[derive(Clone)]
enum Registered {
    Events(Arc<FilterFn>),
    InPlace(Arc<InPlaceFilterFn>),
    Span(Arc<SpanFilterFn>),
}

/// The filters that a project can use, by kind.
#[derive(Clone, Default)]
pub struct FilterRegistry {
    filters: BTreeMap<FilterKind, Registered>,
}

impl fmt::Debug for FilterRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FilterRegistry")
            .field("kinds", &self.filters.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// The largest time of `events`, or 0.0 when there is none.
fn last_time(events: &[Event]) -> f64 {
    events.iter().map(|event| event.time).fold(0.0, f64::max)
}

impl FilterRegistry {
    /// Make a registry with no filter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `filter` under `kind`. A filter that is already under `kind`
    /// is replaced.
    pub fn register(
        &mut self,
        kind: FilterKind,
        filter: impl Fn(&[Event], &FilterOptions) -> Vec<Event> + Send + Sync + 'static,
    ) {
        self.filters
            .insert(kind, Registered::Events(Arc::new(filter)));
    }

    /// Register `filter` as an in-place filter under `kind`. A filter of any
    /// form that is already under `kind` is replaced.
    pub fn register_in_place(
        &mut self,
        kind: FilterKind,
        filter: impl Fn(&[Event], RegionView, &FilterOptions) -> Result<Vec<Event>, String>
            + Send
            + Sync
            + 'static,
    ) {
        self.filters
            .insert(kind, Registered::InPlace(Arc::new(filter)));
    }

    /// Register `filter` as a span filter under `kind`. A filter of any form
    /// that is already under `kind` is replaced.
    pub fn register_span(
        &mut self,
        kind: FilterKind,
        filter: impl Fn(Span, SpanContext<'_>, &FilterOptions) -> Result<Span, String>
            + Send
            + Sync
            + 'static,
    ) {
        self.filters
            .insert(kind, Registered::Span(Arc::new(filter)));
    }

    /// Whether a filter is registered under `kind`.
    #[must_use]
    pub fn contains(&self, kind: &FilterKind) -> bool {
        self.filters.contains_key(kind)
    }

    /// The filter under `kind`, or a render error that names the kind.
    fn lookup(&self, kind: &FilterKind) -> Result<&Registered, NleError> {
        self.filters.get(kind).ok_or_else(|| {
            NleError::RenderError(format!(
                "no filter is registered for kind \"{}\"",
                kind.as_str()
            ))
        })
    }

    /// One filter step: the steps on `FilterFn`, on `InPlaceFilterFn`, or on
    /// `SpanFilterFn`. The events draw on a terminal of `size`.
    fn apply(
        &self,
        events: Vec<Event>,
        applied: &AppliedFilter,
        end: &mut f64,
        size: TerminalSize,
    ) -> Result<Vec<Event>, NleError> {
        let filter = self.lookup(&applied.kind)?;
        let (start, region_end) = match &applied.region {
            FilterRegion::All => (0.0, f64::INFINITY),
            FilterRegion::Range { start, end } => (start.get(), end.get()),
        };

        let mut before = Vec::new();
        let mut inside = Vec::new();
        let mut after = Vec::new();
        for event in events {
            if event.time < start {
                before.push(event);
            } else if event.time < region_end {
                inside.push(Event {
                    time: event.time - start,
                    ..event
                });
            } else {
                after.push(event);
            }
        }

        let kind = applied.kind.as_str();
        // The answer, the view of an in-place filter, the latest time on the
        // timeline that the answer can get, and how far the events after the
        // region move.
        let (answer, view, ceiling, shift) = match filter {
            Registered::Events(filter) => {
                let answer = filter(&inside, &applied.options);
                let shift = last_time(&answer) - last_time(&inside);
                (answer, None, f64::INFINITY, shift)
            }
            Registered::InPlace(filter) => {
                // The end of the region on the timeline.
                let top = region_end.min(*end).max(start);
                let view = RegionView {
                    length: top - start,
                    open: after.is_empty() && region_end >= *end,
                };
                let answer = filter(&inside, view, &applied.options).map_err(|message| {
                    NleError::RenderError(format!("filter \"{kind}\" failed: {message}"))
                })?;
                // An in-place filter moves nothing.
                (answer, Some(view), top, 0.0)
            }
            Registered::Span(filter) => {
                let length = (region_end.min(*end) - start).max(0.0);
                let span = Span {
                    events: inside,
                    length,
                };
                let context = SpanContext {
                    before: &before,
                    after: &after,
                    size,
                };
                let answer = filter(span, context, &applied.options).map_err(|message| {
                    NleError::RenderError(format!("filter \"{kind}\" failed: {message}"))
                })?;
                let largest = last_time(&answer.events);
                if !(answer.length.is_finite() && answer.length >= 0.0 && answer.length >= largest)
                {
                    return Err(NleError::RenderError(format!(
                        "filter \"{kind}\" answered the length {} s, which is not finite, is below zero, or is below its last time {largest} s",
                        answer.length
                    )));
                }
                let shift = answer.length - length;
                (answer.events, None, f64::INFINITY, shift)
            }
        };
        for event in &answer {
            let time = event.time + start;
            if !(time.is_finite() && time >= 0.0) {
                return Err(NleError::RenderError(format!(
                    "filter \"{kind}\" answered the time {time} s, which is not finite or is below zero"
                )));
            }
        }
        if let Some(view) = view {
            let inside_region = 0.0..=view.length;
            if let Some(outside) = answer
                .iter()
                .find(|event| !inside_region.contains(&event.time))
            {
                return Err(NleError::RenderError(format!(
                    "filter \"{kind}\" answered the time {} s, which is outside its region from {start} s to {} s",
                    outside.time + start,
                    ceiling
                )));
            }
        }

        let mut result = before;
        // The length plus the start can round up by one step, so a time is
        // held at the ceiling.
        result.extend(answer.into_iter().map(|event| Event {
            time: (event.time + start).min(ceiling),
            ..event
        }));
        result.extend(after.into_iter().map(|event| Event {
            time: event.time + shift,
            ..event
        }));
        // A stable sort: events at one time keep the order that they had.
        result.sort_by(|a, b| a.time.total_cmp(&b.time));

        if *end >= start {
            *end += shift;
        }
        *end = end.max(last_time(&result)).max(0.0);
        Ok(result)
    }

    /// Run `filters` in order over `events`, by the steps of their form. The
    /// end of the timeline starts at the time of the last event.
    ///
    /// The events have no recording, so a span filter gets the size of a
    /// VT100 (`TerminalSize::VT100`). `render` gives the size of the
    /// recording.
    ///
    /// # Errors
    /// Returns `NleError::RenderError` when a filter kind is not registered,
    /// when a filter answers a time that is not finite or is below zero on
    /// the timeline, when an in-place filter fails or answers a time after
    /// its region, when a span filter answers an error or a bad length, or
    /// when an event time or the end of the timeline that the filters give is
    /// not finite.
    pub fn run(
        &self,
        events: Vec<Event>,
        filters: &[AppliedFilter],
    ) -> Result<Vec<Event>, NleError> {
        let end = last_time(&events);
        self.run_with_end(events, filters, end, TerminalSize::VT100)
            .map(|(events, _end)| events)
    }

    /// Run `filters` in order on a terminal of `size`, and follow the end of
    /// the timeline, which starts at `end`. Returns the events and the final
    /// end.
    ///
    /// # Errors
    /// Returns `NleError::RenderError` when a filter fails, when a span
    /// filter answers an error or a bad length, or when an event time or the
    /// final end is not finite.
    fn run_with_end(
        &self,
        events: Vec<Event>,
        filters: &[AppliedFilter],
        end: f64,
        size: TerminalSize,
    ) -> Result<(Vec<Event>, f64), NleError> {
        let mut events = events;
        let mut end = end;
        for applied in filters {
            events = self.apply(events, applied, &mut end, size)?;
        }
        // A shift or the arithmetic of a clip can still overflow to infinity.
        for event in &events {
            if !event.time.is_finite() {
                return Err(NleError::RenderError(format!(
                    "the render gives the time {} s, which is not finite",
                    event.time
                )));
            }
        }
        if !end.is_finite() {
            return Err(NleError::RenderError(format!(
                "the render gives the end {end} s, which is not finite"
            )));
        }
        Ok((events, end))
    }

    /// Flatten `project` into one recording.
    ///
    /// The rules:
    /// 0. A project that breaks a rule of `EditProject::validate` is refused.
    /// 1. A project with no clip fails with `NleError::RenderError`.
    /// 2. The events are the clips spliced by the rules of the private
    ///    `splice` module. The clips play in the order of their start (a tie
    ///    keeps track order, then clip order). The start of a clip cuts each
    ///    clip that still plays at that time, and each clip that plays after
    ///    the first starts with a hard cut that clears the screen.
    /// 3. The filters of the project run on them by the steps of their form,
    ///    with the end of the timeline starting at the end of the splice: the
    ///    latest end of the part of a clip that plays. With no overlap, it is
    ///    `EditProject::duration`. A span filter gets the padded size: the
    ///    largest width and the largest height of the clips.
    /// 4. `width` and `height` are the padded size. The rest of the header
    ///    (`version`, `timestamp`, `command`, `title`, `env`) is the header of
    ///    the source of the first clip that plays, or of the first clip in the
    ///    play order when no clip plays.
    /// 5. `duration` is the end of the timeline after the filters.
    /// 6. Each event time and the duration must be finite. Else the render
    ///    fails with `NleError::RenderError`.
    ///
    /// A project of one whole-recording clip and no filter renders a
    /// recording equal to its source.
    ///
    /// # Errors
    /// Returns `NleError::InvalidProject` when the project breaks a rule of
    /// `EditProject::validate`, `NleError::RenderError` for an empty project,
    /// and the errors of `FilterRegistry::run`.
    pub fn render(&self, project: &EditProject) -> Result<Recording, NleError> {
        project.validate()?;
        let Some(spliced) = crate::splice::splice(project) else {
            return Err(NleError::RenderError(
                "the project has no clip to render".to_string(),
            ));
        };
        let header = &spliced.header.source;
        let size = spliced.size;
        let (events, end) =
            self.run_with_end(spliced.events, &project.filters, spliced.end, size)?;

        Ok(Recording {
            version: header.version,
            width: size.width,
            height: size.height,
            timestamp: header.timestamp,
            duration: end,
            command: header.command.clone(),
            title: header.title.clone(),
            env: header.env.clone(),
            events,
        })
    }
}

#[cfg(test)]
mod tests;
