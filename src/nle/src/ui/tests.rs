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
use crate::types::{Chapter, Event, EventType, Recording, RecordingMetadata};
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
pub(super) fn fixture() -> LoadedRecording {
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
            chapters: Vec::new(),
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
    rsx! {
        PreviewPane { rendered, playhead, playing }
        Timeline { loaded, rendered, playhead }
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
    /// The id of each element that got an input listener, in creation order.
    inputs: Vec<ElementId>,
    /// The ids that the newest render created, when it created any.
    newest: Vec<ElementId>,
    /// The click listeners of the first render.
    first_render: Vec<ElementId>,
    /// The id of each element that got a change listener, in creation order.
    changes: Vec<ElementId>,
    /// The Apply Speed button, when a test noted it.
    apply: Option<ElementId>,
    /// The Play and Pause button, when a test noted it.
    play: Option<ElementId>,
    /// The Stop button, when a test noted it.
    stop: Option<ElementId>,
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
            inputs: Vec::new(),
            newest: Vec::new(),
            first_render: Vec::new(),
            changes: Vec::new(),
            apply: None,
            play: None,
            stop: None,
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
        if !created.is_empty() {
            self.clicks.extend(&created);
            self.newest = created;
        }
    }

    /// Renders what is dirty.
    fn render(&mut self) {
        let edits = self.dom.render_immediate_to_vec().edits;
        self.note_listeners(&edits);
    }

    /// The Open Recording button. It is the one click listener of the first render.
    fn open_button(&self) -> ElementId {
        assert_eq!(self.first_render.len(), 1, "{:?}", self.first_render);
        self.first_render[0]
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
        let form = dioxus::html::SerializedFormData::new(text.to_owned(), Vec::new());
        let data = Rc::new(PlatformEventData::new(Box::new(form)));
        let event = dioxus::prelude::Event::new(data as Rc<dyn Any>, true);
        self.dom.runtime().handle_event("input", event, id);
        self.render();
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

    /// The input listeners that the open render of one recording created, in
    /// creation order: the Start, End, and Speed fields of the speed panel,
    /// the time, the label, and the search fields of the chapter panel, then
    /// the playhead control of the timeline. The properties panel mounts
    /// before the timeline, so the playhead is the last one.
    fn open_inputs(&self) -> [ElementId; 7] {
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

    /// The time, the label, and the search fields of the chapter panel, in
    /// this order.
    fn chapter_fields(&self) -> [ElementId; 3] {
        let [_, _, _, time, label, search, _] = self.open_inputs();
        [time, label, search]
    }

    /// The playhead control of the timeline.
    fn playhead_control(&self) -> ElementId {
        let [.., playhead] = self.open_inputs();
        playhead
    }

    /// The click listeners that the render of a load creates, in creation
    /// order: Apply Speed of the speed panel, Add Chapter of the chapter
    /// panel, then Play and Stop of the preview toolbar. The properties panel
    /// mounts before the preview toolbar, as it does for the input listeners.
    /// A recording with chapters adds the buttons of its rows, thus this list
    /// is for a recording with no chapter.
    fn load_buttons(&self) -> [ElementId; 4] {
        self.newest
            .clone()
            .try_into()
            .unwrap_or_else(|ids: Vec<ElementId>| panic!("the load made the buttons {ids:?}"))
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
        let page = self.html();
        let (before, _) = page
            .split_once(&format!(">{OPEN_LABEL}</button>"))
            .unwrap_or_else(|| panic!("no Open Recording button in {page}"));
        let (_, tag) = before.rsplit_once("<button").expect("a button start tag");
        tag.split([' ', '>', '=']).any(|word| word == "disabled")
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
    assert!(page.contains("Timeline editor"), "{page}");
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
    assert_eq!(window.first_render.len(), 1);
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

#[test]
fn properties_of_an_open_document_show_the_chapter_panel() {
    let page = render_panels(fixture());
    assert!(page.contains("class=\"chapters-panel\""), "{page}");
    assert!(page.contains(">Chapters<"), "{page}");
    assert!(page.contains("No chapters yet"), "{page}");
}

#[test]
fn properties_with_no_recording_show_no_chapter_panel() {
    let page = render_empty_app();
    assert!(!page.contains("class=\"chapters-panel\""), "{page}");
}

#[tokio::test]
async fn a_failed_chapter_action_shows_the_banner_and_a_good_one_removes_it() {
    let fakes = Fakes::new()
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())));
    let mut window = Window::open(&fakes);
    window.click(window.open_button());
    window.settle().await;
    assert!(
        !window.html().contains("role=\"alert\""),
        "{}",
        window.html()
    );
    let [_, add, _, _] = window.load_buttons();
    let [time, label, _] = window.chapter_fields();
    window.click(add);
    let page = window.html();
    assert!(page.contains("role=\"alert\""), "{page}");
    assert!(
        page.contains("Failed to change the chapters: the time is empty"),
        "{page}"
    );
    window.type_into(time, "5");
    window.type_into(label, "Intro");
    window.click(add);
    let page = window.html();
    assert!(!page.contains("role=\"alert\""), "{page}");
    assert!(
        page.contains("<span class=\"chapter-label\">Intro</span>"),
        "{page}"
    );
}

#[tokio::test]
async fn a_second_recording_starts_the_chapter_panel_with_no_edit_and_no_search() {
    let mut second = fixture();
    second.recording.title = "Second Recording".into();
    second.metadata.title = "Second Recording".into();
    second.recording.chapters = vec![Chapter {
        time: 1.0,
        label: "Setup".into(),
    }];
    let fakes = Fakes::new()
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())))
        .pick(picked())
        .load(Answer::Ready(Ok(second)));
    let mut window = Window::open(&fakes);
    window.click(window.open_button());
    window.settle().await;
    let [_, add, _, _] = window.load_buttons();
    let [time, label, search] = window.chapter_fields();
    window.type_into(time, "5");
    window.type_into(label, "Intro");
    window.click(add);
    // The render of the new row created its Edit button first.
    let edit = window.newest[0];
    window.click(edit);
    window.type_into(search, "intro");
    assert!(
        window.html().contains(">Save Chapter<"),
        "{}",
        window.html()
    );

    window.click(window.open_button());
    window.settle().await;

    let page = window.html();
    assert!(page.contains("Second Recording"), "{page}");
    assert!(page.contains(">Add Chapter<"), "{page}");
    assert!(!page.contains(">Save Chapter<"), "{page}");
    assert!(
        page.contains("<span class=\"chapter-label\">Setup</span>"),
        "{page}"
    );
}

/// The row text of a speed change as the page shows it.
fn speed_row(region: &str, speed: &str) -> String {
    format!("<span>{region} at {speed}\u{d7}</span>")
}

/// The row of the page that shows the duration after the edits.
fn edited_row(text: &str) -> String {
    metadata_row("Edited duration", text, None)
}

/// Opens the fixture recording through the fakes, and notes the Apply Speed,
/// Play, and Stop buttons of the open render. `Window::load_buttons` gives
/// their order.
async fn open_recording() -> Window {
    open_loaded(fixture()).await
}

/// Opens `loaded` through the fakes, and notes the three buttons as
/// `open_recording` does. `loaded` must hold no chapter.
async fn open_loaded(loaded: LoadedRecording) -> Window {
    let fakes = Fakes::new().pick(picked()).load(Answer::Ready(Ok(loaded)));
    let mut window = Window::open(&fakes);
    window.click(window.open_button());
    window.settle().await;
    let [apply, _, play, stop] = window.load_buttons();
    window.play = Some(play);
    window.stop = Some(stop);
    window.apply = Some(apply);
    window
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
    let [_, _, play, _] = window.load_buttons();
    window.click(play);
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
    assert!(page.contains("Theme"), "{page}");
    assert_eq!(page.matches("<select").count(), 1, "{page}");
    assert_eq!(page.matches("<option").count(), 5, "{page}");
    let menu = text_between(&page, "<select", "</select>");
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
