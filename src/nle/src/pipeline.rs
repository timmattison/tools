//! The filter pipeline: it runs filters over the events of a timeline, and it
//! flattens an edit project into one recording.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::de::DeserializeOwned;

use crate::error::NleError;
use crate::types::{
    AppliedFilter, Clip, EditProject, Event, FilterKind, FilterOptions, FilterRegion, Recording,
};

/// A filter: a pure function from the events of a region to new events.
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
///
/// A filter of `FilterRegistry::register_typed` gets these steps too, with
/// more: its options are parsed first, it sees the length of its region, it
/// sees and can set the end of the timeline, it can set the shift, and it can
/// refuse its options (see `Span`).
pub type FilterFn = dyn Fn(&[Event], &FilterOptions) -> Vec<Event> + Send + Sync;

/// The events of a region, the end of the timeline, and the shift of the
/// events after the region, as a filter of `FilterRegistry::register_typed`
/// sees them and answers them.
///
/// The steps on `FilterFn` apply to `events`. These steps apply to `end`:
///
/// 1. The filter sees the end of the timeline, relative to the region start,
///    when the end is inside the region: at or after its start, and before its
///    end. The region `All` holds every end. Else the filter sees `None`.
/// 2. An answer of `None` leaves the end to step 8 on `FilterFn`: the end
///    moves by the shift.
/// 3. An answer of `Some(end)` sets the end of the timeline to `end` plus the
///    region start. It is an error when the filter saw `None`, and when `end`
///    is not finite or is below zero on the timeline. Each error is a render
///    error that names the kind.
/// 4. The end is never less than the time of the last event.
///
/// These steps apply to `shift`:
///
/// 1. The filter sees `None`.
/// 2. An answer of `None` leaves the shift of step 6 on `FilterFn`.
/// 3. An answer of `Some(shift)` is the shift. The events after the region
///    move by it, also when no event of the region moves, and so does the end
///    by step 8 on `FilterFn`. It is an error when `shift` is not finite, and
///    when it moves an event after the region below zero. The error is a
///    render error that names the kind.
///
/// These steps apply to `length`:
///
/// 1. The filter sees the length of its region: its end minus its start. The
///    length of `All` is infinite. The length does not stop at the end of the
///    timeline, because `end` gives that.
/// 2. The pipeline does not read the length of the answer.
///
/// Thus a filter that cuts the time after its last event answers that time
/// as the end. A plain filter cannot do this, because step 8 on `FilterFn`
/// keeps the gap between the last event and the end. And a filter that adds
/// time to its region answers that time as the shift, so the events after the
/// region move also when no event of the region moves. And a filter whose
/// options do not fit its region answers an error, for example a pause at a
/// point past the end of its range.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    /// The events, with times relative to the region start.
    pub events: Vec<Event>,
    /// The end of the timeline, relative to the region start, by the steps
    /// above.
    pub end: Option<f64>,
    /// The shift of the events after the region, by the steps above.
    pub shift: Option<f64>,
    /// The length of the region, by the steps above.
    pub length: f64,
}

/// The one form in which the registry keeps a filter: a function from a span
/// to a span. It fails with a reason when the options do not parse.
type Stage = dyn Fn(Span, &FilterOptions) -> Result<Span, String> + Send + Sync;

/// The filters that a project can use, by kind.
#[derive(Clone, Default)]
pub struct FilterRegistry {
    filters: BTreeMap<FilterKind, Arc<Stage>>,
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
        self.filters.insert(
            kind,
            Arc::new(move |span: Span, options: &FilterOptions| {
                Ok(Span {
                    events: filter(&span.events, options),
                    end: None,
                    shift: None,
                    length: span.length,
                })
            }),
        );
    }

    /// Register `filter` under `kind`, with options of the type `O`.
    ///
    /// Before each call, the pipeline parses the `FilterOptions` of the
    /// `AppliedFilter` as `O`. When they do not parse, the render fails with a
    /// render error that names the kind and the reason. Thus `O` holds the
    /// rules of the options: a missing key, a value of the wrong type, and,
    /// with `#[serde(deny_unknown_fields)]`, a key that `O` does not know.
    /// The filter answers `Ok` with a span, or `Err` with a reason when its
    /// options do not fit its region. An `Err` is a render error that names
    /// the kind, says that the options are invalid, and holds the reason.
    /// A filter that is already under `kind` is replaced.
    pub fn register_typed<O, F>(&mut self, kind: FilterKind, filter: F)
    where
        O: DeserializeOwned + 'static,
        F: Fn(Span, &O) -> Result<Span, String> + Send + Sync + 'static,
    {
        self.filters.insert(
            kind,
            Arc::new(move |span: Span, options: &FilterOptions| {
                let options = options.parse::<O>().map_err(|error| error.to_string())?;
                filter(span, &options)
            }),
        );
    }

    /// Whether a filter is registered under `kind`.
    #[must_use]
    pub fn contains(&self, kind: &FilterKind) -> bool {
        self.filters.contains_key(kind)
    }

    /// The filter under `kind`, or a render error that names the kind.
    fn lookup(&self, kind: &FilterKind) -> Result<&Arc<Stage>, NleError> {
        self.filters.get(kind).ok_or_else(|| {
            NleError::RenderError(format!(
                "no filter is registered for kind \"{}\"",
                kind.as_str()
            ))
        })
    }

    /// One filter step: the steps on `FilterFn` and on `Span`.
    fn apply(
        &self,
        events: Vec<Event>,
        applied: &AppliedFilter,
        end: &mut f64,
    ) -> Result<Vec<Event>, NleError> {
        let filter = self.lookup(&applied.kind)?;
        let name = applied.kind.as_str();
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

        let inside_last = last_time(&inside);
        let end_inside = start <= *end && *end < region_end;
        let span = Span {
            events: inside,
            end: end_inside.then_some(*end - start),
            shift: None,
            length: region_end - start,
        };
        let Span {
            events: answer,
            end: answered_end,
            shift: answered_shift,
            length: _,
        } = filter(span, &applied.options).map_err(|reason| {
            NleError::RenderError(format!("filter \"{name}\" has invalid options: {reason}"))
        })?;
        for event in &answer {
            let time = event.time + start;
            if !(time.is_finite() && time >= 0.0) {
                return Err(NleError::RenderError(format!(
                    "filter \"{name}\" answered the time {time} s, which is not finite or is below zero"
                )));
            }
        }
        let answered_end = match answered_end {
            None => None,
            Some(_) if !end_inside => {
                return Err(NleError::RenderError(format!(
                    "filter \"{name}\" answered an end of the timeline, but the end is not inside its region"
                )));
            }
            Some(relative) => {
                let time = relative + start;
                if !(time.is_finite() && time >= 0.0) {
                    return Err(NleError::RenderError(format!(
                        "filter \"{name}\" answered the end {time} s, which is not finite or is below zero"
                    )));
                }
                Some(time)
            }
        };
        let shift = match answered_shift {
            None => last_time(&answer) - inside_last,
            Some(shift) => {
                // A shift below zero moves the first event after the region
                // below zero first.
                let first_after = after
                    .iter()
                    .map(|event| event.time)
                    .fold(f64::INFINITY, f64::min);
                if !(shift.is_finite() && first_after + shift >= 0.0) {
                    return Err(NleError::RenderError(format!(
                        "filter \"{name}\" answered the shift {shift} s, which is not finite or moves an event after its region below zero"
                    )));
                }
                shift
            }
        };

        let mut result = before;
        result.extend(answer.into_iter().map(|event| Event {
            time: event.time + start,
            ..event
        }));
        result.extend(after.into_iter().map(|event| Event {
            time: event.time + shift,
            ..event
        }));
        // A stable sort: events at one time keep the order that they had.
        result.sort_by(|a, b| a.time.total_cmp(&b.time));

        match answered_end {
            Some(time) => *end = time,
            None if *end >= start => *end += shift,
            None => {}
        }
        *end = end.max(last_time(&result)).max(0.0);
        Ok(result)
    }

    /// Run `filters` in order over `events`, by the steps on `FilterFn` and
    /// on `Span`. The end of the timeline starts at the time of the last
    /// event.
    ///
    /// # Errors
    /// Returns `NleError::RenderError` when a filter kind is not registered,
    /// when the options of a typed filter do not parse or the filter refuses
    /// them, when a filter answers a time or an end that is not finite or is
    /// below zero on the timeline, when a filter answers an end that it did
    /// not see, when a filter answers a shift that is not finite or moves an
    /// event below zero, or when an event time or the end of the timeline that
    /// the filters give is not finite.
    pub fn run(
        &self,
        events: Vec<Event>,
        filters: &[AppliedFilter],
    ) -> Result<Vec<Event>, NleError> {
        let end = last_time(&events);
        self.run_with_end(events, filters, end)
            .map(|(events, _end)| events)
    }

    /// Run `filters` in order, and follow the end of the timeline, which
    /// starts at `end`. Returns the events and the final end.
    ///
    /// # Errors
    /// Returns `NleError::RenderError` when a filter fails, or when an event
    /// time or the final end is not finite.
    fn run_with_end(
        &self,
        events: Vec<Event>,
        filters: &[AppliedFilter],
        end: f64,
    ) -> Result<(Vec<Event>, f64), NleError> {
        let mut events = events;
        let mut end = end;
        for applied in filters {
            events = self.apply(events, applied, &mut end)?;
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
    /// 2. The events are the events of each clip on the timeline, track by
    ///    track and clip by clip, in a stable time order. Events at one time
    ///    keep track order, then clip order.
    /// 3. The filters of the project run on them by the steps on `FilterFn`
    ///    and on `Span`, with the end of the timeline starting at
    ///    `EditProject::duration`.
    /// 4. The header (`version`, `width`, `height`, `timestamp`, `command`,
    ///    `title`, `env`) is the header of the source of the clip that starts
    ///    first on the timeline. A tie goes to the clip that comes first in
    ///    track order, then in clip order.
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
        // The clips in track order, then in the order on their track.
        let ordered: Vec<&Clip> = project
            .timeline
            .iter()
            .flat_map(|track| &track.clips)
            .filter_map(|id| project.clips.iter().find(|clip| clip.id == *id))
            .collect();
        // `min_by` gives the first of several equal clips, so a tie goes to
        // the earlier clip in that order.
        let Some(first) = ordered
            .iter()
            .min_by(|a, b| a.timeline_offset.get().total_cmp(&b.timeline_offset.get()))
        else {
            return Err(NleError::RenderError(
                "the project has no clip to render".to_string(),
            ));
        };

        let mut events: Vec<Event> = ordered
            .iter()
            .flat_map(|clip| clip.timeline_events())
            .collect();
        // A stable sort keeps track order, then clip order, on a tie.
        events.sort_by(|a, b| a.time.total_cmp(&b.time));
        let (events, end) = self.run_with_end(events, &project.filters, project.duration())?;

        let header = &first.source;
        Ok(Recording {
            version: header.version,
            width: header.width,
            height: header.height,
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
