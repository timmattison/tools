//! Tests of the view of the timeline. Each test renders the view of a
//! document with the Dioxus SSR renderer, sends it events, and reads the page
//! as text.

use std::any::Any;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Once};

use dioxus::html::geometry::PixelsSize;
use dioxus::html::point_interaction::SerializedPointInteraction;
use dioxus::html::{
    Code, Key, Location, Modifiers, PlatformEventData, SerializedKeyboardData, SerializedMouseData,
    SerializedResizeData, SerializedWheelData,
};
use dioxus_core::{ElementId, Mutation};

use super::*;
use crate::editor::Edit;
use crate::timeline::{Extent, ZOOM_STEP};
use crate::types::{ClipId, Event, EventType, LoadedRecording, Recording, Seconds, TrackId};

/// The length of the fixture recording, in seconds.
const LENGTH: f64 = 60.0;
/// The title of the fixture recording.
const TITLE: &str = "Demo";

/// A recording of `LENGTH` seconds and `width` x `height` cells, with an
/// output event each half second.
fn recording(width: u16, height: u16) -> Recording {
    Recording {
        version: 2,
        width,
        height,
        timestamp: 0.0,
        duration: LENGTH,
        command: "echo".into(),
        title: TITLE.into(),
        env: BTreeMap::new(),
        events: (0..120)
            .map(|index| Event {
                time: f64::from(index) / 2.0,
                event_type: EventType::Output,
                data: "x".into(),
            })
            .collect(),
        chapters: Vec::new(),
    }
}

/// The loaded form of a recording of 80 x 24 cells.
fn loaded() -> LoadedRecording {
    let recording = recording(80, 24);
    LoadedRecording {
        metadata: recording.metadata("/tmp/demo.json"),
        recording,
    }
}

/// What the test root builds its document from: the edits that the editor of
/// the document applies to the project of `loaded()`.
#[derive(Clone)]
struct Edits(Vec<Edit>);

/// A test root that shows the view of the timeline of a document.
fn root() -> Element {
    let Edits(edits) = use_context::<Edits>();
    let loaded = use_signal(|| {
        let mut document =
            Document::open(loaded(), Arc::new(crate::filters::registry())).expect("a document");
        for edit in edits {
            document.editor.apply(edit).expect("an edit that applies");
        }
        Some(document)
    });
    let view = use_signal(View::default);
    rsx! {
        TimelineView { loaded, view }
    }
}

/// A rendered view of the timeline that a test sends events to.
struct Page {
    dom: VirtualDom,
    /// The name and the element of each listener, in creation order.
    listeners: Vec<(String, ElementId)>,
}

impl Page {
    /// Renders the view of a document whose editor applied `edits`.
    fn open(edits: Vec<Edit>) -> Self {
        static CONVERTER: Once = Once::new();
        CONVERTER.call_once(|| {
            dioxus::html::set_event_converter(Box::new(dioxus::html::SerializedHtmlEventConverter));
        });
        let mut dom = VirtualDom::new(root).with_root_context(Edits(edits));
        let edits = dom.rebuild_to_vec().edits;
        let mut page = Self {
            dom,
            listeners: Vec::new(),
        };
        page.note(&edits);
        page
    }

    /// Keeps each new listener of `edits`.
    fn note(&mut self, edits: &[Mutation]) {
        self.listeners
            .extend(edits.iter().filter_map(|edit| match edit {
                Mutation::NewEventListener { name, id } => Some((name.clone(), *id)),
                _ => None,
            }));
    }

    /// The elements with a listener of `name`, in creation order.
    fn with(&self, name: &str) -> Vec<ElementId> {
        self.listeners
            .iter()
            .filter(|(listener, _)| listener == name)
            .map(|(_, id)| *id)
            .collect()
    }

    /// The one element with a listener of `name`.
    fn one(&self, name: &str) -> ElementId {
        let ids = self.with(name);
        let [id] = ids[..] else {
            panic!("the page has the {name} listeners {ids:?}")
        };
        id
    }

    /// The Zoom out, Zoom in, and Fit buttons, in this order.
    fn buttons(&self) -> [ElementId; 3] {
        self.with("click")
            .try_into()
            .unwrap_or_else(|ids: Vec<ElementId>| panic!("the page has the buttons {ids:?}"))
    }

    /// Sends the event `name` with `data` to `id`, renders, and gives back
    /// the event, so a test can read its default action.
    fn send(
        &mut self,
        name: &str,
        data: Box<dyn Any>,
        id: ElementId,
    ) -> dioxus::prelude::Event<dyn Any> {
        let data = Rc::new(PlatformEventData::new(data));
        let event = dioxus::prelude::Event::new(data as Rc<dyn Any>, true);
        self.dom.runtime().handle_event(name, event.clone(), id);
        let edits = self.dom.render_immediate_to_vec().edits;
        self.note(&edits);
        event
    }

    /// Clicks the button `id`.
    fn click(&mut self, id: ElementId) {
        self.send("click", Box::<SerializedMouseData>::default(), id);
    }

    /// Sends a wheel event of `x` and `y` pixels at `client_x` in the view,
    /// with `modifiers`, and gives back the event.
    fn wheel(
        &mut self,
        x: f64,
        y: f64,
        client_x: f64,
        modifiers: Modifiers,
    ) -> dioxus::prelude::Event<dyn Any> {
        let data = SerializedWheelData {
            mouse: SerializedPointInteraction {
                client_x,
                ctrl_key: modifiers.contains(Modifiers::CONTROL),
                meta_key: modifiers.contains(Modifiers::META),
                shift_key: modifiers.contains(Modifiers::SHIFT),
                alt_key: modifiers.contains(Modifiers::ALT),
                ..SerializedPointInteraction::default()
            },
            delta_mode: 0,
            delta_x: x,
            delta_y: y,
            delta_z: 0.0,
        };
        let view = self.one("wheel");
        self.send("wheel", Box::new(data), view)
    }

    /// Sends a press of the key with the text `text`, with `modifiers`.
    fn key(&mut self, text: &str, modifiers: Modifiers) -> dioxus::prelude::Event<dyn Any> {
        let data = SerializedKeyboardData::new(
            Key::Character(text.into()),
            Code::Unidentified,
            Location::Standard,
            false,
            modifiers,
            false,
        );
        let view = self.one("keydown");
        self.send("keydown", Box::new(data), view)
    }

    /// Sends a resize of the view to `width` pixels.
    fn resize(&mut self, width: f64) {
        let size = PixelsSize::new(width, 160.0);
        let view = self.one("resize");
        self.send(
            "resize",
            Box::new(SerializedResizeData::new(size, size)),
            view,
        );
    }

    /// The page as text.
    fn html(&self) -> String {
        dioxus_ssr::render(&self.dom)
    }
}

/// The style of a clip block that starts at `start` seconds and lasts
/// `length` seconds in the frame of `view`.
fn clip_style(view: View, extent: Extent, start: f64, length: f64) -> String {
    let frame = view.frame(extent);
    let pixels_per_second = frame.pixels_per_second();
    format!(
        "left: {}px; width: {}px;",
        start * pixels_per_second,
        length * pixels_per_second
    )
}

/// The style of the content of a track in the frame of `view`.
fn content_style(view: View, extent: Extent) -> String {
    format!("transform: translateX({}px);", view.frame(extent).x_of(0.0))
}

/// The extent of the fixture in a view of `START_WIDTH` pixels.
fn start_extent() -> Extent {
    Extent::new(LENGTH, START_WIDTH)
}

/// A time in seconds.
fn seconds(value: f64) -> Seconds {
    Seconds::new(value).expect("a valid time")
}

#[test]
fn the_view_shows_a_ruler_a_track_and_a_block_for_the_clip() {
    let page = Page::open(Vec::new()).html();
    for label in ["0:00", "0:05", "0:30", "1:00"] {
        assert!(
            page.contains(&format!(">{label}</span>")),
            "{label} in {page}"
        );
    }
    assert!(page.contains(">Track 1<"), "{page}");
    assert!(page.contains("class=\"timeline-clip\""), "{page}");
    assert!(page.contains("left: 0px; width: 1200px;"), "{page}");
    assert!(page.contains(&format!(">{TITLE}<")), "{page}");
    assert!(page.contains("transform: translateX(0px);"), "{page}");
}

#[test]
fn each_tile_in_the_window_draws_its_density_as_one_path() {
    let page = Page::open(Vec::new()).html();
    // 1,200 pixels of clip are three tiles of up to 512 pixels.
    assert_eq!(page.matches("<svg").count(), 3, "{page}");
    assert_eq!(page.matches("<path").count(), 3, "{page}");
    // At 20 pixels per second, the event at 0.5 seconds is in column 10.
    assert!(page.contains("d=\"M0.5 32v-"), "{page}");
    assert!(page.contains("M10.5 32v-"), "{page}");
}

#[test]
fn only_the_tiles_near_the_window_render() {
    let mut page = Page::open(Vec::new());
    for _ in 0..40 {
        page.key("+", Modifiers::empty());
    }
    let html = page.html();
    // At 1,000 pixels per second the clip has 118 tiles, and the window and
    // its overscan hold four of them.
    assert!(html.contains("width: 60000px;"), "{html}");
    let tiles = html.matches("<svg").count();
    assert!((3..=5).contains(&tiles), "{tiles} tiles in {html}");
}

#[test]
fn the_zoom_buttons_zoom_at_the_center_and_fit_shows_the_whole_project() {
    let mut page = Page::open(Vec::new());
    let [zoom_out, zoom_in, fit] = page.buttons();
    let extent = start_extent();
    let center = START_WIDTH / 2.0;
    let zoomed = View::default().zoom(extent, ZOOM_STEP, center);

    page.click(zoom_in);
    let html = page.html();
    assert!(
        html.contains(&clip_style(zoomed, extent, 0.0, LENGTH)),
        "{html}"
    );
    assert!(html.contains(&content_style(zoomed, extent)), "{html}");

    page.click(zoom_in);
    page.click(zoom_out);
    let html = page.html();
    let back = zoomed
        .zoom(extent, ZOOM_STEP, center)
        .zoom(extent, 1.0 / ZOOM_STEP, center);
    assert!(
        html.contains(&clip_style(back, extent, 0.0, LENGTH)),
        "{html}"
    );

    page.click(fit);
    let html = page.html();
    assert!(html.contains("left: 0px; width: 1200px;"), "{html}");
    assert!(html.contains("transform: translateX(0px);"), "{html}");
}

#[test]
fn the_buttons_have_labels() {
    let page = Page::open(Vec::new()).html();
    for label in ["Zoom out", "Zoom in", "Show the whole timeline"] {
        assert!(
            page.contains(&format!("aria-label=\"{label}\"")),
            "{label} in {page}"
        );
    }
}

#[test]
fn a_wheel_with_command_zooms_at_the_pointer_and_stops_the_default_action() {
    let mut page = Page::open(Vec::new());
    let extent = start_extent();
    for modifier in [Modifiers::CONTROL, Modifiers::META] {
        let event = page.wheel(0.0, -5.0, 300.0, modifier);
        assert!(!event.default_action_enabled(), "{modifier:?}");
    }
    let expected =
        View::default()
            .zoom(extent, (0.3_f64).exp(), 300.0)
            .zoom(extent, (0.3_f64).exp(), 300.0);
    let html = page.html();
    assert!(
        html.contains(&clip_style(expected, extent, 0.0, LENGTH)),
        "{html}"
    );
    assert!(html.contains(&content_style(expected, extent)), "{html}");
}

#[test]
fn a_horizontal_wheel_scrolls_a_zoomed_view() {
    let mut page = Page::open(Vec::new());
    let extent = start_extent();
    page.key("+", Modifiers::empty());
    let event = page.wheel(100.0, 0.0, 300.0, Modifiers::empty());
    assert!(!event.default_action_enabled());
    let expected = View::default()
        .zoom(extent, ZOOM_STEP, START_WIDTH / 2.0)
        .pan(extent, 100.0);
    let html = page.html();
    assert!(html.contains(&content_style(expected, extent)), "{html}");
}

#[test]
fn a_vertical_wheel_with_no_modifier_keeps_its_default_action() {
    let mut page = Page::open(Vec::new());
    let before = page.html();
    let event = page.wheel(0.0, 40.0, 300.0, Modifiers::empty());
    assert!(event.default_action_enabled());
    assert_eq!(page.html(), before);
}

#[test]
fn the_keys_zoom_in_zoom_out_and_show_the_whole_project() {
    let mut page = Page::open(Vec::new());
    let extent = start_extent();
    let center = START_WIDTH / 2.0;
    let event = page.key("+", Modifiers::empty());
    assert!(!event.default_action_enabled());
    page.key("=", Modifiers::empty());
    page.key("-", Modifiers::empty());
    let expected = View::default()
        .zoom(extent, ZOOM_STEP, center)
        .zoom(extent, ZOOM_STEP, center)
        .zoom(extent, 1.0 / ZOOM_STEP, center);
    let html = page.html();
    assert!(
        html.contains(&clip_style(expected, extent, 0.0, LENGTH)),
        "{html}"
    );

    page.key("0", Modifiers::empty());
    assert!(page.html().contains("left: 0px; width: 1200px;"));
}

#[test]
fn a_key_with_command_keeps_the_view_and_its_default_action() {
    let mut page = Page::open(Vec::new());
    let before = page.html();
    for modifier in [Modifiers::CONTROL, Modifiers::META, Modifiers::ALT] {
        let event = page.key("+", modifier);
        assert!(event.default_action_enabled(), "{modifier:?}");
    }
    assert_eq!(page.html(), before);
}

#[test]
fn a_resize_fits_the_whole_project_into_the_new_width() {
    let mut page = Page::open(Vec::new());
    page.resize(600.0);
    let html = page.html();
    assert!(html.contains("left: 0px; width: 600px;"), "{html}");
    // At 10 pixels per second, a label each 5 seconds has no room.
    assert!(html.contains(">0:10</span>"), "{html}");
    assert!(!html.contains(">0:05</span>"), "{html}");
}

/// The edits that put a clip of a recording of `width` x `height` cells
/// after the first clip, on track 1.
fn second_clip(width: u16, height: u16) -> Edit {
    Edit::AddClip {
        clip: crate::types::Clip {
            id: ClipId(2),
            source: Arc::new(recording(width, height)),
            in_point: Seconds::ZERO,
            out_point: seconds(LENGTH),
            timeline_offset: seconds(LENGTH),
        },
        track: TrackId(1),
    }
}

#[test]
fn a_change_of_the_terminal_size_shows_a_mark_at_the_clip() {
    let page = Page::open(vec![second_clip(120, 40)]).html();
    // Two minutes in 1,200 pixels put the second clip at 600 pixels.
    assert!(
        page.contains("class=\"size-mark\" style=\"left: 600px;\""),
        "{page}"
    );
    assert!(page.contains(">120x40</span>"), "{page}");
    assert!(
        page.contains("The terminal size changes from 80x24 to 120x40 at 1:00.0"),
        "{page}"
    );
    assert!(page.contains("left: 600px; width: 600px;"), "{page}");
}

/// The edits that put a clip of `width` x `height` cells at the end of the
/// first clip, on a new track with the id `track`.
fn clip_on_new_track(track: u64, width: u16, height: u16) -> [Edit; 2] {
    [
        Edit::AddTrack { id: TrackId(track) },
        Edit::AddClip {
            clip: crate::types::Clip {
                id: ClipId(track),
                source: Arc::new(recording(width, height)),
                in_point: Seconds::ZERO,
                out_point: seconds(LENGTH),
                timeline_offset: seconds(LENGTH),
            },
            track: TrackId(track),
        },
    ]
}

#[test]
fn two_size_changes_at_one_time_show_two_marks_after_a_new_render() {
    let edits = [clip_on_new_track(2, 100, 30), clip_on_new_track(3, 120, 40)].concat();
    let mut page = Page::open(edits);
    // The zoom keeps the time of the marks, the center, in the window, and
    // the marks render again.
    page.key("+", Modifiers::empty());
    let html = page.html();
    assert_eq!(html.matches("class=\"size-mark\"").count(), 2, "{html}");
    assert!(html.contains(">100x30</span>"), "{html}");
    assert!(html.contains(">120x40</span>"), "{html}");
}

#[test]
fn the_ruler_of_a_very_long_project_renders_again_with_no_panic() {
    // Near 1.6e14 seconds, an `f64` holds a time only to 1/32 of a second,
    // so two ticks 1/50 of a second apart can get one place.
    let length = 1.6e14;
    let mut long = recording(80, 24);
    long.duration = length;
    let edits = vec![
        Edit::AddTrack { id: TrackId(2) },
        Edit::AddClip {
            clip: crate::types::Clip {
                id: ClipId(2),
                source: Arc::new(long),
                in_point: Seconds::ZERO,
                out_point: seconds(length),
                timeline_offset: Seconds::ZERO,
            },
            track: TrackId(2),
        },
    ];
    let mut page = Page::open(edits);
    for _ in 0..200 {
        page.key("+", Modifiers::empty());
    }
    page.wheel(1e18, 0.0, 0.0, Modifiers::empty());
    page.key("-", Modifiers::empty());
    page.key("+", Modifiers::empty());
    assert!(page.html().contains("timeline-tick"));
}

#[test]
fn clips_of_one_size_show_no_mark() {
    let page = Page::open(vec![second_clip(80, 24)]).html();
    assert!(!page.contains("size-mark"), "{page}");
}

#[test]
fn a_second_track_shows_its_own_row_and_clip() {
    let clip = crate::types::Clip {
        id: ClipId(2),
        source: Arc::new(recording(80, 24)),
        in_point: seconds(10.0),
        out_point: seconds(40.0),
        timeline_offset: seconds(15.0),
    };
    let page = Page::open(vec![
        Edit::AddTrack { id: TrackId(2) },
        Edit::AddClip {
            clip,
            track: TrackId(2),
        },
    ])
    .html();
    assert_eq!(
        page.matches("class=\"timeline-track\"").count(),
        2,
        "{page}"
    );
    assert!(page.contains(">Track 2<"), "{page}");
    assert!(page.contains("left: 300px; width: 600px;"), "{page}");
}

#[test]
fn the_clip_shows_its_range_when_the_pointer_is_on_it() {
    let trim = Edit::TrimClip {
        id: ClipId(1),
        in_point: seconds(5.0),
        out_point: seconds(LENGTH),
    };
    let page = Page::open(vec![trim]).html();
    assert!(
        page.contains("title=\"Demo: 0m 5.0s to 1m 0.0s of the recording\""),
        "{page}"
    );
}

/// The start of the tag of the label of a clip whose label is `left` pixels
/// from the start of the block.
fn label_tag(left: f64) -> String {
    format!("class=\"timeline-clip-label\" style=\"left: {left}px;\"")
}

#[test]
fn the_view_takes_the_focus_and_the_label_of_a_clip_stays_in_the_window() {
    let mut page = Page::open(Vec::new());
    let html = page.html();
    assert!(
        html.contains("class=\"timeline-view\" tabindex=\"0\""),
        "{html}"
    );
    assert!(html.contains(&label_tag(0.0)), "{html}");

    let extent = start_extent();
    page.key("+", Modifiers::empty());
    page.wheel(100.0, 0.0, 300.0, Modifiers::empty());
    let view = View::default().zoom(extent, ZOOM_STEP, START_WIDTH / 2.0);
    let start = view.pan(extent, 100.0).frame(extent).x_of(0.0);
    assert!(start < 0.0, "the clip starts left of the window: {start}");
    let html = page.html();
    assert!(html.contains(&label_tag(-start)), "{html}");
}

#[test]
fn a_scroll_renders_no_tile_again() {
    let extent = start_extent();
    let mut page = Page::open(Vec::new());
    page.key("+", Modifiers::empty());
    let before = TILE_RENDERS.with(std::cell::Cell::get);
    assert!(before > 0, "the tiles rendered");

    page.wheel(10.0, 0.0, 300.0, Modifiers::empty());

    let view = View::default()
        .zoom(extent, ZOOM_STEP, START_WIDTH / 2.0)
        .pan(extent, 10.0);
    let html = page.html();
    assert!(html.contains(&content_style(view, extent)), "{html}");
    assert_eq!(html.matches("<svg").count(), 3, "{html}");
    assert_eq!(TILE_RENDERS.with(std::cell::Cell::get), before);
}
