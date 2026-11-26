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
