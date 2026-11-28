//! The clip list of the properties panel: the clips of each track in their
//! play order, with a Move Earlier button and a Move Later button for each.
//!
//! A move applies `Edit::ReorderClip`, so it puts the clips of the track back
//! to back in the new order, as one undo step.

use dioxus::prelude::*;

use super::timeline::TRACK_PREFIX;
use super::{edit_document, report, Document, UNTITLED};
use crate::editor::Edit;
use crate::format::format_duration;
use crate::types::ClipId;

/// The heading of the clip list.
const CLIPS_HEADING: &str = "Clips";
/// The hint of the clip list.
const CLIPS_HINT: &str =
    "The clips of a track play in this order. A move puts them back to back.";
/// The text of the button that moves a clip one place earlier.
const MOVE_EARLIER_LABEL: &str = "Move Earlier";
/// The text of the button that moves a clip one place later.
const MOVE_LATER_LABEL: &str = "Move Later";
/// The start of the message for a move that does not apply.
const MOVE_FAILED_PREFIX: &str = "Failed to move the clip";

/// The clips of one track as the list shows them.
struct TrackClips {
    /// The id of the track, as the key of its group.
    key: u64,
    /// The name of the track: `Track` and its number from 1.
    label: String,
    /// The rows of the clips, in their play order.
    rows: Vec<ClipRow>,
}

/// A clip as the list shows it.
struct ClipRow {
    /// The clip.
    id: ClipId,
    /// The text of the row: the place from 1, the title, and the length.
    text: String,
    /// The place of a Move Earlier, or `None` for the first clip.
    earlier: Option<usize>,
    /// The place of a Move Later, or `None` for the last clip.
    later: Option<usize>,
}

/// The clip list of the open document. `on_result` gets the outcome of each
/// move: `Ok`, or the message of the banner.
#[component]
pub(super) fn ClipsPanel(
    loaded: Signal<Option<Document>>,
    on_result: EventHandler<Result<(), String>>,
) -> Element {
    let tracks = track_clips(&loaded.read());
    rsx! {
        div { class: "clips-panel",
            h3 { {CLIPS_HEADING} }
            p { class: "clips-hint", {CLIPS_HINT} }
            for track in tracks {
                div { class: "clip-track", key: "{track.key}",
                    span { class: "clip-track-label", "{track.label}" }
                    for row in track.rows {
                        div { class: "clip-row", key: "{row.id.0}",
                            span { class: "clip-row-text", "{row.text}" }
                            button {
                                disabled: row.earlier.is_none(),
                                onclick: move |_| move_clip(loaded, on_result, row.id, row.earlier),
                                {MOVE_EARLIER_LABEL}
                            }
                            button {
                                disabled: row.later.is_none(),
                                onclick: move |_| move_clip(loaded, on_result, row.id, row.later),
                                {MOVE_LATER_LABEL}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Puts clip `id` at `index` in the play order of its track, and sends the
/// outcome to `on_result`. With no `index`, nothing changes.
fn move_clip(
    loaded: Signal<Option<Document>>,
    on_result: EventHandler<Result<(), String>>,
    id: ClipId,
    index: Option<usize>,
) {
    let Some(index) = index else {
        return;
    };
    let outcome = edit_document(loaded, |_| Ok(Edit::ReorderClip { id, index }));
    report(on_result, MOVE_FAILED_PREFIX, outcome);
}

/// The groups of the list: one for each track of the project that holds a
/// clip, in track order. A track keeps the number of its place in the
/// timeline, also when a track before it holds no clip.
fn track_clips(document: &Option<Document>) -> Vec<TrackClips> {
    let Some(open) = document else {
        return Vec::new();
    };
    let project = open.editor.project();
    project
        .timeline
        .iter()
        .zip(1_u32..)
        .filter(|(track, _)| !track.clips.is_empty())
        .map(|(track, number)| {
            let order = project.track_order(track);
            let last = order.len().saturating_sub(1);
            let rows = order
                .iter()
                .enumerate()
                .map(|(index, clip)| {
                    let title = if clip.source.title.is_empty() {
                        UNTITLED
                    } else {
                        clip.source.title.as_str()
                    };
                    let length = clip.out_point.get() - clip.in_point.get();
                    ClipRow {
                        id: clip.id,
                        text: format!("{}. {title}, {}", index + 1, format_duration(length)),
                        earlier: index.checked_sub(1),
                        later: (index < last).then_some(index + 1),
                    }
                })
                .collect();
            TrackClips {
                key: track.id.0,
                label: format!("{TRACK_PREFIX} {number}"),
                rows,
            }
        })
        .collect()
}
