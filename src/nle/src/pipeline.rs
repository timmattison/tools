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

    /// One filter step: the steps on `FilterFn`.
    fn apply(&self, events: Vec<Event>, applied: &AppliedFilter) -> Result<Vec<Event>, NleError> {
        let filter = self.lookup(&applied.kind)?;
        let (start, end) = match &applied.region {
            FilterRegion::All => (0.0, f64::INFINITY),
            FilterRegion::Range { start, end } => (start.get(), end.get()),
        };

        let mut before = Vec::new();
        let mut inside = Vec::new();
        let mut after = Vec::new();
        for event in events {
            if event.time < start {
                before.push(event);
            } else if event.time < end {
                inside.push(Event {
                    time: event.time - start,
                    ..event
                });
            } else {
                after.push(event);
            }
        }

        let answer = filter(&inside, &applied.options);
        let shift = last_time(&answer) - last_time(&inside);

        let mut result = before;
        result.extend(answer.into_iter().map(|event| Event {
            time: event.time + start,
            ..event
        }));
        result.extend(after.into_iter().map(|event| Event {
            time: event.time + shift,
            ..event
        }));
        Ok(result)
    }

    /// Run `filters` in order over `events`, by the steps on `FilterFn`. The
    /// end of the timeline starts at the time of the last event.
    ///
    /// # Errors
    /// Returns `NleError::RenderError` when a filter kind is not registered,
    /// or when a filter answers a time that is not finite or is below zero on
    /// the timeline.
    pub fn run(
        &self,
        events: Vec<Event>,
        filters: &[AppliedFilter],
    ) -> Result<Vec<Event>, NleError> {
        let mut events = events;
        for applied in filters {
            events = self.apply(events, applied)?;
        }
        Ok(events)
    }

    /// Flatten `project` into one recording.
    ///
    /// The rules:
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
    ///
    /// A project of one whole-recording clip and no filter renders a
    /// recording equal to its source.
    ///
    /// # Errors
    /// Returns `NleError::RenderError` for an empty project, and the errors of
    /// `FilterRegistry::run`.
    pub fn render(&self, project: &EditProject) -> Result<Recording, NleError> {
        let _ = project;
        todo!("render")
    }
}

#[cfg(test)]
mod tests;
