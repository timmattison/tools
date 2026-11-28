//! The splice of the clips of a project into one stream of events, before the
//! filters: the order in which the clips play, the part of each clip that
//! plays, the hard cut at the start of each clip after the first, and the
//! size of the terminal that holds every clip.

use crate::pipeline::TerminalSize;
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

/// The largest width and the largest height of the sources of `clips`, or
/// `None` when there is no clip.
fn largest_size<'a>(clips: impl IntoIterator<Item = &'a Clip>) -> Option<TerminalSize> {
    clips
        .into_iter()
        .map(|clip| TerminalSize {
            width: clip.source.width,
            height: clip.source.height,
        })
        .reduce(|a, b| TerminalSize {
            width: a.width.max(b.width),
            height: a.height.max(b.height),
        })
}

/// The clips of a project, spliced into one stream of events.
pub(crate) struct Spliced<'a> {
    /// The events on the timeline, in time order, with the cuts.
    pub(crate) events: Vec<Event>,
    /// The end of the timeline.
    pub(crate) end: f64,
    /// The size of the terminal that holds every clip.
    pub(crate) size: TerminalSize,
    /// The clip whose source gives the header of the recording.
    pub(crate) header: &'a Clip,
}

/// Splice the clips of `project` into one stream of events. The answer is
/// `None` for a project with no clip on a track.
///
/// The rules:
/// 1. The clips play in `play_order`.
/// 2. The later start wins. A clip plays from its start to its end on the
///    timeline (`timeline_offset + out_point - in_point`), or to the start of
///    the next clip in the play order when that comes first. Thus the start
///    of a clip cuts each clip that still plays at that time, and the rest of
///    the earlier clip does not play. A tie of two starts goes to the clip
///    later in the play order.
/// 3. A clip whose part does not end after its start plays nothing. It gets
///    no cut, and its events do not play.
/// 4. A clip that plays holds the events of `Clip::timeline_events`. A clip
///    that a later start cuts holds only the events before that start.
/// 5. The terminal has the largest width and the largest height of the
///    clips.
/// 6. Each clip that plays, after the first one, starts with a hard cut: one
///    output event at its start that resets the attributes, sets the scroll
///    region, clears the screen, and moves the cursor home.
/// 7. The scroll region of a clip with fewer rows than the terminal holds the
///    rows of the clip, so the clip scrolls in its own rows. The first clip
///    that plays gets that scroll region in one output event at its start.
///    The cut into a clip with all the rows resets the scroll region. A
///    narrower clip keeps the whole width, because the terminal has no left
///    and right margins.
/// 8. The events of each clip come after the events of the clips before it in
///    the play order. Then a stable sort puts them in time order. Thus an
///    event of a clip at the start of the next clip plays before the cut.
/// 9. The end of the timeline is the latest end of the part of a clip, or 0
///    for a project whose clips all start at 0 and play nothing. With no
///    overlap, it is `EditProject::duration`.
/// 10. The header is the first clip that plays, or the first clip in the play
///     order when no clip plays.
pub(crate) fn splice(project: &EditProject) -> Option<Spliced<'_>> {
    let order = play_order(project);
    let first = *order.first()?;
    let size = largest_size(order.iter().copied())?;
    let mut header = None;
    let mut events = Vec::new();
    let mut end = 0.0_f64;
    for (index, clip) in order.iter().enumerate() {
        let start = clip.timeline_offset.get();
        let full_end = start + (clip.out_point.get() - clip.in_point.get());
        // The next clip starts at or after this start, so the part never
        // ends before its start.
        let part_end = order
            .get(index + 1)
            .map_or(full_end, |next| full_end.min(next.timeline_offset.get()));
        end = end.max(part_end);
        if part_end <= start {
            continue;
        }
        let rows = clip.source.height;
        let shorter = rows < size.height;
        let opening = if header.is_some() {
            Some(cut_text(shorter.then_some(rows)))
        } else {
            shorter.then(|| scroll_region(rows))
        };
        if let Some(data) = opening {
            events.push(Event {
                time: start,
                event_type: EventType::Output,
                data,
            });
        }
        header.get_or_insert(*clip);
        let cut = part_end < full_end;
        events.extend(
            clip.timeline_events()
                .filter(|event| !cut || event.time < part_end),
        );
    }
    // A stable sort keeps the play order on a tie.
    events.sort_by(|a, b| a.time.total_cmp(&b.time));
    Some(Spliced {
        events,
        end,
        size,
        header: header.unwrap_or(first),
    })
}

/// The text of the output event of a hard cut. `rows` is the height of the
/// scroll region for a clip with fewer rows than the terminal, or `None` for
/// the whole screen.
fn cut_text(rows: Option<u16>) -> String {
    let region = rows.map_or_else(|| RESET_SCROLL_REGION.to_owned(), scroll_region);
    format!("{RESET_ATTRIBUTES}{region}{CLEAR_SCREEN}{CURSOR_HOME}")
}

/// The sequence that sets the scroll region to the rows from 1 to `rows`
/// (DECSTBM).
fn scroll_region(rows: u16) -> String {
    format!("\x1b[1;{rows}r")
}
