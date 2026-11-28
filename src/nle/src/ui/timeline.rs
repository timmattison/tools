//! The view of the timeline: the zoom buttons, the ruler, the tracks with a
//! block for each clip, the event density of each clip, and the marks where
//! the terminal size changes.
//!
//! The view shows project time, before the filters. `App` holds the zoom and
//! the scroll, so a new recording opens with the whole project in the window.
//! The layout math is in `crate::timeline`, and this module only draws it and
//! turns the events of the webview into changes of the view.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use dioxus::html::geometry::WheelDelta;
use dioxus::html::{Key, Modifiers};
use dioxus::prelude::*;

use super::{Document, UNTITLED};
use crate::format::{format_clock, format_duration};
use crate::timeline::{
    density_path, key_action, ruler_ticks, size_marks, tile_width, visible_tiles, wheel_action,
    ClipEvents, EventCache, Extent, Frame, KeyAction, SizeMark, View, Wheel, WheelAction,
    WheelUnit, TILE_WIDTH, ZOOM_STEP,
};
use crate::types::{Clip, EditProject};

/// The width of the view in pixels until the first resize event gives the
/// real width. The window opens a little wider than this width.
const START_WIDTH: f64 = 1200.0;
/// The height of the event density of a clip, in the units of its path. The
/// style sheet sets the height on the screen, and the path scales to it.
const DENSITY_HEIGHT: f64 = 32.0;
/// The accessible label of the button that zooms out.
const ZOOM_OUT_LABEL: &str = "Zoom out";
/// The accessible label of the button that zooms in.
const ZOOM_IN_LABEL: &str = "Zoom in";
/// The accessible label of the button that shows the whole project.
const FIT_LABEL: &str = "Show the whole timeline";
/// The accessible label of the area of the ruler and the tracks.
const VIEW_LABEL: &str = "Tracks of the timeline";
/// The class of a tick of the ruler with no label.
const TICK_CLASS: &str = "timeline-tick";
/// The class of a tick of the ruler with a label.
const MAJOR_TICK_CLASS: &str = "timeline-tick timeline-tick-major";
/// The text before the number of a track.
pub(super) const TRACK_PREFIX: &str = "Track";

/// The view of the timeline of the open document. `view` holds the zoom and
/// the scroll.
///
/// The area of the ruler and the tracks takes the focus, so the keys `+`,
/// `-`, and `0` work on it. Cmd or Ctrl with the wheel zooms at the pointer,
/// and a horizontal wheel scrolls (see `crate::timeline::wheel_action`).
#[component]
pub(super) fn TimelineView(loaded: ReadSignal<Option<Document>>, view: Signal<View>) -> Element {
    let mut width = use_signal(|| START_WIDTH);
    // The left edge of the area in the coordinates of the window. A wheel
    // event gives the place of the pointer in the same coordinates.
    let left = use_signal(|| 0.0);
    let mut area = use_signal(|| None::<Rc<MountedData>>);
    let cache = use_hook(|| Rc::new(RefCell::new(EventCache::default())));

    let document = loaded.read();
    let Some(open) = document.as_ref() else {
        return rsx! {};
    };
    let project = open.editor.project();
    let duration = project.duration();
    let frame = view().frame(Extent::new(duration, width()));
    let shift = frame.x_of(0.0);
    let pixels_per_second = frame.pixels_per_second();
    let ticks: Vec<(f64, &str, Option<String>)> = ruler_ticks(frame)
        .into_iter()
        .map(|tick| {
            let class = if tick.label.is_some() {
                MAJOR_TICK_CLASS
            } else {
                TICK_CLASS
            };
            (tick.x, class, tick.label)
        })
        .collect();
    let events = cache.borrow_mut().events(&project.clips);
    let rows = track_rows(project, &events, frame);
    let marks: Vec<MarkRow> = size_marks(project)
        .iter()
        .filter_map(|mark| MarkRow::of(mark, frame))
        .collect();

    // Each handler reads the width when the event comes, so a resize that
    // has not rendered yet still counts.
    let zoom_at_center = move |factor: f64| {
        let mut view = view;
        let extent = Extent::new(duration, width());
        view.set(view().zoom(extent, factor, width() / 2.0));
    };

    rsx! {
        div { class: "timeline-zoom",
            button {
                aria_label: ZOOM_OUT_LABEL,
                title: "{ZOOM_OUT_LABEL} (-)",
                onclick: move |_| zoom_at_center(1.0 / ZOOM_STEP),
                "\u{2212}"
            }
            button {
                aria_label: ZOOM_IN_LABEL,
                title: "{ZOOM_IN_LABEL} (+)",
                onclick: move |_| zoom_at_center(ZOOM_STEP),
                "+"
            }
            button {
                aria_label: FIT_LABEL,
                title: "{FIT_LABEL} (0)",
                onclick: move |_| {
                    let mut view = view;
                    view.set(View::default());
                },
                "Fit"
            }
        }
        div {
            class: "timeline-view",
            tabindex: "0",
            role: "group",
            aria_label: VIEW_LABEL,
            onmounted: move |event| {
                let data = event.data();
                area.set(Some(Rc::clone(&data)));
                spawn(read_left(data, left));
            },
            onresize: move |event| {
                if let Ok(size) = event.get_content_box_size() {
                    width.set(size.width);
                }
                // A change of the layout can also move the left edge.
                if let Some(data) = area.peek().clone() {
                    spawn(read_left(data, left));
                }
            },
            onwheel: move |event| {
                let modifiers = event.modifiers();
                let (x, y, unit) = match event.delta() {
                    WheelDelta::Pixels(delta) => (delta.x, delta.y, WheelUnit::Pixels),
                    WheelDelta::Lines(delta) => (delta.x, delta.y, WheelUnit::Lines),
                    WheelDelta::Pages(delta) => (delta.x, delta.y, WheelUnit::Pages),
                };
                let wheel = Wheel {
                    x,
                    y,
                    unit,
                    command: modifiers.contains(Modifiers::CONTROL)
                        || modifiers.contains(Modifiers::META),
                    shift: modifiers.contains(Modifiers::SHIFT),
                };
                match wheel_action(wheel, width()) {
                    WheelAction::Zoom(factor) => {
                        event.prevent_default();
                        let anchor = event.client_coordinates().x - left();
                        let mut view = view;
                        let extent = Extent::new(duration, width());
                        view.set(view().zoom(extent, factor, anchor));
                    }
                    WheelAction::Pan(pixels) => {
                        event.prevent_default();
                        let mut view = view;
                        let extent = Extent::new(duration, width());
                        view.set(view().pan(extent, pixels));
                    }
                    WheelAction::Nothing => {}
                }
            },
            onkeydown: move |event| {
                let Key::Character(text) = event.key() else {
                    return;
                };
                let modifiers = event.modifiers();
                let command = modifiers.contains(Modifiers::CONTROL)
                    || modifiers.contains(Modifiers::META)
                    || modifiers.contains(Modifiers::ALT);
                let Some(action) = key_action(&text, command) else {
                    return;
                };
                event.prevent_default();
                match action {
                    KeyAction::ZoomIn => zoom_at_center(ZOOM_STEP),
                    KeyAction::ZoomOut => zoom_at_center(1.0 / ZOOM_STEP),
                    KeyAction::Fit => {
                        let mut view = view;
                        view.set(View::default());
                    }
                }
            },
            div { class: "timeline-ruler",
                // Two ticks of a very long project can share one place, so
                // the key is the position in the list.
                for (index, (x, class, label)) in ticks.into_iter().enumerate() {
                    div { key: "{index}", class, style: "left: {x}px;",
                        if let Some(label) = label {
                            span { class: "timeline-tick-label", "{label}" }
                        }
                    }
                }
            }
            div { class: "timeline-tracks",
                for row in rows {
                    div { key: "{row.key}", class: "timeline-track",
                        span { class: "timeline-track-label", "{row.label}" }
                        div {
                            class: "timeline-content",
                            style: "transform: translateX({shift}px);",
                            for block in row.clips {
                                div {
                                    key: "{block.key}",
                                    class: "timeline-clip",
                                    style: "left: {block.left}px; width: {block.width}px;",
                                    title: "{block.title}",
                                    span {
                                        class: "timeline-clip-label",
                                        style: "left: {block.label_left}px;",
                                        "{block.label}"
                                    }
                                    for index in block.tiles {
                                        Tile {
                                            key: "{index}",
                                            events: block.events.clone(),
                                            index,
                                            pixels_per_second,
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // Two marks can share one place, so the key is the position in
            // the list.
            for (index, mark) in marks.into_iter().enumerate() {
                div {
                    key: "{index}",
                    class: "size-mark",
                    style: "left: {mark.x}px;",
                    span {
                        class: "size-mark-label",
                        title: "{mark.description}",
                        aria_label: "{mark.description}",
                        "{mark.text}"
                    }
                }
            }
        }
    }
}

/// Reads the left edge of the area of `data` into `left`. A read that fails
/// keeps the old value.
async fn read_left(data: Rc<MountedData>, mut left: Signal<f64>) {
    if let Ok(rect) = data.get_client_rect().await {
        left.set(rect.origin.x);
    }
}

#[cfg(test)]
thread_local! {
    /// The number of renders of `Tile` on this thread, for the tests. Each test
    /// runs on its own thread and renders its own `VirtualDom` there.
    static TILE_RENDERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One tile of a clip: the event density of `TILE_WIDTH` pixels of the clip
/// as one SVG path. The props change only with a zoom or with new events, so
/// a scroll renders no tile again.
#[component]
fn Tile(events: ClipEvents, index: usize, pixels_per_second: f64) -> Element {
    #[cfg(test)]
    TILE_RENDERS.with(|renders| renders.set(renders.get() + 1));
    let width = tile_width(events.length() * pixels_per_second, index);
    let left = index as f64 * TILE_WIDTH;
    let path = density_path(&events, index, pixels_per_second, DENSITY_HEIGHT);
    rsx! {
        svg {
            class: "timeline-tile",
            style: "left: {left}px; width: {width}px;",
            view_box: "0 0 {width} {DENSITY_HEIGHT}",
            preserve_aspect_ratio: "none",
            path { d: "{path}" }
        }
    }
}

/// A track as the view draws it.
struct TrackRow {
    /// The id of the track, as the key of its row.
    key: u64,
    /// The name of the track: `Track` and its number from 1.
    label: String,
    /// The blocks of the clips of the track.
    clips: Vec<ClipBlock>,
}

/// A clip as the view draws it.
struct ClipBlock {
    /// The id of the clip, as the key of its block.
    key: u64,
    /// The start of the block from time zero, in pixels.
    left: f64,
    /// The width of the block, in pixels.
    width: f64,
    /// The title of the source recording.
    label: String,
    /// The text that shows when the pointer is on the block.
    title: String,
    /// The place of the label in the block. It keeps the label in the window
    /// when the start of the block is to the left of the window.
    label_left: f64,
    /// The tiles of the block that render.
    tiles: Range<usize>,
    /// The events of the clip.
    events: ClipEvents,
}

/// The rows of the tracks of `project` in `frame`. `events` holds the events
/// of each clip of the project, in the order of its clips.
fn track_rows(project: &EditProject, events: &[ClipEvents], frame: Frame) -> Vec<TrackRow> {
    let clips = &project.clips;
    project
        .timeline
        .iter()
        .zip(1_u32..)
        .map(|(track, number)| TrackRow {
            key: track.id.0,
            label: format!("{TRACK_PREFIX} {number}"),
            clips: track
                .clips
                .iter()
                .filter_map(|id| clips.iter().zip(events).find(|(clip, _)| clip.id == *id))
                .map(|(clip, events)| ClipBlock::of(clip, events.clone(), frame))
                .collect(),
        })
        .collect()
}

impl ClipBlock {
    /// The block of `clip` with `events` in `frame`.
    fn of(clip: &Clip, events: ClipEvents, frame: Frame) -> Self {
        let pixels_per_second = frame.pixels_per_second();
        let start = clip.timeline_offset.get();
        let width = events.length() * pixels_per_second;
        let x = frame.x_of(start);
        let label = if clip.source.title.is_empty() {
            UNTITLED.to_owned()
        } else {
            clip.source.title.clone()
        };
        let title = format!(
            "{label}: {} to {} of the recording",
            format_duration(clip.in_point.get()),
            format_duration(clip.out_point.get())
        );
        Self {
            key: clip.id.0,
            left: start * pixels_per_second,
            width,
            title,
            label,
            label_left: (-x).max(0.0),
            tiles: visible_tiles(x, width, frame.width()),
            events,
        }
    }
}

/// A mark of a size change as the view draws it.
struct MarkRow {
    /// The place of the mark in the window, in pixels.
    x: f64,
    /// The new size, as the label of the mark.
    text: String,
    /// The change in one sentence, for the pointer and for a screen reader.
    description: String,
}

impl MarkRow {
    /// The row of `mark` in `frame`, or `None` for a mark outside the window.
    fn of(mark: &SizeMark, frame: Frame) -> Option<Self> {
        let x = frame.x_of(mark.time);
        if !(0.0..=frame.width()).contains(&x) {
            return None;
        }
        let (from, to) = (mark.from, mark.to);
        Some(Self {
            x,
            text: format!("{}x{}", to.0, to.1),
            description: format!(
                "The terminal size changes from {}x{} to {}x{} at {}",
                from.0,
                from.1,
                to.0,
                to.1,
                format_clock(mark.time, true)
            ),
        })
    }
}

#[cfg(test)]
mod tests;
