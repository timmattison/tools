//! The filter pipeline: it runs filters over the events of a timeline, and it
//! flattens an edit project into one recording.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::error::NleError;
use crate::types::{
    AppliedFilter, Chapter, Clip, EditProject, Event, FilterKind, FilterOptions, FilterRegion,
    Recording,
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
/// 9. A chapter time moves with the events, by `move_chapter`. A chapter
///    before the region stays. A chapter at or after the region end moves by
///    the shift. A chapter inside the region keeps its place between the
///    events around it. If the filter adds or removes events, it keeps its
///    place in proportion to the last time of the region instead.
///
/// The region of a later filter is in the time that the earlier filters leave.
/// After the last filter, `FilterRegistry::run` fails when an event time or
/// the end of the timeline is not finite, because a shift can overflow.
pub type FilterFn = dyn Fn(&[Event], &FilterOptions) -> Vec<Event> + Send + Sync;

/// The filters that a project can use, by kind.
#[derive(Clone, Default)]
pub struct FilterRegistry {
    filters: BTreeMap<FilterKind, Arc<FilterFn>>,
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

/// What one filter step did to the times of its region, as `move_chapter` needs it.
struct StepTimes {
    /// The start of the region.
    start: f64,
    /// The end of the region, or infinity for `FilterRegion::All`.
    region_end: f64,
    /// The times of the events that the filter saw, relative to `start`, in time order.
    seen: Vec<f64>,
    /// The times of the answer, relative to `start`, in ascending order.
    answered: Vec<f64>,
    /// The last time of the answer minus the last time of the events that the filter saw.
    shift: f64,
}

/// Move the chapter time `time` through one filter step. The result is never below 0.
///
/// A time before the region start does not move. A time at or after the region
/// end moves by the shift, like the events after the region.
///
/// Inside the region, the answer matters. If the filter kept the number of
/// events, the chapter keeps its place between the events around it. It keeps
/// its fraction of the gap between the event before and the event after.
/// Before the first event, it scales from the region start. After the last
/// event, it keeps its gap to that event.
///
/// If the filter added or removed events, or the region holds none, a chapter
/// up to the last seen event moves in proportion to the last time of the
/// answer over the last time seen. A chapter after that moves by the shift.
fn move_chapter(time: f64, step: &StepTimes) -> f64 {
    if time < step.start {
        return time.max(0.0);
    }
    if time >= step.region_end {
        return (time + step.shift).max(0.0);
    }
    let rel = time - step.start;
    let seen = &step.seen;
    let answered = &step.answered;
    let mapped = if seen.len() == answered.len() && !seen.is_empty() {
        let k = seen.partition_point(|x| *x < rel);
        let at = |list: &[f64], index: usize| list.get(index).copied().unwrap_or(0.0);
        if k >= seen.len() {
            let last = seen.len() - 1;
            at(answered, last) + (rel - at(seen, last))
        } else if k == 0 {
            let first = at(seen, 0);
            if first > 0.0 {
                at(answered, 0) * rel / first
            } else {
                at(answered, 0)
            }
        } else {
            let (i0, i1) = (at(seen, k - 1), at(seen, k));
            let (a0, a1) = (at(answered, k - 1), at(answered, k));
            a0 + (rel - i0) / (i1 - i0) * (a1 - a0)
        }
    } else {
        let seen_last = seen.last().copied().unwrap_or(0.0);
        let answered_last = answered.last().copied().unwrap_or(0.0);
        if rel <= seen_last {
            if seen_last > 0.0 {
                rel * answered_last / seen_last
            } else {
                0.0
            }
        } else {
            rel + step.shift
        }
    };
    (step.start + mapped).max(0.0)
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
        self.filters.insert(kind, Arc::new(filter));
    }

    /// Whether a filter is registered under `kind`.
    #[must_use]
    pub fn contains(&self, kind: &FilterKind) -> bool {
        self.filters.contains_key(kind)
    }

    /// The filter under `kind`, or a render error that names the kind.
    fn lookup(&self, kind: &FilterKind) -> Result<&Arc<FilterFn>, NleError> {
        self.filters.get(kind).ok_or_else(|| {
            NleError::RenderError(format!(
                "no filter is registered for kind \"{}\"",
                kind.as_str()
            ))
        })
    }

    /// One filter step: the steps on `FilterFn`. `chapters` holds the chapter
    /// times, and each one moves by `move_chapter`.
    fn apply(
        &self,
        events: Vec<Event>,
        applied: &AppliedFilter,
        end: &mut f64,
        chapters: &mut [f64],
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

        let answer = filter(&inside, &applied.options);
        for event in &answer {
            let time = event.time + start;
            if !(time.is_finite() && time >= 0.0) {
                return Err(NleError::RenderError(format!(
                    "filter \"{}\" answered the time {time} s, which is not finite or is below zero",
                    applied.kind.as_str()
                )));
            }
        }
        let shift = last_time(&answer) - last_time(&inside);

        let mut answered: Vec<f64> = answer.iter().map(|event| event.time).collect();
        answered.sort_by(f64::total_cmp);
        let step = StepTimes {
            start,
            region_end,
            seen: inside.iter().map(|event| event.time).collect(),
            answered,
            shift,
        };
        for chapter in chapters.iter_mut() {
            *chapter = move_chapter(*chapter, &step);
        }

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

        if *end >= start {
            *end += shift;
        }
        *end = end.max(last_time(&result)).max(0.0);
        Ok(result)
    }

    /// Run `filters` in order over `events`, by the steps on `FilterFn`. The
    /// end of the timeline starts at the time of the last event.
    ///
    /// # Errors
    /// Returns `NleError::RenderError` when a filter kind is not registered,
    /// when a filter answers a time that is not finite or is below zero on
    /// the timeline, or when an event time or the end of the timeline that the
    /// filters give is not finite.
    pub fn run(
        &self,
        events: Vec<Event>,
        filters: &[AppliedFilter],
    ) -> Result<Vec<Event>, NleError> {
        let end = last_time(&events);
        self.run_with_end(events, filters, end, &mut [])
            .map(|(events, _end)| events)
    }

    /// Run `filters` in order, and follow the end of the timeline, which
    /// starts at `end`. The chapter times in `chapters` move through the
    /// filters. A chapter time can become infinite, and the caller drops it
    /// with the chapters after the end. Returns the events and the final end.
    ///
    /// # Errors
    /// Returns `NleError::RenderError` when a filter fails, or when an event
    /// time or the final end is not finite.
    fn run_with_end(
        &self,
        events: Vec<Event>,
        filters: &[AppliedFilter],
        end: f64,
        chapters: &mut [f64],
    ) -> Result<(Vec<Event>, f64), NleError> {
        let mut events = events;
        let mut end = end;
        for applied in filters {
            events = self.apply(events, applied, &mut end, chapters)?;
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
    /// 3. The filters of the project run on them by the steps on `FilterFn`,
    ///    with the end of the timeline starting at `EditProject::duration`.
    /// 4. The header (`version`, `width`, `height`, `timestamp`, `command`,
    ///    `title`, `env`) is the header of the source of the clip that starts
    ///    first on the timeline. A tie goes to the clip that comes first in
    ///    track order, then in clip order.
    /// 5. `duration` is the end of the timeline after the filters.
    /// 6. Each event time and the duration must be finite. Else the render
    ///    fails with `NleError::RenderError`.
    /// 7. The chapters are the chapters of the project, moved through the
    ///    filters. A chapter after the duration is dropped. A chapter time that
    ///    overflows to infinity is after the duration too. The others are in
    ///    time order, and two at one time keep the order of
    ///    `EditProject::chapters_in_order`.
    ///
    /// A project from `EditProject::from_recording` with no filter renders a
    /// recording equal to its source, when the chapters of the source are in
    /// time order and none is after its duration.
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
        let markers = project.chapters_in_order();
        let mut chapter_times: Vec<f64> = markers.iter().map(|marker| marker.time.get()).collect();
        let (events, end) = self.run_with_end(
            events,
            &project.filters,
            project.duration(),
            &mut chapter_times,
        )?;
        let mut chapters: Vec<Chapter> = markers
            .iter()
            .zip(&chapter_times)
            .filter(|(_, time)| **time <= end)
            .map(|(marker, time)| Chapter {
                time: *time,
                label: marker.label.clone(),
            })
            .collect();
        // A stable sort: two chapters at one time keep their order.
        chapters.sort_by(|a, b| a.time.total_cmp(&b.time));

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
            chapters,
        })
    }
}

#[cfg(test)]
mod tests;
