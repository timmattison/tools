//! Tests of the regions of the window. Each test renders the components to
//! text with the Dioxus SSR renderer and reads that text.

use super::*;
use std::any::Any;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use crate::format::format_recorded;
use crate::types::{Event, EventType, Recording, RecordingMetadata};
use dioxus::html::SerializedFormData;
use dioxus_core::{ElementId, Mutation};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::sync::oneshot;

use crate::editor::Edit;
use crate::error::NleError;
use crate::export::{ExportProgress, ExportRequest, ExportTarget, Theme};
use crate::types::{ClipId, Seconds};
use crate::ui::export::SaveDialog;
use std::num::NonZeroU32;

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
        let mut document =
            Document::open(input.recording, Arc::new(FilterRegistry::new())).expect("a document");
        for edit in input.edits {
            document.editor.apply(edit).expect("an edit that applies");
        }
        Some(document)
    });
    rsx! {
        PreviewPane { loaded }
        Timeline { loaded }
        PropertiesPanel { loaded }
    }
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

type ExportOutcome = Result<PathBuf, NleError>;

/// What a fake export does: it sends each stage of `progress` at once, then it
/// waits for `outcome`.
struct FakeExport {
    progress: Vec<ExportProgress>,
    outcome: Answer<ExportOutcome>,
}

/// What a fake export got: the request, and the render of the editor.
struct ExportCall {
    request: ExportRequest,
    render: Recording,
}

/// What the fakes saw and what they will answer.
#[derive(Default)]
struct Script {
    picks: usize,
    loads: Vec<PathBuf>,
    pick_answers: VecDeque<Answer<Option<PathBuf>>>,
    load_answers: VecDeque<Answer<LoadResult>>,
    /// Each save dialog that the fake showed.
    dialogs: Vec<SaveDialog>,
    dialog_answers: VecDeque<Answer<Option<PathBuf>>>,
    exports: Vec<ExportCall>,
    export_answers: VecDeque<FakeExport>,
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

    /// Queues the answer of the next save dialog.
    fn dialog(self, answer: Answer<Option<PathBuf>>) -> Self {
        self.script().dialog_answers.push_back(answer);
        self
    }

    /// Queues the next export: the stages that it reports at once, and its end.
    fn export(self, progress: Vec<ExportProgress>, outcome: Answer<ExportOutcome>) -> Self {
        self.script()
            .export_answers
            .push_back(FakeExport { progress, outcome });
        self
    }

    /// The save dialogs that the fake showed, oldest first.
    fn dialogs(&self) -> Vec<SaveDialog> {
        self.script().dialogs.clone()
    }

    /// The requests of the exports that the fake got, oldest first.
    fn export_requests(&self) -> Vec<ExportRequest> {
        let script = self.script();
        script
            .exports
            .iter()
            .map(|call| call.request.clone())
            .collect()
    }

    /// The renders of the editors that the exports got, oldest first.
    fn export_renders(&self) -> Vec<Recording> {
        let script = self.script();
        script
            .exports
            .iter()
            .map(|call| call.render.clone())
            .collect()
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
        let dialog_script = Arc::clone(&self.script);
        let export_script = Arc::clone(&self.script);
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
            move |dialog| {
                let mut script = dialog_script.lock().expect("the script lock");
                script.dialogs.push(dialog);
                let answer = script
                    .dialog_answers
                    .pop_front()
                    .expect("an unexpected save dialog");
                drop(script);
                Box::pin(answer.resolve())
            },
            move |editor, request, sender| {
                let render = editor.render().expect("a render");
                let mut script = export_script.lock().expect("the script lock");
                script.exports.push(ExportCall { request, render });
                let fake = script
                    .export_answers
                    .pop_front()
                    .expect("an unexpected export");
                drop(script);
                for stage in fake.progress {
                    sender.send(stage).expect("the export bar listens");
                }
                Box::pin(fake.outcome.resolve())
            },
        )
    }
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
    /// The id of each element that got a listener, by event name, in creation
    /// order.
    listeners: BTreeMap<String, Vec<ElementId>>,
    /// For each event name, the ids that the newest render created for it.
    newest_listeners: BTreeMap<String, Vec<ElementId>>,
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
            listeners: BTreeMap::new(),
            newest_listeners: BTreeMap::new(),
        };
        window.note_listeners(&edits);
        window.first_render = window.clicks.clone();
        window
    }

    /// Keeps the id of each new listener in `edits`, by event name, and the id
    /// of each new click listener in `clicks`.
    fn note_listeners(&mut self, edits: &[Mutation]) {
        let mut created_by_name: BTreeMap<String, Vec<ElementId>> = BTreeMap::new();
        for edit in edits {
            if let Mutation::NewEventListener { name, id } = edit {
                created_by_name.entry(name.clone()).or_default().push(*id);
            }
        }
        for (name, ids) in created_by_name {
            self.listeners.entry(name.clone()).or_default().extend(&ids);
            self.newest_listeners.insert(name, ids);
        }
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

    /// The Export button. It is the second click listener that the app ever
    /// made: the first is the Open Recording button, and the export bar
    /// appears with the first document, before any banner.
    fn export_button(&self) -> ElementId {
        assert!(self.html().contains(EXPORT_BAR_TAG), "no export bar");
        let clicks = &self.listeners["click"];
        assert!(clicks.len() >= 2, "{clicks:?}");
        clicks[1]
    }

    /// The select of the kind of file. It is the first `change` listener that
    /// the app ever made, because the export bar starts with it, and the other
    /// controls of the bar come after it.
    fn kind_select(&self) -> ElementId {
        assert!(self.html().contains(EXPORT_BAR_TAG), "no export bar");
        self.listeners
            .get("change")
            .and_then(|ids| ids.first().copied())
            .unwrap_or_else(|| panic!("no kind select in {}", self.html()))
    }

    /// The FPS field. It is the first of the `input` listeners that the newest
    /// render of such listeners made, because the FPS field comes before the
    /// resolution field in a bar that shows both.
    fn fps_input(&self) -> ElementId {
        self.newest_listeners
            .get("input")
            .and_then(|ids| ids.first().copied())
            .unwrap_or_else(|| panic!("no FPS field in {}", self.html()))
    }

    /// The ids of the listeners of the event `name` that the newest render of
    /// such listeners created, in creation order.
    fn newest_listeners_of(&self, name: &str) -> Vec<ElementId> {
        self.newest_listeners
            .get(name)
            .cloned()
            .unwrap_or_else(|| panic!("no `{name}` listeners in {}", self.html()))
    }

    /// Sends a change of a form control to the element, and renders.
    fn change(&mut self, id: ElementId, value: &str) {
        self.send_form_event("change", id, value);
    }

    /// Sends an input to a text field, and renders.
    fn input(&mut self, id: ElementId, value: &str) {
        self.send_form_event("input", id, value);
    }

    /// Sends a form event with `value` under the event `name`, and renders.
    fn send_form_event(&mut self, name: &str, id: ElementId, value: &str) {
        let form = SerializedFormData::new(value.to_string(), vec![]);
        let data = Rc::new(PlatformEventData::new(Box::new(form)));
        let event = dioxus::prelude::Event::new(data as Rc<dyn Any>, true);
        self.dom.runtime().handle_event(name, event, id);
        self.render();
    }

    /// Whether the start tag of the Export button holds `disabled`.
    fn export_is_disabled(&self) -> bool {
        let page = self.html();
        let (before, _) = page
            .split_once(">Export</button>")
            .unwrap_or_else(|| panic!("no Export button in {page}"));
        let (_, tag) = before.rsplit_once("<button").expect("a button start tag");
        tag.split([' ', '>', '=']).any(|word| word == "disabled")
    }

    /// Sends a click to the element and renders.
    fn click(&mut self, id: ElementId) {
        let data = Rc::new(PlatformEventData::new(Box::<SerializedMouseData>::default()));
        let event = dioxus::prelude::Event::new(data as Rc<dyn Any>, true);
        self.dom.runtime().handle_event("click", event, id);
        self.render();
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
fn preview_shows_the_size_of_the_recording() {
    let page = render_panels(fixture());
    assert!(page.contains("coming in #151"), "{page}");
    assert!(page.contains("120x40"), "{page}");
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

/// The `class` attribute of the export bar. The style sheet in the page names
/// the class too, so the bare class name proves nothing.
const EXPORT_BAR_TAG: &str = "class=\"export-bar\"";
/// The path that the fake save dialog picks.
const EXPORT_PICKED: &str = "/tmp/out";
/// The file that the fake export writes.
const EXPORT_WRITTEN: &str = "/tmp/out.json";

/// A fake script that opens the fixture recording.
fn fakes_with_recording() -> Fakes {
    Fakes::new()
        .pick(picked())
        .load(Answer::Ready(Ok(fixture())))
}

/// Opens a window and loads the fixture recording into it.
async fn window_with_recording(fakes: &Fakes) -> Window {
    let mut window = Window::open(fakes);
    window.click(window.open_button());
    window.settle().await;
    window
}

#[tokio::test]
async fn an_export_with_the_default_settings_writes_a_recording() {
    let fakes = fakes_with_recording()
        .dialog(Answer::Ready(Some(PathBuf::from(EXPORT_PICKED))))
        .export(Vec::new(), Answer::Ready(Ok(PathBuf::from(EXPORT_WRITTEN))));
    let mut window = window_with_recording(&fakes).await;
    window.click(window.export_button());
    window.settle().await;
    assert_eq!(
        fakes.dialogs(),
        vec![SaveDialog {
            file_name: "demo-edited.json".into(),
            directory: Some(PathBuf::from("/home/user/recordings")),
            filter_name: "Beta recording",
            extensions: &["json"],
        }]
    );
    assert_eq!(
        fakes.export_requests(),
        vec![ExportRequest {
            output_path: PathBuf::from(EXPORT_WRITTEN),
            target: ExportTarget::Recording {
                compress: Some(false)
            },
        }]
    );
    assert_eq!(fakes.export_renders(), vec![fixture().recording]);
    let page = window.html();
    assert!(
        page.contains(
            "<span class=\"export-status\" role=\"status\">Exported to /tmp/out.json</span>"
        ),
        "{page}"
    );
    assert!(!page.contains("role=\"alert\""), "{page}");
}

#[test]
fn app_with_no_recording_has_no_export_bar() {
    let page = render_empty_app();
    assert!(!page.contains(EXPORT_BAR_TAG), "{page}");
}

#[tokio::test]
async fn a_video_export_shows_its_progress_and_blocks_a_second_click() {
    let (finish, pending) = Answer::later();
    let stages = vec![
        ExportProgress::Rendering,
        ExportProgress::Writing,
        ExportProgress::Frames {
            done: 30,
            total: 300,
        },
    ];
    let fakes = fakes_with_recording()
        .dialog(Answer::Ready(Some(PathBuf::from(EXPORT_PICKED))))
        .export(stages, pending);
    let mut window = window_with_recording(&fakes).await;
    window.change(window.kind_select(), "mp4");
    window.input(window.fps_input(), "30");
    let button = window.export_button();
    window.click(button);
    window.settle().await;
    assert_eq!(fakes.dialogs()[0].file_name, "demo-edited.mp4");
    let page = window.html();
    assert!(page.contains("<progress"), "{page}");
    assert!(page.contains("value=\"30\""), "{page}");
    assert!(page.contains("max=\"300\""), "{page}");
    assert!(page.contains("Step 2 of 2: frame 30 of 300"), "{page}");
    assert!(window.export_is_disabled(), "{page}");
    window.click(button);
    window.settle().await;
    assert_eq!(
        (fakes.dialogs().len(), fakes.export_requests().len()),
        (1, 1)
    );
    finish
        .send(Ok(PathBuf::from("/tmp/out.mp4")))
        .expect("the export future waits");
    window.settle().await;
    let page = window.html();
    assert!(!page.contains("<progress"), "{page}");
    assert!(page.contains("Exported to /tmp/out.mp4"), "{page}");
    assert!(!window.export_is_disabled(), "{page}");
    assert_eq!(
        fakes.export_requests()[0],
        ExportRequest {
            output_path: PathBuf::from("/tmp/out.mp4"),
            target: ExportTarget::Video {
                theme: Theme::Auto,
                fps: NonZeroU32::new(30),
                resolution: None,
                optimize_web: false,
            },
        }
    );
}

/// The first export outcome that fails.
fn boom() -> Answer<ExportOutcome> {
    Answer::Ready(Err(NleError::ExportError(
        "beta export failed: boom".into(),
    )))
}

#[tokio::test]
async fn a_failed_export_shows_the_banner_until_an_export_works() {
    let fakes = fakes_with_recording()
        .dialog(Answer::Ready(Some(PathBuf::from(EXPORT_PICKED))))
        .export(Vec::new(), boom())
        .dialog(Answer::Ready(Some(PathBuf::from(EXPORT_PICKED))))
        .export(Vec::new(), Answer::Ready(Ok(PathBuf::from(EXPORT_WRITTEN))));
    let mut window = window_with_recording(&fakes).await;
    let button = window.export_button();
    window.click(button);
    window.settle().await;
    let page = window.html();
    assert!(page.contains("role=\"alert\""), "{page}");
    assert!(
        page.contains("Export failed: beta export failed: boom"),
        "{page}"
    );
    assert!(!page.contains("role=\"status\""), "{page}");
    assert!(!window.export_is_disabled(), "{page}");
    assert!(!page.contains("<progress"), "{page}");
    window.click(button);
    window.settle().await;
    let page = window.html();
    assert!(!page.contains("role=\"alert\""), "{page}");
    assert!(page.contains("Exported to /tmp/out.json"), "{page}");
}

#[tokio::test]
async fn a_bad_frame_rate_shows_the_banner_and_asks_no_dialog() {
    let fakes = fakes_with_recording();
    let mut window = window_with_recording(&fakes).await;
    window.change(window.kind_select(), "mp4");
    window.input(window.fps_input(), "0");
    window.click(window.export_button());
    window.settle().await;
    let page = window.html();
    assert!(page.contains("role=\"alert\""), "{page}");
    assert!(
        page.contains("Export failed: the frame rate &#34;0&#34; is not a whole number above zero"),
        "{page}"
    );
    assert!(fakes.dialogs().is_empty());
    assert!(!window.export_is_disabled(), "{page}");
}

#[tokio::test]
async fn a_cancelled_save_dialog_exports_nothing() {
    let fakes = fakes_with_recording().dialog(Answer::Ready(None));
    let mut window = window_with_recording(&fakes).await;
    window.click(window.export_button());
    window.settle().await;
    let page = window.html();
    assert_eq!(fakes.dialogs().len(), 1);
    assert!(fakes.export_requests().is_empty());
    assert!(!window.export_is_disabled(), "{page}");
    assert!(!page.contains("role=\"alert\""), "{page}");
    assert!(!page.contains("role=\"status\""), "{page}");
}

/// The markup of the Theme menu with its label.
const THEME_MARKUP: &str = "<label>Theme<select>";
/// The markup of the FPS field with its label.
const FPS_MARKUP: &str = "<label>FPS<input";
/// The markup of the Resolution field with its label.
const RESOLUTION_MARKUP: &str = "<label>Resolution<input";
/// The markup of the end of the Optimize for web box with its label.
const OPTIMIZE_MARKUP: &str = "/>Optimize for web</label>";
/// The markup of the end of the Compress box with its label.
const COMPRESS_MARKUP: &str = "/>Compress</label>";

/// Whether each of the five controls of the export bar is on the page, in the
/// order Theme, FPS, Resolution, Optimize for web, Compress.
fn shown_controls(page: &str) -> [bool; 5] {
    [
        THEME_MARKUP,
        FPS_MARKUP,
        RESOLUTION_MARKUP,
        OPTIMIZE_MARKUP,
        COMPRESS_MARKUP,
    ]
    .map(|markup| page.contains(markup))
}

#[tokio::test]
async fn each_kind_shows_its_own_controls() {
    let cases: [(&str, [bool; 5]); 5] = [
        ("json", [false, false, false, false, false]),
        ("json-gz", [false, false, false, false, false]),
        ("html", [true, false, false, false, true]),
        ("mp4", [true, true, true, true, false]),
        ("gif", [true, true, true, true, false]),
    ];
    for (key, expected) in cases {
        let fakes = fakes_with_recording();
        let mut window = window_with_recording(&fakes).await;
        window.change(window.kind_select(), key);
        let page = window.html();
        assert_eq!(
            shown_controls(&page),
            expected,
            "controls of {key} (Theme, FPS, Resolution, Optimize, Compress): {page}"
        );
    }
}

#[tokio::test]
async fn a_web_export_carries_the_theme_and_the_compress_box() {
    let fakes = fakes_with_recording()
        .dialog(Answer::Ready(Some(PathBuf::from(EXPORT_PICKED))))
        .export(
            Vec::new(),
            Answer::Ready(Ok(PathBuf::from("/tmp/out.html"))),
        );
    let mut window = window_with_recording(&fakes).await;
    window.change(window.kind_select(), "html");
    // The newest `change` listeners are the Theme menu and the Compress box.
    let listeners = window.newest_listeners_of("change");
    let [theme, compress] = listeners[..] else {
        panic!("expected the Theme menu and the Compress box: {listeners:?}");
    };
    window.change(theme, "dracula");
    window.change(compress, "true");
    window.click(window.export_button());
    window.settle().await;
    assert_eq!(
        fakes.export_requests(),
        vec![ExportRequest {
            output_path: PathBuf::from("/tmp/out.html"),
            target: ExportTarget::Web {
                theme: Theme::Dracula,
                compress: true,
            },
        }]
    );
}

#[tokio::test]
async fn a_video_export_carries_the_theme_the_resolution_and_the_optimize_box() {
    let fakes = fakes_with_recording()
        .dialog(Answer::Ready(Some(PathBuf::from(EXPORT_PICKED))))
        .export(Vec::new(), Answer::Ready(Ok(PathBuf::from("/tmp/out.gif"))));
    let mut window = window_with_recording(&fakes).await;
    window.change(window.kind_select(), "gif");
    // The newest `change` listeners are the Theme menu and the Optimize box,
    // and the newest `input` listeners are the FPS field and the Resolution field.
    let changes = window.newest_listeners_of("change");
    let [theme, optimize] = changes[..] else {
        panic!("expected the Theme menu and the Optimize box: {changes:?}");
    };
    let inputs = window.newest_listeners_of("input");
    let [_fps, resolution] = inputs[..] else {
        panic!("expected the FPS and Resolution fields: {inputs:?}");
    };
    window.change(theme, "monokai");
    window.input(resolution, "1280x720");
    window.change(optimize, "true");
    window.click(window.export_button());
    window.settle().await;
    assert_eq!(
        fakes.export_requests(),
        vec![ExportRequest {
            output_path: PathBuf::from("/tmp/out.gif"),
            target: ExportTarget::Video {
                theme: Theme::Monokai,
                fps: None,
                resolution: Some("1280x720".parse().expect("a valid resolution")),
                optimize_web: true,
            },
        }]
    );
}

#[tokio::test]
async fn open_recording_is_disabled_while_the_save_dialog_shows() {
    let (answer, pending) = Answer::later();
    let fakes = fakes_with_recording().dialog(pending);
    let mut window = window_with_recording(&fakes).await;
    window.click(window.export_button());
    window.settle().await;
    assert!(window.open_is_disabled(), "{}", window.html());
    window.click(window.open_button());
    window.settle().await;
    assert_eq!(fakes.picks(), 1);
    answer.send(None).expect("the dialog future waits");
    window.settle().await;
    assert!(!window.open_is_disabled(), "{}", window.html());
}

#[tokio::test]
async fn open_recording_is_disabled_while_an_export_runs() {
    let (finish, pending) = Answer::later();
    let fakes = fakes_with_recording()
        .dialog(Answer::Ready(Some(PathBuf::from(EXPORT_PICKED))))
        .export(Vec::new(), pending);
    let mut window = window_with_recording(&fakes).await;
    window.click(window.export_button());
    window.settle().await;
    assert!(window.open_is_disabled(), "{}", window.html());
    window.click(window.open_button());
    window.settle().await;
    assert_eq!(fakes.picks(), 1);
    finish
        .send(Ok(PathBuf::from(EXPORT_WRITTEN)))
        .expect("the export future waits");
    window.settle().await;
    assert!(!window.open_is_disabled(), "{}", window.html());
}

#[tokio::test]
async fn export_is_disabled_while_a_second_recording_opens() {
    let (answer, pending) = Answer::later();
    let fakes = fakes_with_recording().pick(pending);
    let mut window = window_with_recording(&fakes).await;
    window.click(window.open_button());
    window.settle().await;
    assert!(window.export_is_disabled(), "{}", window.html());
    window.click(window.export_button());
    window.settle().await;
    assert!(fakes.dialogs().is_empty());
    answer.send(None).expect("the pick future waits");
    window.settle().await;
    assert!(!window.export_is_disabled(), "{}", window.html());
}
