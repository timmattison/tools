//! Tests of the regions of the window. Each test renders the components to
//! text with the Dioxus SSR renderer and reads that text.

use super::*;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use crate::format::format_recorded;
use crate::preview::PreviewTheme;
use crate::types::{Event, EventType, Recording, RecordingMetadata};
use dioxus_core::{ElementId, Mutation};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::sync::oneshot;

use crate::editor::Edit;
use crate::error::NleError;
use crate::types::{ClipId, Seconds};

const FIXTURE_PATH: &str = "/home/user/recordings/demo.json";
/// The number of events in the fixture.
const EVENT_COUNT: u32 = 42;
const FIXTURE_TIMESTAMP: f64 = 1_700_000_000.0;
/// The recording time in the metadata of `properties_show_the_metadata`. It
/// differs from `FIXTURE_TIMESTAMP`, so the recording cannot stand in for it.
const METADATA_TIMESTAMP: f64 = 1_600_000_000.0;

/// A recording with the metadata of the React tests: 65.3 seconds, 42 events,
/// 120x40 cells.
fn fixture() -> LoadedRecording {
    LoadedRecording {
        recording: Recording {
            version: 2,
            width: 120,
            height: 40,
            timestamp: FIXTURE_TIMESTAMP,
            duration: 65.3,
            command: "echo hello".into(),
            title: "Demo Recording".into(),
            env: BTreeMap::new(),
            events: (0..EVENT_COUNT)
                .map(|index| Event {
                    time: f64::from(index) / 10.0,
                    event_type: EventType::Output,
                    data: "x".into(),
                })
                .collect(),
        },
        metadata: RecordingMetadata {
            file_path: FIXTURE_PATH.into(),
            title: "Demo Recording".into(),
            command: "echo hello".into(),
            duration: 65.3,
            event_count: EVENT_COUNT as usize,
            width: 120,
            height: 40,
            timestamp: FIXTURE_TIMESTAMP,
        },
    }
}

/// The HTML of one row of the properties panel: the label span and the value
/// span, side by side. `hover` is the `title` attribute of the value span.
fn metadata_row(label: &str, value: &str, hover: Option<&str>) -> String {
    let title = hover.map_or_else(String::new, |text| format!(" title=\"{text}\""));
    format!(
        "<span class=\"metadata-label\">{label}</span>\
         <span class=\"metadata-value\"{title}>{value}</span>"
    )
}

/// What `panels_root` builds its document from: a recording, and the edits
/// that the editor of the document applied.
#[derive(Clone)]
struct PanelsInput {
    recording: LoadedRecording,
    edits: Vec<Edit>,
}

/// Renders `root` to text. The recording goes into the root context.
fn render_panels(recording: LoadedRecording) -> String {
    render_edited_panels(recording, Vec::new())
}

/// Renders the panels of a document whose editor applied `edits`.
fn render_edited_panels(recording: LoadedRecording, edits: Vec<Edit>) -> String {
    let mut dom = VirtualDom::new(panels_root).with_root_context(PanelsInput { recording, edits });
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

/// A test root that shows the three panels of a document from the context.
fn panels_root() -> Element {
    let input = use_context::<PanelsInput>();
    let loaded = use_signal(|| {
        let mut document = Document::open(input.recording, Arc::new(crate::filters::registry()))
            .expect("a document");
        for edit in input.edits {
            document.editor.apply(edit).expect("an edit that applies");
        }
        Some(document)
    });
    let playhead = use_signal(|| Seconds::ZERO);
    let playing = use_signal(|| false);
    let rendered = use_memo(move || preview::render_of(&loaded.read()));
    let view = use_signal(View::default);
    rsx! {
        PreviewPane { rendered, playhead, playing }
        Timeline { loaded, rendered, playhead, view }
        PropertiesPanel { loaded, on_result: |_| {} }
    }
}

/// What `preview_root` takes from the test, and gives back to it.
#[derive(Clone)]
struct PreviewInput {
    /// The recording for the first render. The root takes it out, so the
    /// context holds no strong count.
    recording: Rc<RefCell<Option<Arc<Recording>>>>,
    /// The signal of the root. The root puts it here, so the test can write it.
    signal: Rc<Cell<Option<Signal<RenderedState>>>>,
}

/// A test root that shows the preview pane of the recording in the context.
fn preview_root() -> Element {
    let input = use_context::<PreviewInput>();
    let rendered = use_signal(|| {
        input
            .recording
            .borrow_mut()
            .take()
            .map(|recording| Ok(preview::Rendered(recording)))
    });
    input.signal.set(Some(rendered));
    let playhead = use_signal(|| Seconds::ZERO);
    let playing = use_signal(|| false);
    rsx! {
        PreviewPane { rendered, playhead, playing }
    }
}

#[test]
fn a_failed_render_frees_the_old_preview() {
    let recording = Arc::new(fixture().recording);
    let weak = Arc::downgrade(&recording);
    let input = PreviewInput {
        recording: Rc::new(RefCell::new(Some(recording))),
        signal: Rc::new(Cell::new(None)),
    };
    let mut dom = VirtualDom::new(preview_root).with_root_context(input.clone());
    dom.rebuild_in_place();
    assert!(weak.upgrade().is_some(), "the engine holds the recording");
    let mut signal = input.signal.take().expect("the root stored its signal");
    dom.in_runtime(|| signal.set(Some(Err("boom".to_owned()))));
    dom.render_immediate_to_vec();
    assert_eq!(
        weak.strong_count(),
        0,
        "the pane cannot show the old preview, so it must drop it"
    );
    assert!(dioxus_ssr::render(&dom).contains(preview::PREVIEW_FAILED_PREFIX));
}

/// Renders the `App` with no recording. The fakes are never called.
fn render_empty_app() -> String {
    Window::open(&Fakes::new()).html()
}

/// The most rounds that `settle` waits for.
const MAX_SETTLE_ROUNDS: usize = 50;
/// The longest that one round of `settle` waits for work.
const SETTLE_ROUND_TIMEOUT: Duration = Duration::from_millis(100);
/// The text of the Open Recording button.
const OPEN_LABEL: &str = "Open Recording";
/// The text of the Add Recording button.
const ADD_LABEL: &str = "Add Recording";
/// The text of the Undo button.
const UNDO_TEXT: &str = "Undo";
/// The text of the Redo button.
const REDO_TEXT: &str = "Redo";
/// The markup before the label of the first transport button.
const TRANSPORT_START: &str = "class=\"transport\">";
/// The markup before the text of the time readout.
const READOUT_START: &str = "class=\"preview-time\">";
/// The path that the fake dialog picks.
const PICKED_PATH: &str = "/tmp/demo.json";

type LoadResult = Result<LoadedRecording, NleError>;

/// The answer of a fake: ready now, or later when the test sends it.
enum Answer<T> {
    Ready(T),
    Later(oneshot::Receiver<T>),
}

impl<T> Answer<T> {
    /// Makes an answer that waits for the test, and the sender that ends the wait.
    fn later() -> (oneshot::Sender<T>, Self) {
        let (sender, receiver) = oneshot::channel();
        (sender, Self::Later(receiver))
    }

    /// Waits for the answer.
    async fn resolve(self) -> T {
        match self {
            Self::Ready(value) => value,
            Self::Later(receiver) => receiver.await.expect("the test dropped the sender"),
        }
    }
}

/// What the fakes saw and what they will answer.
#[derive(Default)]
struct Script {
    picks: usize,
    loads: Vec<PathBuf>,
    pick_answers: VecDeque<Answer<Option<PathBuf>>>,
    load_answers: VecDeque<Answer<LoadResult>>,
}

/// A fake dialog and a fake load. Each takes its answer from a queue.
struct Fakes {
    script: Arc<Mutex<Script>>,
}

impl Fakes {
    fn new() -> Self {
        Self {
            script: Arc::default(),
        }
    }

    fn script(&self) -> std::sync::MutexGuard<'_, Script> {
        self.script.lock().expect("the script lock")
    }

    /// Queues the answer of the next dialog.
    fn pick(self, answer: Answer<Option<PathBuf>>) -> Self {
        self.script().pick_answers.push_back(answer);
        self
    }

    /// Queues the answer of the next load.
    fn load(self, answer: Answer<LoadResult>) -> Self {
        self.script().load_answers.push_back(answer);
        self
    }

    fn picks(&self) -> usize {
        self.script().picks
    }

    fn loads(&self) -> Vec<PathBuf> {
        self.script().loads.clone()
    }

    /// The services that read and write this script.
    fn services(&self) -> Services {
        let pick_script = Arc::clone(&self.script);
        let load_script = Arc::clone(&self.script);
        Services::new(
            move || {
                let mut script = pick_script.lock().expect("the script lock");
                script.picks += 1;
                let answer = script
                    .pick_answers
                    .pop_front()
                    .expect("an unexpected dialog");
                drop(script);
                Box::pin(answer.resolve())
            },
            move |path| {
                let mut script = load_script.lock().expect("the script lock");
                script.loads.push(path);
                let answer = script.load_answers.pop_front().expect("an unexpected load");
                drop(script);
                Box::pin(answer.resolve())
            },
        )
    }
}

/// The text of `page` after the first `start` and before the next `end`.
fn text_between(page: &str, start: &str, end: &str) -> String {
    let (_, after) = page
        .split_once(start)
        .unwrap_or_else(|| panic!("no {start} in {page}"));
    let (text, _) = after
        .split_once(end)
        .unwrap_or_else(|| panic!("no {end} after {start} in {page}"));
    text.to_owned()
}

/// A window of the app that a test drives: it renders, it clicks, and it
/// reads the page as text.
struct Window {
    dom: VirtualDom,
    /// The id of each element that got a click listener, in creation order.
    clicks: Vec<ElementId>,
    /// The ids that the newest render created, when it created any.
    newest: Vec<ElementId>,
    /// The click listeners of the first render.
    first_render: Vec<ElementId>,
    /// The id of each element that got an input listener, in creation order.
    inputs: Vec<ElementId>,
    /// The id of each element that got a change listener, in creation order.
    changes: Vec<ElementId>,
    /// The Apply Speed button, when a test noted it.
    apply: Option<ElementId>,
    /// The Apply Redaction button, when a test noted it.
    redact: Option<ElementId>,
    /// The Play and Pause button, when a test noted it.
    play: Option<ElementId>,
    /// The Stop button, when a test noted it.
    stop: Option<ElementId>,
    /// The Zoom in button of the timeline, when a test noted it.
    zoom_in: Option<ElementId>,
}

impl Window {
    /// Builds the app with the services of `fakes` and renders it once.
    fn open(fakes: &Fakes) -> Self {
        static CONVERTER: Once = Once::new();
        CONVERTER.call_once(|| {
            dioxus::html::set_event_converter(Box::new(dioxus::html::SerializedHtmlEventConverter));
        });
        let mut dom = VirtualDom::new(App).with_root_context(fakes.services());
        let edits = dom.rebuild_to_vec().edits;
        let mut window = Self {
            dom,
            clicks: Vec::new(),
            newest: Vec::new(),
            first_render: Vec::new(),
            inputs: Vec::new(),
            changes: Vec::new(),
            apply: None,
            redact: None,
            play: None,
            stop: None,
            zoom_in: None,
        };
        window.note_listeners(&edits);
        window.first_render = window.clicks.clone();
        window
    }

    /// Keeps the id of each new click listener in `edits`.
    fn note_listeners(&mut self, edits: &[Mutation]) {
        let created: Vec<ElementId> = edits
            .iter()
            .filter_map(|edit| match edit {
                Mutation::NewEventListener { name, id } if name == "click" => Some(*id),
                _ => None,
            })
            .collect();
        if !created.is_empty() {
            self.clicks.extend(&created);
            self.newest = created;
        }
        self.inputs
            .extend(edits.iter().filter_map(|edit| match edit {
                Mutation::NewEventListener { name, id } if name == "input" => Some(*id),
                _ => None,
            }));
        self.changes
            .extend(edits.iter().filter_map(|edit| match edit {
                Mutation::NewEventListener { name, id } if name == "change" => Some(*id),
                _ => None,
            }));
    }

    /// Renders what is dirty.
    fn render(&mut self) {
        let edits = self.dom.render_immediate_to_vec().edits;
        self.note_listeners(&edits);
    }

    /// The button of the menu bar at `position`. The first render creates the
    /// click listeners of the menu bar in this order: Open Recording, Add
    /// Recording, Undo, Redo.
    fn menu_button(&self, position: usize, name: &str) -> ElementId {
        self.first_render
            .get(position)
            .copied()
            .unwrap_or_else(|| panic!("no {name} button in {:?}", self.first_render))
    }

    /// The Open Recording button. It is the first click listener of the first render.
    fn open_button(&self) -> ElementId {
        self.menu_button(0, OPEN_LABEL)
    }

    /// The Add Recording button. It is the second click listener of the first render.
    fn add_button(&self) -> ElementId {
        self.menu_button(1, ADD_LABEL)
    }

    /// The Undo button. It is the third click listener of the first render.
    fn undo_button(&self) -> ElementId {
        self.menu_button(2, UNDO_TEXT)
    }

    /// The Redo button. It is the fourth click listener of the first render.
    fn redo_button(&self) -> ElementId {
        self.menu_button(3, REDO_TEXT)
    }

    /// The Dismiss button. It is the one click listener of the render that showed the banner.
    fn dismiss_button(&self) -> ElementId {
        assert!(self.html().contains("role=\"alert\""), "no banner");
        assert_eq!(
            self.newest.len(),
            1,
            "the banner render made {:?}",
            self.newest
        );
        self.newest[0]
    }

    /// Sends a click to the element and renders.
    fn click(&mut self, id: ElementId) {
        let data = Rc::new(PlatformEventData::new(Box::<SerializedMouseData>::default()));
        let event = dioxus::prelude::Event::new(data as Rc<dyn Any>, true);
        self.dom.runtime().handle_event("click", event, id);
        self.render();
    }

    /// Sends the text to the input element, as if the user typed it, and renders.
    fn type_into(&mut self, id: ElementId, text: &str) {
        let form = dioxus::html::SerializedFormData::new(text.to_string(), Vec::new());
        let data = Rc::new(PlatformEventData::new(Box::new(form)));
        let event = dioxus::prelude::Event::new(data as Rc<dyn Any>, true);
        self.dom.runtime().handle_event("input", event, id);
        self.render();
    }

    /// Sends a key down of `press` to the element, as if the user pressed the
    /// key while the element had the focus, and renders. The answer is true
    /// when a handler prevented the default action of the key.
    fn press(&mut self, id: ElementId, press: &Press) -> bool {
        let data = SerializedKeyboardData::new(
            Key::Character(press.text.to_owned()),
            press.code,
            Location::Standard,
            false,
            press.modifiers,
            false,
        );
        let data = Rc::new(PlatformEventData::new(Box::new(data)));
        let event = dioxus::prelude::Event::new(data as Rc<dyn Any>, true);
        self.dom
            .runtime()
            .handle_event("keydown", event.clone(), id);
        self.render();
        !event.default_action_enabled()
    }

    /// Sends the value to the select element, as if the user chose it, and renders.
    fn choose(&mut self, id: ElementId, value: &str) {
        let form = dioxus::html::SerializedFormData::new(value.to_string(), Vec::new());
        let data = Rc::new(PlatformEventData::new(Box::new(form)));
        let event = dioxus::prelude::Event::new(data as Rc<dyn Any>, true);
        self.dom.runtime().handle_event("change", event, id);
        self.render();
    }

    /// The theme menu of the preview toolbar. It is the one change listener of
    /// the open render.
    fn theme_menu(&self) -> ElementId {
        assert_eq!(self.changes.len(), 1, "{:?}", self.changes);
        self.changes[0]
    }

    /// The input listeners that the open render created, in creation order:
    /// the Start, End, and Speed fields of the speed panel, then the Find,
    /// Pattern, Replace with, Replacement text, and Events fields of the
    /// redaction panel, then the playhead control of the timeline. The
    /// properties panel mounts before the timeline, so the playhead is the
    /// last one.
    fn open_inputs(&self) -> [ElementId; 9] {
        self.inputs
            .clone()
            .try_into()
            .unwrap_or_else(|ids: Vec<ElementId>| panic!("the page has the inputs {ids:?}"))
    }

    /// The Start, End, and Speed fields, in this order.
    fn speed_fields(&self) -> [ElementId; 3] {
        let [start, end, speed, ..] = self.open_inputs();
        [start, end, speed]
    }

    /// The Find, Pattern, Replace with, Replacement text, and Events fields,
    /// in this order.
    fn redaction_fields(&self) -> [ElementId; 5] {
        let [_, _, _, find, pattern, replace, replacement, events, _] = self.open_inputs();
        [find, pattern, replace, replacement, events]
    }

    /// The playhead control of the timeline.
    fn playhead_control(&self) -> ElementId {
        let [.., playhead] = self.open_inputs();
        playhead
    }

    /// The Apply Redaction button. `open_recording` noted it.
    fn redact_button(&self) -> ElementId {
        self.redact.expect("a window with an open recording")
    }

    /// Fills the five fields of the redaction panel and clicks Apply Redaction.
    fn apply_redaction(
        &mut self,
        find: &str,
        pattern: &str,
        replace: &str,
        replacement: &str,
        events: &str,
    ) {
        let fields = self.redaction_fields();
        for (field, text) in fields
            .into_iter()
            .zip([find, pattern, replace, replacement, events])
        {
            self.type_into(field, text);
        }
        self.click(self.redact_button());
    }

    /// Notes the seven click listeners that the render of an open recording
    /// created. The render creates them in this order: Apply Speed, Apply
    /// Redaction, the Zoom out, Zoom in, and Fit buttons of the timeline,
    /// Play, Stop. The properties panel mounts before the timeline, and the
    /// timeline before the preview toolbar. The speed panel comes before the
    /// redaction panel, and the Play button comes before the Stop button.
    fn note_open_buttons(&mut self) {
        let [apply, redact, _, zoom_in, _, play, stop] = self.newest[..] else {
            panic!("the open render made {:?}", self.newest)
        };
        self.apply = Some(apply);
        self.redact = Some(redact);
        self.zoom_in = Some(zoom_in);
        self.play = Some(play);
        self.stop = Some(stop);
    }

    /// The Apply Speed button. `open_recording` noted it.
    fn apply_button(&self) -> ElementId {
        self.apply.expect("a window with an open recording")
    }

    /// The Play and Pause button. `open_recording` noted it.
    fn play_button(&self) -> ElementId {
        self.play.expect("a window with an open recording")
    }

    /// The Stop button. `open_recording` noted it.
    fn stop_button(&self) -> ElementId {
        self.stop.expect("a window with an open recording")
    }

    /// The Zoom in button of the timeline. `open_recording` noted it.
    fn zoom_in_button(&self) -> ElementId {
        self.zoom_in.expect("a window with an open recording")
    }

    /// The style of the first clip block of the timeline: its place and its
    /// width, which show the zoom and the scroll.
    fn clip_style(&self) -> String {
        text_between(&self.html(), "class=\"timeline-clip\" style=\"", "\"")
    }

    /// The label of the Play and Pause button, read from the page.
    fn play_label(&self) -> String {
        text_between(&self.html(), TRANSPORT_START, "</button>")
    }

    /// The time readout of the preview toolbar, read from the page.
    fn readout(&self) -> String {
        text_between(&self.html(), READOUT_START, "</span>")
    }

    /// The Remove button of the row that the newest render added.
    fn remove_button(&self) -> ElementId {
        assert_eq!(self.newest.len(), 1, "the render made {:?}", self.newest);
        self.newest[0]
    }

    /// Fills the three fields and clicks Apply Speed.
    fn apply_speed(&mut self, start: &str, end: &str, speed: &str) {
        let [start_field, end_field, speed_field] = self.speed_fields();
        self.type_into(start_field, start);
        self.type_into(end_field, end);
        self.type_into(speed_field, speed);
        self.click(self.apply_button());
    }

    /// Runs the tasks of the app until they have no more work, and renders
    /// after each round. Each wait has a bound.
    async fn settle(&mut self) {
        for _ in 0..MAX_SETTLE_ROUNDS {
            if tokio::time::timeout(SETTLE_ROUND_TIMEOUT, self.dom.wait_for_work())
                .await
                .is_err()
            {
                return;
            }
            self.render();
        }
    }

    /// The page as text.
    fn html(&self) -> String {
        dioxus_ssr::render(&self.dom)
    }

    /// Whether the start tag of the Open Recording button holds `disabled`.
    fn open_is_disabled(&self) -> bool {
        self.button_is_disabled(OPEN_LABEL)
    }

    /// Whether the start tag of the button with the text `label` holds `disabled`.
    fn button_is_disabled(&self, label: &str) -> bool {
        let page = self.html();
        let (before, _) = page
            .split_once(&format!(">{label}</button>"))
            .unwrap_or_else(|| panic!("no {label} button in {page}"));
        let (_, tag) = before.rsplit_once("<button").expect("a button start tag");
        tag.split([' ', '>', '=']).any(|word| word == "disabled")
    }

    /// Whether the Undo button and the Redo button are disabled, in this order.
    fn history_disabled(&self) -> [bool; 2] {
        [UNDO_TEXT, REDO_TEXT].map(|label| self.button_is_disabled(label))
    }

    /// Whether the start tag of the input after the text `label` holds `disabled`.
    fn field_is_disabled(&self, label: &str) -> bool {
        let page = self.html();
        let (_, after) = page
            .split_once(&format!(">{label}<input"))
            .unwrap_or_else(|| panic!("no field {label} in {page}"));
        let (tag, _) = after.split_once('>').expect("an input start tag");
        tag.split([' ', '/', '=']).any(|word| word == "disabled")
    }
}

#[test]
fn app_with_no_recording_shows_the_empty_regions() {
    let page = render_empty_app();
    assert!(page.contains(OPEN_LABEL), "{page}");
    assert!(
        page.contains("Open a recording to preview it here"),
        "{page}"
    );
    assert!(page.contains("Properties"), "{page}");
    assert_eq!(page.matches("No recording loaded").count(), 2, "{page}");
    assert!(!page.contains("role=\"alert\""), "{page}");
}

#[test]
fn app_renders_the_style_sheet_first() {
    let page = render_empty_app();
    assert!(page.contains(".app-layout"), "{page}");
    assert!(
        page.find("<style")
            .is_some_and(|at| at < page.find("menu-bar\"").unwrap_or(0)),
        "{page}"
    );
}

#[test]
fn properties_show_the_metadata() {
    let loaded = fixture();
    let loaded = LoadedRecording {
        metadata: RecordingMetadata {
            file_path: FIXTURE_PATH.into(),
            title: "Metadata Title".into(),
            command: "ls -la".into(),
            duration: 125.5,
            event_count: 7,
            width: 80,
            height: 24,
            timestamp: METADATA_TIMESTAMP,
        },
        ..loaded
    };
    let page = render_panels(loaded);
    let recorded = format_recorded(METADATA_TIMESTAMP);
    let rows = [
        metadata_row("Title", "Metadata Title", None),
        metadata_row("Command", "ls -la", None),
        metadata_row("Duration", "2m 5.5s", None),
        metadata_row("Events", "7", None),
        metadata_row("Size", "80x24", None),
        metadata_row("Recorded", &recorded, None),
        metadata_row("File", "demo.json", Some(FIXTURE_PATH)),
    ];
    for row in rows {
        assert!(page.contains(&row), "{row} missing in {page}");
    }
}

#[test]
fn properties_show_untitled_for_an_empty_title() {
    let mut recording = fixture();
    recording.metadata.title = String::new();
    let page = render_panels(recording);
    assert!(page.contains("(untitled)"), "{page}");
}

#[test]
fn properties_show_the_file_name_of_a_windows_path() {
    let mut recording = fixture();
    recording.metadata.file_path = r"C:\Users\test\recordings\demo.json".into();
    let page = render_panels(recording);
    assert!(page.contains(">demo.json<"), "{page}");
}

#[test]
fn timeline_shows_the_event_count_and_the_duration() {
    let page = render_panels(fixture());
    assert!(page.contains("42 events"), "{page}");
    assert!(page.contains("class=\"timeline-clip\""), "{page}");
    assert!(!page.contains("coming in #150"), "{page}");
}

#[test]
fn timeline_rounds_the_duration_to_tenths() {
    for (seconds, expected) in [(125.5, "2m 5.5s"), (59.96, "1m 0.0s")] {
        let mut recording = fixture();
        recording.recording.duration = seconds;
        let page = render_panels(recording);
        assert!(
            page.contains(&format!("| {expected}")),
            "{expected} missing in {page}"
        );
    }
}

#[test]
fn timeline_reads_the_project_of_the_editor() {
    let trim = Edit::TrimClip {
        id: ClipId(1),
        in_point: Seconds::new(1.0).expect("a time"),
        out_point: Seconds::new(2.0).expect("a time"),
    };
    let page = render_edited_panels(fixture(), vec![trim]);
    assert!(page.contains("10 events | 0m 1.0s"), "{page}");
}

#[test]
fn preview_draws_the_terminal_at_time_zero() {
    let page = render_panels(fixture());
    assert!(page.contains("--columns:120;--rows:40"), "{page}");
    assert!(page.contains(">x</span>"), "{page}");
    assert!(page.contains("terminal-row"), "{page}");
    assert!(!page.contains("coming in #151"), "{page}");
}

fn picked() -> Answer<Option<PathBuf>> {
    Answer::Ready(Some(PathBuf::from(PICKED_PATH)))
}

/// The message of the banner for a file that does not exist.
const NOT_FOUND_BANNER: &str = "Failed to open recording: File not found: /tmp/demo.json";

fn not_found() -> Answer<LoadResult> {
    Answer::Ready(Err(NleError::FileNotFound(PICKED_PATH.into())))
}

#[tokio::test]
async fn app_with_no_recording_has_an_enabled_open_button_and_no_dialog() {
    let fakes = Fakes::new();
    let window = Window::open(&fakes);
    assert!(!window.open_is_disabled(), "{}", window.html());
    // Open Recording, Add Recording, Undo, and Redo.
    assert_eq!(window.first_render.len(), 4);
    assert_eq!((fakes.picks(), fakes.loads().len()), (0, 0));
}

#[tokio::test]
async fn a_picked_recording_loads_and_fills_the_panels() {
    let fakes = Fakes::new()
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())));
    let mut window = Window::open(&fakes);
    window.click(window.open_button());
    window.settle().await;
    let page = window.html();
    assert_eq!(fakes.loads(), vec![PathBuf::from(PICKED_PATH)]);
    assert!(page.contains("Demo Recording"), "{page}");
    assert!(page.contains("42 events | 1m 5.3s"), "{page}");
    assert!(!window.open_is_disabled(), "{page}");
    assert!(!page.contains("role=\"alert\""), "{page}");
}

#[tokio::test]
async fn a_cancelled_dialog_loads_nothing() {
    let fakes = Fakes::new().pick(Answer::Ready(None));
    let mut window = Window::open(&fakes);
    window.click(window.open_button());
    window.settle().await;
    let page = window.html();
    assert_eq!((fakes.picks(), fakes.loads().len()), (1, 0));
    assert!(!window.open_is_disabled(), "{page}");
    assert!(!page.contains("role=\"alert\""), "{page}");
    assert_eq!(page.matches("No recording loaded").count(), 2, "{page}");
}

#[tokio::test]
async fn a_failed_load_shows_the_banner_until_it_is_dismissed() {
    let fakes = Fakes::new().pick(picked()).load(not_found());
    let mut window = Window::open(&fakes);
    window.click(window.open_button());
    window.settle().await;
    let page = window.html();
    assert!(page.contains("role=\"alert\""), "{page}");
    assert!(page.contains(NOT_FOUND_BANNER), "{page}");
    assert!(!window.open_is_disabled(), "{page}");
    window.click(window.dismiss_button());
    window.settle().await;
    assert!(
        !window.html().contains("role=\"alert\""),
        "{}",
        window.html()
    );
}

#[tokio::test]
async fn the_button_stays_disabled_while_the_dialog_is_open() {
    let (answer, pending) = Answer::later();
    let fakes = Fakes::new().pick(pending);
    let mut window = Window::open(&fakes);
    let button = window.open_button();
    window.click(button);
    window.settle().await;
    assert!(window.open_is_disabled(), "{}", window.html());
    window.click(button);
    window.settle().await;
    assert_eq!(fakes.picks(), 1);
    answer.send(None).expect("the dialog future waits");
    window.settle().await;
    assert!(!window.open_is_disabled(), "{}", window.html());
    assert_eq!((fakes.picks(), fakes.loads().len()), (1, 0));
}

#[tokio::test]
async fn the_button_stays_disabled_while_the_recording_loads() {
    let (answer, pending) = Answer::later();
    let fakes = Fakes::new().pick(picked()).load(pending);
    let mut window = Window::open(&fakes);
    let button = window.open_button();
    window.click(button);
    window.settle().await;
    assert!(window.open_is_disabled(), "{}", window.html());
    assert_eq!((fakes.picks(), fakes.loads().len()), (1, 1));
    window.click(button);
    window.settle().await;
    assert_eq!((fakes.picks(), fakes.loads().len()), (1, 1));
    answer.send(Ok(fixture())).expect("the load future waits");
    window.settle().await;
    let page = window.html();
    assert!(!window.open_is_disabled(), "{page}");
    assert!(page.contains("Demo Recording"), "{page}");
}

#[tokio::test]
async fn a_failed_load_keeps_the_open_recording() {
    let fakes = Fakes::new()
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())))
        .pick(picked())
        .load(not_found());
    let mut window = Window::open(&fakes);
    for _ in 0..2 {
        window.click(window.open_button());
        window.settle().await;
    }
    let page = window.html();
    assert!(page.contains(NOT_FOUND_BANNER), "{page}");
    assert!(page.contains("Demo Recording"), "{page}");
    assert!(page.contains("42 events | 1m 5.3s"), "{page}");
}

#[tokio::test]
async fn a_recording_that_cannot_become_a_project_shows_the_banner() {
    let mut loaded = fixture();
    loaded.recording.duration = -1.0;
    let fakes = Fakes::new().pick(picked()).load(Answer::Ready(Ok(loaded)));
    let mut window = Window::open(&fakes);
    window.click(window.open_button());
    window.settle().await;
    let page = window.html();
    assert!(
        page.contains(
            "Failed to open recording: Invalid time: -1 (a time must be finite and not below zero)"
        ),
        "{page}"
    );
    assert_eq!(page.matches("No recording loaded").count(), 2, "{page}");
}

#[tokio::test]
async fn a_load_after_an_error_removes_the_banner() {
    let fakes = Fakes::new()
        .pick(picked())
        .load(not_found())
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())));
    let mut window = Window::open(&fakes);
    window.click(window.open_button());
    window.settle().await;
    assert!(
        window.html().contains("role=\"alert\""),
        "{}",
        window.html()
    );
    window.click(window.open_button());
    window.settle().await;
    let page = window.html();
    assert!(!page.contains("role=\"alert\""), "{page}");
    assert!(page.contains("Demo Recording"), "{page}");
}

// Tests of the Add Recording button.

/// The banner for an added recording whose file does not exist.
const ADD_NOT_FOUND_BANNER: &str = "Failed to add recording: File not found: /tmp/demo.json";

/// A second take of the fixture: 10 events in 5 seconds, with its own title.
fn second_take() -> LoadedRecording {
    let mut loaded = fixture();
    loaded.recording.title = "Second Take".into();
    loaded.recording.duration = 5.0;
    loaded.recording.events.truncate(10);
    loaded.metadata.title = "Second Take".into();
    loaded.metadata.duration = 5.0;
    loaded.metadata.event_count = 10;
    loaded
}

/// Opens the fixture through `fakes`, whose first dialog and first load open
/// it. The other answers of `fakes` stay for the test.
async fn open_with(fakes: &Fakes) -> Window {
    let mut window = Window::open(fakes);
    window.click(window.open_button());
    window.settle().await;
    window
}

/// The fakes that open the fixture, and then answer one Add Recording with
/// `added`.
fn open_then_add(added: Answer<LoadResult>) -> Fakes {
    Fakes::new()
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())))
        .pick(picked())
        .load(added)
}

/// The titles of the clip blocks of the timeline, in the order of the page.
fn clip_titles(page: &str) -> Vec<String> {
    page.split("class=\"timeline-clip\"")
        .skip(1)
        .map(|block| text_between(block, "title=\"", "\""))
        .collect()
}

#[tokio::test]
async fn add_recording_is_disabled_until_a_recording_is_open() {
    let fakes = open_then_add(Answer::Ready(Ok(second_take())));
    let window = Window::open(&fakes);
    assert!(window.button_is_disabled(ADD_LABEL), "{}", window.html());

    let window = open_with(&fakes).await;

    assert!(!window.button_is_disabled(ADD_LABEL), "{}", window.html());
}

#[tokio::test]
async fn an_added_recording_plays_after_the_end_of_the_timeline() {
    let fakes = open_then_add(Answer::Ready(Ok(second_take())));
    let mut window = open_with(&fakes).await;

    window.click(window.add_button());
    window.settle().await;

    let page = window.html();
    assert_eq!(fakes.picks(), 2);
    assert_eq!(fakes.loads(), vec![PathBuf::from(PICKED_PATH); 2]);
    assert!(!page.contains("role=\"alert\""), "{page}");
    // The timeline holds both clips, one after the other.
    assert_eq!(
        clip_titles(&page),
        vec![
            "Demo Recording: 0s to 1m 5.3s of the recording",
            "Second Take: 0s to 5.0s of the recording",
        ]
    );
    assert!(page.contains("52 events | 1m 10.3s"), "{page}");
    // The rendered recording ends at the end of the second clip.
    assert!(page.contains(&edited_row("1m 10.3s")), "{page}");
    // The properties still show the file that the project came from.
    assert!(page.contains(&metadata_row("Title", "Demo Recording", None)), "{page}");
}

#[tokio::test]
async fn one_undo_removes_an_added_recording() {
    let fakes = open_then_add(Answer::Ready(Ok(second_take())));
    let mut window = open_with(&fakes).await;
    window.click(window.add_button());
    window.settle().await;

    window.click(window.undo_button());
    window.settle().await;

    let page = window.html();
    assert_eq!(clip_titles(&page).len(), 1, "{page}");
    assert!(page.contains("42 events | 1m 5.3s"), "{page}");
}

#[tokio::test]
async fn a_failed_add_shows_the_banner_and_keeps_the_project() {
    let fakes = open_then_add(not_found());
    let mut window = open_with(&fakes).await;

    window.click(window.add_button());
    window.settle().await;

    let page = window.html();
    assert!(page.contains(ADD_NOT_FOUND_BANNER), "{page}");
    assert_eq!(clip_titles(&page).len(), 1, "{page}");
    assert!(page.contains("42 events | 1m 5.3s"), "{page}");
    assert!(!window.button_is_disabled(ADD_LABEL), "{page}");
}

#[tokio::test]
async fn a_recording_that_cannot_become_a_clip_shows_the_banner() {
    let mut broken = second_take();
    broken.recording.duration = -1.0;
    let fakes = open_then_add(Answer::Ready(Ok(broken)));
    let mut window = open_with(&fakes).await;

    window.click(window.add_button());
    window.settle().await;

    let page = window.html();
    assert!(
        page.contains(
            "Failed to add recording: Invalid time: -1 (a time must be finite and not below zero)"
        ),
        "{page}"
    );
    assert_eq!(clip_titles(&page).len(), 1, "{page}");
}

#[tokio::test]
async fn both_buttons_stay_disabled_while_the_add_dialog_is_open() {
    let (answer, pending) = Answer::later();
    let fakes = Fakes::new()
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())))
        .pick(pending);
    let mut window = open_with(&fakes).await;
    window.click(window.add_button());
    window.settle().await;
    assert!(window.open_is_disabled(), "{}", window.html());
    assert!(window.button_is_disabled(ADD_LABEL), "{}", window.html());
    window.click(window.add_button());
    window.click(window.open_button());
    window.settle().await;
    assert_eq!(fakes.picks(), 2);

    answer.send(None).expect("the dialog future waits");
    window.settle().await;

    assert!(!window.open_is_disabled(), "{}", window.html());
    assert!(!window.button_is_disabled(ADD_LABEL), "{}", window.html());
    assert_eq!(clip_titles(&window.html()).len(), 1);
}

/// The row text of a speed change as the page shows it.
fn speed_row(region: &str, speed: &str) -> String {
    format!("<span>{region} at {speed}\u{d7}</span>")
}

/// The row of the page that shows the duration after the edits.
fn edited_row(text: &str) -> String {
    metadata_row("Edited duration", text, None)
}

/// Opens the fixture recording through the fakes, and notes the four click
/// listeners that the open render creates (`Window::note_open_buttons`).
async fn open_recording() -> Window {
    open_loaded(fixture()).await
}

/// Opens `loaded` through the fakes, and notes the four click listeners as
/// `open_recording` does.
async fn open_loaded(loaded: LoadedRecording) -> Window {
    let fakes = Fakes::new().pick(picked()).load(Answer::Ready(Ok(loaded)));
    let mut window = Window::open(&fakes);
    window.click(window.open_button());
    window.settle().await;
    window.note_open_buttons();
    window
}

/// The row text of a redaction as the page shows it.
fn redaction_row(text: &str) -> String {
    format!("<span>{text}</span>")
}

#[tokio::test]
async fn the_speed_panel_shows_its_fields_and_the_edited_duration() {
    let window = open_recording().await;
    let page = window.html();
    for text in [
        "Speed",
        "Times are in seconds, on the timeline after the speed changes above.",
        "Start (s)",
        "End (s)",
        "Apply Speed",
    ] {
        assert!(page.contains(text), "{text} missing in {page}");
    }
    assert!(page.contains(&edited_row("1m 5.3s")), "{page}");
    assert_eq!(window.speed_fields().len(), 3);
    assert!(!page.contains("role=\"alert\""), "{page}");
}

#[tokio::test]
async fn the_redaction_panel_shows_its_fields_and_its_button() {
    let window = open_recording().await;
    let page = window.html();
    for text in [
        "Redaction",
        "Each redaction covers the whole timeline. Escape sequences do not change.",
        "Find",
        "Pattern",
        "Replace with",
        "Replacement text",
        "Events",
        "Apply Redaction",
    ] {
        assert!(page.contains(text), "{text} missing in {page}");
    }
    assert_eq!(window.redaction_fields().len(), 5);
    assert!(!page.contains("role=\"alert\""), "{page}");
}

#[tokio::test]
async fn applying_a_speed_adds_a_row_and_shortens_the_recording() {
    let mut window = open_recording().await;
    window.apply_speed("10", "20", "2");
    let page = window.html();
    assert!(page.contains(&speed_row("10 s to 20 s", "2")), "{page}");
    assert!(page.contains(&edited_row("1m 0.3s")), "{page}");
    assert!(!page.contains("role=\"alert\""), "{page}");
}

#[tokio::test]
async fn removing_a_speed_row_restores_the_duration() {
    let mut window = open_recording().await;
    window.apply_speed("10", "20", "2");
    assert!(window.html().contains(" at 2\u{d7}"), "{}", window.html());
    window.click(window.remove_button());
    let page = window.html();
    assert!(!page.contains(" at 2\u{d7}"), "{page}");
    assert!(page.contains(&edited_row("1m 5.3s")), "{page}");
}

#[tokio::test]
async fn two_regions_keep_their_own_speeds() {
    let mut window = open_recording().await;
    window.apply_speed("10", "20", "2");
    window.apply_speed("30", "40", "0.5");
    let page = window.html();
    let first = page
        .find(&speed_row("10 s to 20 s", "2"))
        .unwrap_or_else(|| panic!("no first row in {page}"));
    let second = page
        .find(&speed_row("30 s to 40 s", "0.5"))
        .unwrap_or_else(|| panic!("no second row in {page}"));
    assert!(first < second, "{page}");
    assert!(page.contains(&edited_row("1m 10.3s")), "{page}");
}

#[tokio::test]
async fn a_row_shows_the_times_of_its_region_as_typed() {
    let mut window = open_recording().await;
    window.apply_speed("0.01", "0.04", "2");
    window.apply_speed("1.04", "2.04", "2");
    let page = window.html();
    assert!(page.contains(&speed_row("0.01 s to 0.04 s", "2")), "{page}");
    assert!(page.contains(&speed_row("1.04 s to 2.04 s", "2")), "{page}");
}

#[tokio::test]
async fn a_bad_speed_change_shows_the_reason_and_adds_no_row() {
    let cases = [
        (("abc", "20", "2"), "the start is not a number"),
        (("10", "xyz", "2"), "the end is not a number"),
        (("10", "20", ""), "the speed is not a number"),
        (("10", "5", "2"), "the end must come after the start"),
        (("10", "10", "2"), "the end must come after the start"),
        (("10", "20", "0"), "Invalid speed: 0"),
        (("-1", "20", "2"), "Invalid time: -1"),
    ];
    for ((start, end, speed), reason) in cases {
        let mut window = open_recording().await;
        window.apply_speed(start, end, speed);
        let page = window.html();
        let banner = format!("Failed to change the speed: {reason}");
        assert!(page.contains(&banner), "{banner} missing in {page}");
        assert!(!page.contains(" at "), "{page}");
        assert!(page.contains(&edited_row("1m 5.3s")), "{page}");
    }
}

#[tokio::test]
async fn a_failed_speed_change_keeps_the_fields_and_a_later_one_works() {
    let mut window = open_recording().await;
    window.apply_speed("10", "5", "2");
    assert!(window.html().contains("role=\"alert\""));
    let [_, end_field, _] = window.speed_fields();
    window.type_into(end_field, "20");
    window.click(window.apply_button());
    let page = window.html();
    assert!(page.contains(&speed_row("10 s to 20 s", "2")), "{page}");
    assert!(!page.contains("role=\"alert\""), "{page}");
}

#[tokio::test]
async fn a_remove_after_a_failed_speed_change_removes_the_banner() {
    let mut window = open_recording().await;
    window.apply_speed("10", "20", "2");
    let remove = window.remove_button();
    window.apply_speed("10", "5", "2");
    assert!(
        window.html().contains("role=\"alert\""),
        "{}",
        window.html()
    );
    window.click(remove);
    let page = window.html();
    assert!(!page.contains(" at 2\u{d7}"), "{page}");
    assert!(!page.contains("role=\"alert\""), "{page}");
}

#[tokio::test]
async fn a_speed_change_after_which_the_render_fails_is_refused() {
    let mut window = open_recording().await;
    window.apply_speed("0", "10", "1e-320");
    let page = window.html();
    assert!(page.contains("Failed to change the speed: "), "{page}");
    assert!(page.contains("answered the length inf s"), "{page}");
    assert!(!page.contains(" at "), "{page}");
    assert!(page.contains(&edited_row("1m 5.3s")), "{page}");
}

#[tokio::test]
async fn a_remove_after_which_the_render_fails_is_refused() {
    let mut window = open_recording().await;
    window.apply_speed("0", "100", "1e300");
    let first_remove = window.remove_button();
    window.apply_speed("0", "10", "1e-308");
    let page = window.html();
    assert!(!page.contains("role=\"alert\""), "{page}");
    window.click(first_remove);
    let page = window.html();
    assert!(page.contains("role=\"alert\""), "{page}");
    assert!(page.contains("answered the length inf s"), "{page}");
    assert!(
        page.contains(&speed_row("0 s to 100 s", &1e300_f64.to_string())),
        "{page}"
    );
    assert!(
        page.contains(&speed_row("0 s to 10 s", &1e-308_f64.to_string())),
        "{page}"
    );
}

#[test]
fn a_whole_timeline_filter_shows_a_row_and_the_edited_duration() {
    use crate::filters::time_dilation::{self, Speed};
    use crate::types::FilterRegion;
    let filter = time_dilation::applied(FilterRegion::All, Speed::new(2.0).expect("a speed"));
    let page = render_edited_panels(fixture(), vec![Edit::AddFilter { index: 0, filter }]);
    assert!(
        page.contains(&speed_row("the whole timeline", "2")),
        "{page}"
    );
    let expected = crate::format::format_duration(65.3 / 2.0);
    assert!(page.contains(&edited_row(&expected)), "{page}");
}

// The renders that an edit and its edited duration cost.

/// The kind of the filter of `counting_filters`.
const COUNTED_KIND: &str = "counted";

/// A registry with one filter that keeps each event, and the count of the
/// renders that ran that filter.
fn counting_filters() -> (Arc<FilterRegistry>, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let renders = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&renders);
    let mut registry = FilterRegistry::new();
    registry.register(
        crate::types::FilterKind::new(COUNTED_KIND),
        move |events, _| {
            counter.fetch_add(1, Ordering::SeqCst);
            events.to_vec()
        },
    );
    (Arc::new(registry), renders)
}

#[test]
fn an_edit_that_renders_and_its_edited_duration_cost_one_render() {
    let (filters, renders) = counting_filters();
    let mut document = Document::open(fixture(), filters).expect("a document");
    let filter = AppliedFilter {
        kind: crate::types::FilterKind::new(COUNTED_KIND),
        region: FilterRegion::All,
        options: crate::types::FilterOptions::new(),
    };

    document
        .editor
        .apply_renderable(Edit::AddFilter { index: 0, filter })
        .expect("an edit that renders");
    let edited = edited_duration(&Some(document));

    assert_eq!(edited, "1m 5.3s");
    assert_eq!(renders.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// The document of the fixture, with the filters of the app.
fn app_document() -> Document {
    Document::open(fixture(), Arc::new(crate::filters::registry())).expect("a document")
}

/// An edit that doubles the speed of the whole timeline.
fn double_speed() -> Edit {
    let filter = time_dilation::applied(FilterRegion::All, Speed::new(2.0).expect("a speed"));
    Edit::AddFilter { index: 0, filter }
}

#[test]
fn an_undo_after_a_speed_change_shows_the_duration_before_it() {
    let mut document = app_document();
    document
        .editor
        .apply_renderable(double_speed())
        .expect("an edit that renders");

    assert!(document.editor.undo());

    assert_eq!(edited_duration(&Some(document)), "1m 5.3s");
}

#[test]
fn an_edit_after_a_speed_change_shows_the_duration_after_it() {
    let mut document = app_document();
    document
        .editor
        .apply_renderable(double_speed())
        .expect("an edit that renders");

    document
        .editor
        .apply(double_speed())
        .expect("an edit that applies");

    assert_eq!(
        edited_duration(&Some(document)),
        format_duration(65.3 / 4.0)
    );
}

// The redaction panel.

#[tokio::test]
async fn applying_a_text_redaction_adds_a_row_and_keeps_the_duration() {
    let mut window = open_recording().await;
    window.apply_redaction("text", "hunter2", "asterisks", "", "output");
    let page = window.html();
    let row = redaction_row("text \u{201c}hunter2\u{201d} \u{2192} asterisks (output)");
    assert!(page.contains(&row), "{row} missing in {page}");
    assert!(page.contains(&edited_row("1m 5.3s")), "{page}");
    assert!(!page.contains("role=\"alert\""), "{page}");
}

#[tokio::test]
async fn a_preset_redaction_shows_the_label_of_the_preset() {
    let mut window = open_recording().await;
    window.apply_redaction("email", "ignored", "asterisks", "", "output");
    let page = window.html();
    let row = redaction_row("Email addresses \u{2192} asterisks (output)");
    assert!(page.contains(&row), "{row} missing in {page}");
    assert!(!page.contains("role=\"alert\""), "{page}");
}

#[tokio::test]
async fn every_preset_of_the_menu_applies() {
    for preset in crate::filters::redaction::Preset::ALL {
        let mut window = open_recording().await;
        window.apply_redaction(preset.key(), "", "blocks", "", "output");
        let page = window.html();
        let row = redaction_row(&format!("{} \u{2192} blocks (output)", preset.label()));
        assert!(page.contains(&row), "{row} missing in {page}");
    }
}

#[tokio::test]
async fn a_text_replacement_and_both_kinds_of_events_show_in_the_row() {
    let mut window = open_recording().await;
    window.apply_redaction("email", "", "text", "[REDACTED]", "output-and-input");
    let page = window.html();
    let row =
        redaction_row("Email addresses \u{2192} \u{201c}[REDACTED]\u{201d} (output and input)");
    assert!(page.contains(&row), "{row} missing in {page}");
}

#[tokio::test]
async fn a_regex_redaction_shows_its_pattern() {
    let mut window = open_recording().await;
    window.apply_redaction("regex", "a+b", "blocks", "", "output");
    let page = window.html();
    let row = redaction_row("regex /a+b/ \u{2192} blocks (output)");
    assert!(page.contains(&row), "{row} missing in {page}");
}

#[tokio::test]
async fn a_pattern_keeps_its_spaces() {
    let mut window = open_recording().await;
    window.apply_redaction("text", " a ", "text", " x ", "output");
    let page = window.html();
    let row = redaction_row("text \u{201c} a \u{201d} \u{2192} \u{201c} x \u{201d} (output)");
    assert!(page.contains(&row), "{row} missing in {page}");
}

#[tokio::test]
async fn a_bad_redaction_shows_the_reason_and_adds_no_row() {
    let cases = [
        (
            ("text", "", "asterisks", ""),
            "Invalid redaction: the text to find is empty",
        ),
        (
            ("regex", "(", "asterisks", ""),
            "Invalid redaction: the regular expression does not compile",
        ),
        (
            ("text", "a", "text", "a\tb"),
            "Invalid redaction: the replacement holds a control character",
        ),
    ];
    for ((find, pattern, replace, replacement), reason) in cases {
        let mut window = open_recording().await;
        window.apply_redaction(find, pattern, replace, replacement, "output");
        let page = window.html();
        let banner = format!("Failed to redact: {reason}");
        assert!(page.contains(&banner), "{banner} missing in {page}");
        assert!(!page.contains("class=\"redaction-row\""), "{page}");
    }
}

#[tokio::test]
async fn a_good_redaction_after_a_bad_one_removes_the_banner() {
    let mut window = open_recording().await;
    window.apply_redaction("text", "", "asterisks", "", "output");
    assert!(window.html().contains("role=\"alert\""));
    let [_, pattern_field, ..] = window.redaction_fields();
    window.type_into(pattern_field, "hunter2");
    window.click(window.redact_button());
    let page = window.html();
    let row = redaction_row("text \u{201c}hunter2\u{201d} \u{2192} asterisks (output)");
    assert!(page.contains(&row), "{row} missing in {page}");
    assert!(!page.contains("role=\"alert\""), "{page}");
}

#[tokio::test]
async fn a_field_that_the_menus_ignore_is_disabled() {
    let mut window = open_recording().await;
    let [find, _, replace, ..] = window.redaction_fields();
    let disabled = |window: &Window| {
        [PATTERN_LABEL, REPLACEMENT_LABEL].map(|label| window.field_is_disabled(label))
    };
    assert_eq!(disabled(&window), [false, true], "{}", window.html());
    window.type_into(find, "email");
    window.type_into(replace, REPLACE_TEXT);
    assert_eq!(disabled(&window), [true, false], "{}", window.html());
}

#[tokio::test]
async fn removing_a_redaction_row_removes_it() {
    let mut window = open_recording().await;
    window.apply_redaction("text", "hunter2", "asterisks", "", "output");
    window.click(window.remove_button());
    let page = window.html();
    assert!(!page.contains("class=\"redaction-row\""), "{page}");
    assert!(!page.contains("\u{2192} asterisks"), "{page}");
    assert!(page.contains(&edited_row("1m 5.3s")), "{page}");
}

#[tokio::test]
async fn each_remove_button_keeps_the_rows_of_the_other_panel() {
    let speed = speed_row("10 s to 20 s", "2");
    let redaction = redaction_row("text \u{201c}hunter2\u{201d} \u{2192} asterisks (output)");

    // The speed filter comes first, and the redaction row is removed.
    let mut window = open_recording().await;
    window.apply_speed("10", "20", "2");
    window.apply_redaction("text", "hunter2", "asterisks", "", "output");
    window.click(window.remove_button());
    let page = window.html();
    assert!(page.contains(&speed), "{page}");
    assert!(!page.contains(&redaction), "{page}");
    assert!(page.contains(&edited_row("1m 0.3s")), "{page}");

    // The redaction filter comes first, and the speed row is removed.
    let mut window = open_recording().await;
    window.apply_redaction("text", "hunter2", "asterisks", "", "output");
    window.apply_speed("10", "20", "2");
    window.click(window.remove_button());
    let page = window.html();
    assert!(!page.contains(&speed), "{page}");
    assert!(page.contains(&redaction), "{page}");
    assert!(page.contains(&edited_row("1m 5.3s")), "{page}");
}

#[test]
fn a_redaction_of_a_range_shows_its_region() {
    use crate::filters::redaction::{self, Find, Redaction, Replacement};
    let redaction =
        Redaction::new(Find::Text("key".into()), Replacement::Blocks, false).expect("a redaction");
    let region = FilterRegion::Range {
        start: Seconds::new(10.0).expect("a time"),
        end: Seconds::new(20.0).expect("a time"),
    };
    let filter = redaction::applied(region, &redaction);
    let page = render_edited_panels(fixture(), vec![Edit::AddFilter { index: 0, filter }]);
    let row = redaction_row("text \u{201c}key\u{201d} \u{2192} blocks (output), 10 s to 20 s");
    assert!(page.contains(&row), "{row} missing in {page}");
}

/// The page markup of a run of `count` letters x: the run ends at the cursor.
fn x_run(count: usize) -> String {
    format!(">{}</span>", "x".repeat(count))
}

#[tokio::test]
async fn scrubbing_the_timeline_moves_the_preview() {
    let mut window = open_recording().await;
    let control = window.playhead_control();
    let page = window.html();
    assert!(page.contains("max=\"65.3\""), "{page}");
    assert!(page.contains("aria-label=\"Playhead\""), "{page}");
    assert!(page.contains(&x_run(1)), "{page}");

    window.type_into(control, "1.0");
    let page = window.html();
    assert!(page.contains(&x_run(11)), "{page}");
    assert!(!page.contains(&x_run(12)), "{page}");
    assert!(page.contains("value=\"1\""), "{page}");

    window.type_into(control, "0.5");
    let page = window.html();
    assert!(page.contains(&x_run(6)), "{page}");
    assert!(!page.contains(&x_run(7)), "{page}");

    window.type_into(control, "abc");
    let page = window.html();
    assert!(page.contains(&x_run(6)), "{page}");
    assert!(page.contains("value=\"0.5\""), "{page}");
}

#[tokio::test]
async fn an_edit_updates_the_preview_at_the_same_playhead() {
    let mut window = open_recording().await;
    let control = window.playhead_control();
    window.type_into(control, "1.0");
    assert!(window.html().contains(&x_run(11)));
    window.apply_speed("0", "10", "2");
    let page = window.html();
    assert!(page.contains(&x_run(21)), "{page}");
    assert!(!page.contains(&x_run(22)), "{page}");
}

#[tokio::test]
async fn a_new_recording_starts_at_time_zero() {
    let fakes = Fakes::new()
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())))
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())));
    let mut window = Window::open(&fakes);
    window.click(window.open_button());
    window.settle().await;
    let control = window.playhead_control();
    window.type_into(control, "1.0");
    assert!(window.html().contains(&x_run(11)));

    window.click(window.open_button());
    window.settle().await;
    let page = window.html();
    assert!(page.contains(&x_run(1)), "{page}");
    assert!(!page.contains(&x_run(2)), "{page}");
    assert!(page.contains("value=\"0\""), "{page}");
}

#[tokio::test]
async fn a_new_recording_opens_with_the_whole_timeline() {
    let fakes = Fakes::new()
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())))
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())));
    let mut window = Window::open(&fakes);
    window.click(window.open_button());
    window.settle().await;
    window.note_open_buttons();
    let whole = window.clip_style();
    window.click(window.zoom_in_button());
    assert_ne!(window.clip_style(), whole);

    window.click(window.open_button());
    window.settle().await;
    assert_eq!(window.clip_style(), whole);
}

#[tokio::test]
async fn a_playhead_past_the_end_stops_at_the_end() {
    let mut window = open_recording().await;
    let control = window.playhead_control();
    window.type_into(control, "999");
    let page = window.html();
    assert!(page.contains("value=\"65.3\""), "{page}");
    assert!(page.contains(&x_run(42)), "{page}");
    window.type_into(control, "-1");
    assert!(window.html().contains("value=\"65.3\""));
}

/// The readout of the fixture at time zero.
const READOUT_AT_ZERO: &str = "0m 0.0s / 1m 5.3s";

#[tokio::test]
async fn the_toolbar_shows_the_time_and_stop_returns_to_the_start() {
    let mut window = open_recording().await;
    let page = window.html();
    assert!(page.contains(">Play</button>"), "{page}");
    assert!(page.contains(">Stop</button>"), "{page}");
    assert_eq!(window.readout(), READOUT_AT_ZERO);

    let control = window.playhead_control();
    window.type_into(control, "1.0");
    assert_eq!(window.readout(), "0m 1.0s / 1m 5.3s");

    window.click(window.stop_button());
    assert_eq!(window.readout(), READOUT_AT_ZERO);
    let page = window.html();
    assert!(page.contains(&x_run(1)), "{page}");
    assert!(!page.contains(&x_run(2)), "{page}");
}

#[tokio::test]
async fn play_moves_the_playhead_and_pause_holds_it() {
    let mut window = open_recording().await;
    window.click(window.play_button());
    assert_eq!(window.play_label(), "Pause");

    window.settle().await;
    let readout = window.readout();
    assert!(!readout.starts_with("0m 0.0s"), "{readout}");
    let page = window.html();
    assert!(!page.contains(&x_run(1)), "{page}");

    window.click(window.play_button());
    assert_eq!(window.play_label(), "Play");
    let held = window.readout();
    window.settle().await;
    assert_eq!(window.readout(), held);
}

#[tokio::test]
async fn playback_stops_at_the_end() {
    let mut loaded = fixture();
    loaded.recording.duration = 0.2;
    let mut window = open_loaded(loaded).await;
    window.click(window.play_button());
    assert_eq!(window.play_label(), "Pause");

    window.settle().await;

    assert_eq!(window.play_label(), "Play");
    assert_eq!(window.readout(), "0m 0.2s / 0m 0.2s");
}

#[tokio::test]
async fn play_at_the_end_starts_again_from_the_beginning() {
    let mut window = open_recording().await;
    let control = window.playhead_control();
    window.type_into(control, "65.3");
    assert_eq!(window.readout(), "1m 5.3s / 1m 5.3s");

    window.click(window.play_button());

    assert_eq!(window.readout(), READOUT_AT_ZERO);
    assert_eq!(window.play_label(), "Pause");
    window.click(window.play_button());
}

#[tokio::test]
async fn a_new_recording_opens_stopped() {
    let fakes = Fakes::new()
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())))
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())));
    let mut window = Window::open(&fakes);
    window.click(window.open_button());
    window.settle().await;
    window.note_open_buttons();
    window.click(window.play_button());
    assert_eq!(window.play_label(), "Pause");

    window.click(window.open_button());
    window.settle().await;

    assert_eq!(window.play_label(), "Play");
    assert_eq!(window.readout(), READOUT_AT_ZERO);
}

/// The page markup of the option of `choice`, selected or not.
fn theme_option(choice: PreviewTheme, selected: bool) -> String {
    let flag = if selected { " selected=true" } else { "" };
    format!(
        "<option value=\"{}\"{flag}>{}</option>",
        choice.name(),
        choice.label()
    )
}

#[tokio::test]
async fn the_theme_menu_lists_the_five_themes() {
    let window = open_recording().await;
    let page = window.html();
    // The redaction panel has menus of its own, so the test reads only the
    // label of the theme menu.
    let theme = text_between(&page, "<label class=\"preview-theme\">", "</label>");
    assert!(theme.starts_with("Theme<select"), "{theme}");
    assert_eq!(theme.matches("<select").count(), 1, "{theme}");
    assert_eq!(theme.matches("<option").count(), 5, "{theme}");
    let menu = text_between(&theme, "<select", "</select>");
    let listed: String = PreviewTheme::ALL
        .into_iter()
        .map(|choice| theme_option(choice, choice == PreviewTheme::Auto))
        .collect();
    assert!(menu.ends_with(&listed), "{menu}");
}

#[tokio::test]
async fn a_chosen_theme_recolors_the_preview() {
    let mut window = open_recording().await;
    let control = window.playhead_control();
    window.type_into(control, "1.0");

    window.choose(window.theme_menu(), "dracula");

    let page = window.html();
    assert!(
        page.contains("class=\"preview-screen\" style=\"background-color:#282a36\""),
        "{page}"
    );
    assert!(
        page.contains("style=\"color:#f8f8f2;background-color:#282a36\""),
        "{page}"
    );
    let terminal = text_between(&page, "class=\"terminal\"", "<div class=\"timeline\">");
    assert!(!terminal.contains("background-color:#000000"), "{terminal}");
    assert!(page.contains(&theme_option(PreviewTheme::Dracula, true)));
    assert!(page.contains(&theme_option(PreviewTheme::Auto, false)));
    assert!(page.contains(&x_run(11)), "{page}");
    assert!(!page.contains(&x_run(12)), "{page}");
}

#[tokio::test]
async fn an_unknown_theme_name_changes_nothing() {
    let mut window = open_recording().await;
    let menu = window.theme_menu();
    // Leave the default first, so a reset to Auto shows as a change.
    window.choose(menu, PreviewTheme::Dracula.name());

    for name in ["black", "Dracula", ""] {
        window.choose(menu, name);
        let page = window.html();
        assert!(
            page.contains("class=\"preview-screen\" style=\"background-color:#282a36\""),
            "{name}: {page}"
        );
        assert!(
            page.contains("style=\"color:#f8f8f2;background-color:#282a36\""),
            "{name}: {page}"
        );
        assert!(page.contains(&theme_option(PreviewTheme::Dracula, true)));
        assert!(page.contains(&theme_option(PreviewTheme::Auto, false)));
    }
}

#[tokio::test]
async fn the_theme_stays_when_a_new_recording_opens() {
    let fakes = Fakes::new()
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())))
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())));
    let mut window = Window::open(&fakes);
    window.click(window.open_button());
    window.settle().await;
    window.choose(window.theme_menu(), "monokai");
    let page = window.html();
    assert!(
        page.contains("style=\"background-color:#272822\""),
        "{page}"
    );

    window.click(window.open_button());
    window.settle().await;

    let page = window.html();
    assert!(
        page.contains("style=\"background-color:#272822\""),
        "{page}"
    );
    assert!(page.contains(&theme_option(PreviewTheme::Monokai, true)));
}

/// The declarations of the rule that starts with `selector`, from the style sheet.
fn rule_body(selector: &str) -> String {
    text_between(STYLE, &format!("{selector} {{"), "}")
}

/// The selectors of the rules of the style sheet, in source order.
fn style_selectors() -> Vec<&'static str> {
    STYLE
        .split('}')
        .filter_map(|rule| rule.split_once('{'))
        .map(|(selector, _)| selector.trim())
        .collect()
}

/// The specificity of a CSS selector (Selectors Level 4, section 17). The
/// fields are in the order of the comparison: a field counts only when each
/// field before it is equal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Specificity {
    /// The id selectors.
    ids: u32,
    /// The class selectors and the pseudo-classes.
    classes: u32,
    /// The type selectors.
    types: u32,
}

impl Specificity {
    /// The specificity of `selector`. `:where()` adds nothing, and `:not()`
    /// and `:is()` add the specificity of their argument. A form that this
    /// function does not know fails the test, so that no count is wrong in
    /// silence.
    fn of(selector: &str) -> Self {
        /// The name at the start of `text`, and the text after it.
        fn split_name(text: &str) -> (&str, &str) {
            let end = text
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                .unwrap_or(text.len());
            text.split_at(end)
        }

        /// The argument at the start of `text`, which starts after a `(`, and
        /// the text after the `)` that closes it.
        fn split_argument(text: &str) -> Option<(&str, &str)> {
            let mut depth = 0_u32;
            for (index, character) in text.char_indices() {
                match character {
                    '(' => depth += 1,
                    ')' if depth == 0 => {
                        let (argument, closed) = text.split_at(index);
                        return Some((argument, closed.strip_prefix(')')?));
                    }
                    ')' => depth -= 1,
                    _ => {}
                }
            }
            None
        }

        let mut counts = Self::default();
        let mut rest = selector.trim();
        while let Some(next) = rest.chars().next() {
            rest = if let Some(after) = rest.strip_prefix('#') {
                counts.ids += 1;
                split_name(after).1
            } else if let Some(after) = rest.strip_prefix('.') {
                counts.classes += 1;
                split_name(after).1
            } else if let Some(after) = rest.strip_prefix(':') {
                let (name, after) = split_name(after);
                match after.strip_prefix('(') {
                    None => {
                        counts.classes += 1;
                        after
                    }
                    Some(inner) => {
                        let (argument, after) = split_argument(inner)
                            .unwrap_or_else(|| panic!("no ) after :{name}( in {selector}"));
                        let added = match name {
                            "where" => Self::default(),
                            "not" | "is" => Self::of(argument),
                            _ => panic!("no rule for :{name}() in {selector}"),
                        };
                        counts.ids += added.ids;
                        counts.classes += added.classes;
                        counts.types += added.types;
                        after
                    }
                }
            } else if let Some(after) = rest.strip_prefix([' ', '>', '+', '~', '*']) {
                after
            } else if next.is_ascii_alphabetic() {
                counts.types += 1;
                split_name(rest).1
            } else {
                panic!("no rule for {rest:?} in {selector:?}")
            };
        }
        counts
    }
}

/// The hover rule of every button must lose to the hover rule of the Dismiss
/// button of the error banner. Else the dark text of that button stands on the
/// dark hover color of the other buttons, on a pink banner.
#[test]
fn the_dismiss_button_keeps_its_own_hover_color() {
    let selectors = style_selectors();
    // The cascade takes the rule of the higher specificity, and of two equal
    // specificities, the rule that comes later.
    let rank = |selector: &str| {
        let position = selectors
            .iter()
            .position(|candidate| *candidate == selector)
            .unwrap_or_else(|| panic!("no rule {selector} in {STYLE}"));
        (Specificity::of(selector), position)
    };
    let dismiss = rank(".error-dismiss:hover");
    let every_button: Vec<&str> = selectors
        .iter()
        .copied()
        .filter(|selector| selector.starts_with("button:hover"))
        .collect();
    assert!(!every_button.is_empty(), "no button:hover rule in {STYLE}");
    for selector in every_button {
        assert!(
            dismiss > rank(selector),
            "{selector} {:?} beats .error-dismiss:hover {dismiss:?}",
            rank(selector)
        );
    }
}

#[test]
fn the_style_sheet_fits_the_grid_to_the_pane() {
    let pane = rule_body(".preview-screen");
    assert!(pane.contains("container-type: size"), "{pane}");
    let terminal = rule_body(".terminal");
    let size = text_between(&terminal, "font-size:", ";");
    // A cell is 0.6 em wide and one line high, so the rows divide the height
    // by the line height of the same rule.
    let line_height = text_between(&terminal, "line-height:", ";");
    let line_height = line_height.trim();
    assert_eq!(
        size.trim(),
        format!(
            "min(calc(100cqw / (var(--columns) * 0.6)), \
             calc(100cqh / (var(--rows) * {line_height})))"
        )
    );
}

// The Undo and Redo buttons of the menu bar.

/// The page markup of a run of `count` asterisks: the run ends at the cursor.
fn asterisk_run(count: usize) -> String {
    format!(">{}</span>", "*".repeat(count))
}

/// The row of the speed change from 0 s to 10 s at twice the speed.
fn first_ten_seconds_row() -> String {
    speed_row("0 s to 10 s", "2")
}

/// Opens the fixture with the playhead at one second, and doubles the speed of
/// the first ten seconds. The preview then shows 21 events, not 11.
async fn open_with_a_speed_change() -> Window {
    let mut window = open_recording().await;
    window.type_into(window.playhead_control(), "1.0");
    window.apply_speed("0", "10", "2");
    window
}

/// Asserts that the window shows the project with the speed change of
/// `open_with_a_speed_change`: its row, its preview, its edited duration, and
/// the end of its timeline.
fn assert_sped_up(window: &Window) {
    let page = window.html();
    assert!(page.contains(&first_ten_seconds_row()), "{page}");
    assert!(page.contains(&x_run(21)), "{page}");
    assert!(!page.contains(&x_run(22)), "{page}");
    assert!(page.contains(&edited_row("1m 0.3s")), "{page}");
    assert!(!page.contains("max=\"65.3\""), "{page}");
}

/// Asserts that the window shows the project with no speed change.
fn assert_original_speed(window: &Window) {
    let page = window.html();
    assert!(!page.contains(&first_ten_seconds_row()), "{page}");
    assert!(page.contains(&x_run(11)), "{page}");
    assert!(!page.contains(&x_run(12)), "{page}");
    assert!(page.contains(&edited_row("1m 5.3s")), "{page}");
    assert!(page.contains("max=\"65.3\""), "{page}");
}

#[test]
fn undo_and_redo_are_disabled_with_no_recording() {
    let window = Window::open(&Fakes::new());
    assert_eq!(window.history_disabled(), [true, true], "{}", window.html());
}

#[tokio::test]
async fn undo_and_redo_are_enabled_only_when_the_editor_has_a_step() {
    let mut window = open_recording().await;
    assert_eq!(window.history_disabled(), [true, true], "{}", window.html());

    window.apply_speed("10", "20", "2");
    assert_eq!(
        window.history_disabled(),
        [false, true],
        "{}",
        window.html()
    );

    window.click(window.undo_button());
    assert_eq!(
        window.history_disabled(),
        [true, false],
        "{}",
        window.html()
    );

    window.click(window.redo_button());
    assert_eq!(
        window.history_disabled(),
        [false, true],
        "{}",
        window.html()
    );
}

#[tokio::test]
async fn undo_and_redo_of_a_speed_change_update_the_preview_the_timeline_and_the_row() {
    let mut window = open_with_a_speed_change().await;
    assert_sped_up(&window);

    window.click(window.undo_button());
    assert_original_speed(&window);

    window.click(window.redo_button());
    assert_sped_up(&window);
}

#[tokio::test]
async fn undo_and_redo_of_a_redaction_update_the_preview_and_the_row() {
    let mut window = open_recording().await;
    window.type_into(window.playhead_control(), "1.0");
    window.apply_redaction("text", "x", "asterisks", "", "output");
    let row = redaction_row("text \u{201c}x\u{201d} \u{2192} asterisks (output)");
    let assert_redacted = |window: &Window| {
        let page = window.html();
        assert!(page.contains(&row), "{page}");
        assert!(page.contains(&asterisk_run(11)), "{page}");
        assert!(!page.contains(&x_run(11)), "{page}");
    };
    assert_redacted(&window);

    window.click(window.undo_button());
    let page = window.html();
    assert!(!page.contains(&row), "{page}");
    assert!(page.contains(&x_run(11)), "{page}");
    assert!(!page.contains(&asterisk_run(11)), "{page}");

    window.click(window.redo_button());
    assert_redacted(&window);
}

#[tokio::test]
async fn undo_and_redo_of_a_removed_row_put_it_back_and_take_it_away() {
    let mut window = open_with_a_speed_change().await;
    window.click(window.remove_button());
    assert_original_speed(&window);

    window.click(window.undo_button());
    assert_sped_up(&window);
    assert_eq!(
        window.history_disabled(),
        [false, false],
        "{}",
        window.html()
    );

    window.click(window.redo_button());
    assert_original_speed(&window);
}

#[tokio::test]
async fn an_undo_removes_the_banner_of_a_failed_edit() {
    let mut window = open_with_a_speed_change().await;
    window.apply_speed("10", "5", "2");
    assert!(
        window.html().contains("role=\"alert\""),
        "{}",
        window.html()
    );

    window.click(window.undo_button());

    assert!(
        !window.html().contains("role=\"alert\""),
        "{}",
        window.html()
    );
    assert_original_speed(&window);
}

// The keys of Undo and Redo, and the tooltips that name them.

/// One key press: the value of the key, its place on the keyboard, and the
/// modifiers that are down.
struct Press {
    text: &'static str,
    code: Code,
    modifiers: Modifiers,
}

/// The modifier of the shortcuts on the platform of the test.
const HOST_MODIFIER: Modifiers = if cfg!(target_os = "macos") {
    Modifiers::META
} else {
    Modifiers::CONTROL
};

/// The modifier of the shortcuts on the other platforms.
const OTHER_MODIFIER: Modifiers = if cfg!(target_os = "macos") {
    Modifiers::CONTROL
} else {
    Modifiers::META
};

/// The tooltip of the Undo button on the platform of the test.
const UNDO_TOOLTIP: &str = if cfg!(target_os = "macos") {
    "Undo (\u{2318}Z)"
} else {
    "Undo (Ctrl+Z)"
};

/// The tooltip of the Redo button on the platform of the test.
const REDO_TOOLTIP: &str = if cfg!(target_os = "macos") {
    "Redo (\u{21e7}\u{2318}Z)"
} else {
    "Redo (Ctrl+Shift+Z)"
};

/// Cmd+Z on macOS, and Ctrl+Z on the other platforms.
const UNDO_KEYS: Press = Press {
    text: "z",
    code: Code::KeyZ,
    modifiers: HOST_MODIFIER,
};

/// Cmd+Shift+Z on macOS, and Ctrl+Shift+Z on the other platforms.
const REDO_KEYS: Press = Press {
    text: "Z",
    code: Code::KeyZ,
    modifiers: HOST_MODIFIER.union(Modifiers::SHIFT),
};

impl Window {
    /// The Start, End, and Speed fields of the speed panel, and the Pattern and
    /// Replacement text fields of the redaction panel.
    fn text_fields(&self) -> [ElementId; 5] {
        let [start, end, speed] = self.speed_fields();
        let [_, pattern, _, replacement, _] = self.redaction_fields();
        [start, end, speed, pattern, replacement]
    }

    /// Elements that have the focus in turn and are not text fields: the
    /// Apply Speed button, the Find menu, the playhead control, and the theme
    /// menu.
    fn other_controls(&self) -> [ElementId; 4] {
        let [find, ..] = self.redaction_fields();
        [
            self.apply_button(),
            find,
            self.playhead_control(),
            self.theme_menu(),
        ]
    }
}

#[tokio::test]
async fn the_keys_undo_and_redo_when_a_control_that_is_no_text_field_has_the_focus() {
    let mut window = open_with_a_speed_change().await;
    for control in window.other_controls() {
        assert!(window.press(control, &UNDO_KEYS), "{control:?}");
        assert_original_speed(&window);
        assert_eq!(window.history_disabled(), [true, false]);

        assert!(window.press(control, &REDO_KEYS), "{control:?}");
        assert_sped_up(&window);
        assert_eq!(window.history_disabled(), [false, true]);
    }
}

#[tokio::test]
async fn the_keys_keep_their_text_field_behavior_in_each_text_field() {
    let mut window = open_with_a_speed_change().await;
    for field in window.text_fields() {
        assert!(!window.press(field, &UNDO_KEYS), "{field:?}");
        assert_sped_up(&window);
    }

    window.click(window.undo_button());
    for field in window.text_fields() {
        assert!(!window.press(field, &REDO_KEYS), "{field:?}");
        assert_original_speed(&window);
    }
}

#[tokio::test]
async fn a_key_with_no_step_changes_nothing() {
    let mut window = open_with_a_speed_change().await;
    let control = window.apply_button();
    assert!(window.press(control, &REDO_KEYS));
    assert_sped_up(&window);

    assert!(window.press(control, &UNDO_KEYS));
    assert!(window.press(control, &UNDO_KEYS));
    assert_original_speed(&window);
    assert!(!window.html().contains("role=\"alert\""));

    let mut empty = Window::open(&Fakes::new());
    for keys in [&UNDO_KEYS, &REDO_KEYS] {
        assert!(empty.press(empty.open_button(), keys));
    }
    assert_eq!(empty.html(), render_empty_app());
}

#[tokio::test]
async fn other_keys_change_nothing_and_keep_their_default_action() {
    let mut window = open_with_a_speed_change().await;
    let control = window.apply_button();
    let presses = [
        Press {
            text: "z",
            code: Code::KeyZ,
            modifiers: Modifiers::empty(),
        },
        Press {
            text: "Z",
            code: Code::KeyZ,
            modifiers: Modifiers::SHIFT,
        },
        Press {
            text: "z",
            code: Code::KeyZ,
            modifiers: OTHER_MODIFIER,
        },
        Press {
            text: "z",
            code: Code::KeyZ,
            modifiers: HOST_MODIFIER.union(Modifiers::ALT),
        },
        Press {
            text: "y",
            code: Code::KeyY,
            modifiers: HOST_MODIFIER,
        },
    ];
    for press in &presses {
        assert!(!window.press(control, press), "{}", press.text);
        assert_sped_up(&window);
    }
}

#[test]
fn the_tooltip_of_each_button_names_its_keys() {
    let page = render_empty_app();
    for tooltip in [UNDO_TOOLTIP, REDO_TOOLTIP] {
        assert!(
            page.contains(&format!("title=\"{tooltip}\"")),
            "{tooltip} missing in {page}"
        );
    }
}
