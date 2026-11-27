//! The splice of the clips of a project into one stream of events, before the
//! filters: the order in which the clips play, and the hard cut at the start
//! of each clip after the first.

use crate::types::{Clip, EditProject, Event, EventType};

/// The sequence that resets the attributes of the text (SGR 0).
const RESET_ATTRIBUTES: &str = "\x1b[0m";
/// The sequence that resets the scroll region to the whole screen (DECSTBM
/// with no parameter).
const RESET_SCROLL_REGION: &str = "\x1b[r";
/// The sequence that clears the whole screen (ED 2).
const CLEAR_SCREEN: &str = "\x1b[2J";
/// The sequence that moves the cursor to the top-left cell (CUP).
const CURSOR_HOME: &str = "\x1b[H";

/// The clips of `project` in the order that they play: by their start on the
/// timeline. A tie keeps track order, then the order on the track.
pub(crate) fn play_order(project: &EditProject) -> Vec<&Clip> {
    let mut ordered: Vec<&Clip> = project
        .timeline
        .iter()
        .flat_map(|track| &track.clips)
        .filter_map(|id| project.clips.iter().find(|clip| clip.id == *id))
        .collect();
    // A stable sort keeps track order, then clip order, on a tie.
    ordered.sort_by(|a, b| a.timeline_offset.get().total_cmp(&b.timeline_offset.get()));
    ordered
}

/// The clips of a project, spliced into one stream of events.
pub(crate) struct Spliced<'a> {
    /// The events on the timeline, in time order, with the cuts.
    pub(crate) events: Vec<Event>,
    /// The end of the timeline.
    pub(crate) end: f64,
    /// The clip whose source gives the header of the recording.
    pub(crate) header: &'a Clip,
}

/// Splice the clips of `project` into one stream of events. The answer is
/// `None` for a project with no clip on a track.
///
/// The rules:
/// 1. The clips play in `play_order`.
/// 2. Each clip after the first starts with a hard cut: one output event at
///    its start that resets the attributes and the scroll region, clears the
///    screen, and moves the cursor home.
/// 3. The events of each clip come after the events of the clips before it in
///    the play order. Then a stable sort puts them in time order. Thus an
///    event of a clip at the start of the next clip plays before the cut.
/// 4. The end of the timeline is `EditProject::duration`.
/// 5. The header is the clip that plays first.
pub(crate) fn splice(project: &EditProject) -> Option<Spliced<'_>> {
    let order = play_order(project);
    let header = *order.first()?;
    let mut events = Vec::new();
    for (index, clip) in order.iter().enumerate() {
        if index > 0 {
            events.push(Event {
                time: clip.timeline_offset.get(),
                event_type: EventType::Output,
                data: cut_text(),
            });
        }
        events.extend(clip.timeline_events());
    }
    // A stable sort keeps the play order on a tie.
    events.sort_by(|a, b| a.time.total_cmp(&b.time));
    Some(Spliced {
        events,
        end: project.duration(),
        header,
    })
}

/// The text of the output event of a hard cut.
fn cut_text() -> String {
    format!("{RESET_ATTRIBUTES}{RESET_SCROLL_REGION}{CLEAR_SCREEN}{CURSOR_HOME}")
}
