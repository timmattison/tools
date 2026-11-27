//! The Dioxus user interface of the NLE application.
//!
//! This module holds the window settings, the root component, and the four
//! regions of the window: the menu bar, the preview pane, the timeline, and
//! the properties panel. The open document lives in the state of `App`: the
//! `Editor` of its project, and the metadata of the recording file that it
//! came from.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;

use crate::editor::{Edit, Editor};
use crate::error::NleError;
use crate::filters::idle_compression::{self, IdleCompression};
use crate::filters::time_dilation::{self, Speed};
use crate::pipeline::{FilterRegistry, Mark};
use crate::types::{AppliedFilter, EditProject, FilterRegion, Seconds};

use crate::format::{file_name, format_duration, format_recorded};
use crate::types::{LoadedRecording, RecordingMetadata};

mod preview;
use preview::{end_of, render_of, PreviewPane, RenderedState};

/// The style sheet of the window. The `style` element at the top of `App` holds
/// it, so the webview needs no extra file.
const STYLE: &str = include_str!("../assets/main.css");
/// The text for a recording with no title.
const UNTITLED: &str = "(untitled)";

/// The name of the file filter for recordings in the file dialog.
const RECORDINGS_FILTER_NAME: &str = "Recordings";
/// The extensions that the recordings filter accepts.
const RECORDINGS_FILTER_EXTENSIONS: &[&str] = &["json", "gz"];
/// The name of the file filter that accepts every file.
const ALL_FILES_FILTER_NAME: &str = "All Files";
/// The extension list that accepts every file.
const ALL_FILES_FILTER_EXTENSIONS: &[&str] = &["*"];
/// The start of the message for a recording that does not open.
const OPEN_FAILED_PREFIX: &str = "Failed to open recording";

/// The heading of the speed panel.
const SPEED_HEADING: &str = "Speed";
/// The hint of the speed panel.
const SPEED_HINT: &str =
    "Times are in seconds, on the timeline after each earlier speed change and idle compression.";
/// The label of the field for the start of a region.
const START_LABEL: &str = "Start (s)";
/// The label of the field for the end of a region.
const END_LABEL: &str = "End (s)";
/// The label of the field for the speed.
const SPEED_LABEL: &str = "Speed";
/// The text of the button that applies the speed.
const APPLY_SPEED_LABEL: &str = "Apply Speed";
/// The label of the row for the duration after the edits.
const EDITED_DURATION_LABEL: &str = "Edited duration";

/// The text of the button that removes a speed change.
const REMOVE_LABEL: &str = "Remove";
/// The text for a speed change that covers the whole timeline.
const WHOLE_TIMELINE: &str = "the whole timeline";
/// The start of the message for a speed change that does not apply.
const SPEED_FAILED_PREFIX: &str = "Failed to change the speed";
/// The reason for a start that is not a number.
const START_NOT_A_NUMBER: &str = "the start is not a number";
/// The reason for an end that is not a number.
const END_NOT_A_NUMBER: &str = "the end is not a number";
/// The reason for a speed that is not a number.
const SPEED_NOT_A_NUMBER: &str = "the speed is not a number";
/// The reason for an end that is not above the start.
const END_BEFORE_START: &str = "the end must come after the start";

/// The heading of the idle panel.
const IDLE_HEADING: &str = "Idle Time";
/// The hint of the idle panel.
const IDLE_HINT: &str = "Compresses each stretch of the whole timeline with no key and no change on the screen. A length of 0 removes it. Ignore the rows of a status bar that shows a clock.";
/// The label of the field for the shortest idle stretch to compress.
const THRESHOLD_LABEL: &str = "Idle after (s)";
/// The label of the field for the length of a compressed stretch.
const COMPRESS_TO_LABEL: &str = "Compress to (s)";
/// The label of the field for the rows to ignore at the top of the screen.
const IGNORE_TOP_LABEL: &str = "Ignore top rows";
/// The label of the field for the rows to ignore at the bottom of the screen.
const IGNORE_BOTTOM_LABEL: &str = "Ignore bottom rows";
/// The text of the button that compresses the idle time.
const APPLY_IDLE_LABEL: &str = "Compress Idle Time";
/// The first value of the field for the shortest idle stretch.
const DEFAULT_THRESHOLD: &str = "3";
/// The first value of the field for the length of a compressed stretch.
const DEFAULT_COMPRESS_TO: &str = "1";
/// The first value of the two fields for the rows to ignore.
const DEFAULT_IGNORED_ROWS: &str = "0";
/// The start of the message for an idle compression that does not apply.
const IDLE_FAILED_PREFIX: &str = "Failed to compress idle time";
/// The reason for a threshold that is not a number.
const THRESHOLD_NOT_A_NUMBER: &str = "the idle threshold is not a number";
/// The reason for a compressed length that is not a number.
const COMPRESS_TO_NOT_A_NUMBER: &str = "the compressed length is not a number";
/// The reason for a count of top rows that is not a count.
const TOP_ROWS_NOT_A_COUNT: &str = "the top rows are not a whole number from 0 to 65535";
/// The reason for a count of bottom rows that is not a count.
const BOTTOM_ROWS_NOT_A_COUNT: &str = "the bottom rows are not a whole number from 0 to 65535";

/// The name of the strip of the marks, for a screen reader.
const MARK_STRIP_LABEL: &str = "Marks on the edited recording";
/// The count of the marks when there is one mark.
const ONE_MARK: &str = "1 mark on the edited recording";
/// The text after the count of the marks when there is more than one mark.
const MARKS_SUFFIX: &str = "marks on the edited recording";

/// The accessible label of the playhead control.
const PLAYHEAD_LABEL: &str = "Playhead";

/// The title of the application window.
const WINDOW_TITLE: &str = "NLE - Terminal Recording Editor";
/// The width of the application window, in logical pixels.
const WINDOW_WIDTH: f64 = 1280.0;
/// The height of the application window, in logical pixels.
const WINDOW_HEIGHT: f64 = 800.0;

/// The window settings of the application: a resizable window with the title
/// and the size above.
pub(crate) fn window_config() -> Config {
    Config::new().with_window(
        WindowBuilder::new()
            .with_title(WINDOW_TITLE)
            .with_resizable(true)
            .with_inner_size(LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT)),
    )
}

/// A future that the interface runs on its own thread. It needs no `Send`.
type LocalFuture<T> = Pin<Box<dyn Future<Output = T>>>;

/// The two things that the interface asks of the outside world: the file
/// dialog and the load of a recording.
///
/// The application gets the real dialog and the real command from
/// `Services::desktop`. A test gives fakes to `Services::new`, so it needs
/// no window and no file. The value is `Send + Sync` because the launcher
/// asks for it. The futures that it returns are not `Send`.
#[derive(Clone)]
pub(crate) struct Services {
    pick_recording: Arc<dyn Fn() -> LocalFuture<Option<PathBuf>> + Send + Sync>,
    load_recording:
        Arc<dyn Fn(PathBuf) -> LocalFuture<Result<LoadedRecording, NleError>> + Send + Sync>,
}

impl Services {
    /// Builds the services from a dialog and a load function.
    pub(crate) fn new(
        pick_recording: impl Fn() -> LocalFuture<Option<PathBuf>> + Send + Sync + 'static,
        load_recording: impl Fn(PathBuf) -> LocalFuture<Result<LoadedRecording, NleError>>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        Self {
            pick_recording: Arc::new(pick_recording),
            load_recording: Arc::new(load_recording),
        }
    }

    /// Builds the services of the application: the native file dialog and
    /// the `load_recording` command.
    ///
    /// The command takes a `String`, so a path that is not valid UTF-8 loses
    /// its invalid bytes.
    pub(crate) fn desktop() -> Self {
        Self::new(
            || {
                Box::pin(async {
                    rfd::AsyncFileDialog::new()
                        .add_filter(RECORDINGS_FILTER_NAME, RECORDINGS_FILTER_EXTENSIONS)
                        .add_filter(ALL_FILES_FILTER_NAME, ALL_FILES_FILTER_EXTENSIONS)
                        .pick_file()
                        .await
                        .map(|file| file.path().to_path_buf())
                })
            },
            |path| {
                Box::pin(async move {
                    crate::commands::load_recording(path.to_string_lossy().into_owned()).await
                })
            },
        )
    }

    /// Shows the file dialog. The answer is `None` when the user cancels.
    fn pick_recording(&self) -> LocalFuture<Option<PathBuf>> {
        (self.pick_recording)()
    }

    /// Loads the recording at `path`.
    fn load_recording(&self, path: PathBuf) -> LocalFuture<Result<LoadedRecording, NleError>> {
        (self.load_recording)(path)
    }
}

/// The open document of the window: the editor of its project, and the
/// metadata of the recording file that it came from.
struct Document {
    editor: Editor,
    metadata: RecordingMetadata,
}

impl Document {
    /// Builds the document of a loaded recording: a project of one clip that
    /// holds the whole recording, and an editor for it.
    fn open(loaded: LoadedRecording, filters: Arc<FilterRegistry>) -> Result<Self, NleError> {
        let project = EditProject::from_recording(Arc::new(loaded.recording))?;
        Ok(Self {
            editor: Editor::new(project, filters)?,
            metadata: loaded.metadata,
        })
    }
}

/// The root component of the application.
#[component]
pub(crate) fn App() -> Element {
    let filters = use_hook(|| Arc::new(crate::filters::registry()));
    let mut loaded = use_signal(|| None::<Document>);
    let mut error = use_signal(|| None::<String>);
    let mut playhead = use_signal(|| Seconds::ZERO);
    let mut playing = use_signal(|| false);
    let rendered = use_memo(move || render_of(&loaded.read()));

    rsx! {
        style { {STYLE} }
        div { class: "app-layout",
            MenuBar {
                on_loaded: move |recording| match Document::open(recording, filters.clone()) {
                    Ok(document) => {
                        loaded.set(Some(document));
                        // A new recording starts at time zero.
                        playhead.set(Seconds::ZERO);
                        playing.set(false);
                        error.set(None);
                    }
                    Err(failure) => error.set(Some(format!("{OPEN_FAILED_PREFIX}: {failure}"))),
                },
                on_error: move |message| error.set(Some(message)),
            }
            if let Some(message) = error() {
                div { class: "error-banner", role: "alert",
                    "{message}"
                    button { class: "error-dismiss", onclick: move |_| error.set(None), "Dismiss" }
                }
            }
            PreviewPane { rendered, playhead, playing }
            Timeline { loaded, rendered, playhead }
            PropertiesPanel {
                loaded,
                on_result: move |result: Result<(), String>| error.set(result.err()),
            }
        }
    }
}

/// The menu bar with the Open Recording button.
#[component]
fn MenuBar(on_loaded: EventHandler<LoadedRecording>, on_error: EventHandler<String>) -> Element {
    let services = use_context::<Services>();
    // True from the click until the dialog and the load end.
    let mut opening = use_signal(|| false);
    rsx! {
        div { class: "menu-bar",
            button {
                disabled: opening(),
                onclick: move |_| {
                    // A click that arrives during a dialog or a load starts nothing.
                    if opening() {
                        return;
                    }
                    opening.set(true);
                    let services = services.clone();
                    spawn(async move {
                        if let Some(path) = services.pick_recording().await {
                            match services.load_recording(path).await {
                                Ok(recording) => on_loaded.call(recording),
                                Err(error) => on_error.call(format!("{OPEN_FAILED_PREFIX}: {error}")),
                            }
                        }
                        opening.set(false);
                    });
                },
                "Open Recording"
            }
        }
    }
}

/// The timeline of the open recording.
///
/// The playhead control is a plain range input. Issue #150 replaces it with
/// the real timeline and its playhead.
#[component]
fn Timeline(
    loaded: ReadSignal<Option<Document>>,
    rendered: ReadSignal<RenderedState>,
    mut playhead: Signal<Seconds>,
) -> Element {
    let loaded = loaded.read();
    let end = end_of(&rendered.read());
    rsx! {
        div { class: "timeline",
            if let Some(open) = loaded.as_ref() {
                h3 { "Timeline" }
                input {
                    class: "playhead",
                    r#type: "range",
                    aria_label: PLAYHEAD_LABEL,
                    min: "0",
                    max: "{end.get()}",
                    step: "any",
                    value: "{playhead().get()}",
                    oninput: move |event| {
                        if let Some(at) = playhead_from(&event.value(), end) {
                            playhead.set(at);
                        }
                    },
                }
                div { class: "placeholder",
                    "Timeline editor \u{2014} coming in #150"
                    br {}
                    "{summary(open)}"
                }
                {mark_strip(open)}
            } else {
                div { class: "placeholder", "No recording loaded" }
            }
        }
    }
}

/// The playhead that the text of the control states, held at or below `end`.
/// The answer is `None` for text that is not a valid time.
fn playhead_from(text: &str, end: Seconds) -> Option<Seconds> {
    let value: f64 = text.trim().parse().ok()?;
    let at = Seconds::new(value).ok()?;
    Some(if at > end { end } else { at })
}

/// The event count and the duration of the project of the document.
fn summary(open: &Document) -> String {
    let project = open.editor.project();
    format!(
        "{} events | {}",
        project.event_count(),
        format_duration(project.duration())
    )
}

/// The strip of the marks of the render of the document: a bar for the edited
/// recording, with a mark at the place of each mark of the filters, and the
/// count of the marks. The text of a mark shows when the pointer is on it.
/// There is no strip when the render has no mark or fails. The properties
/// panel shows the error of a render that fails.
fn mark_strip(open: &Document) -> Element {
    let (Ok(duration), Ok(marks)) = (
        open.editor.rendered_duration(),
        open.editor.rendered_marks(),
    ) else {
        return rsx! {};
    };
    if marks.is_empty() {
        return rsx! {};
    }
    let count = match marks.len() {
        1 => ONE_MARK.to_owned(),
        count => format!("{count} {MARKS_SUFFIX}"),
    };
    let rows: Vec<(String, String)> = marks
        .iter()
        .map(|mark| (mark_place(mark, duration), mark_text(mark)))
        .collect();
    rsx! {
        div { class: "mark-strip", aria_label: MARK_STRIP_LABEL,
            for (index, (place, text)) in rows.into_iter().enumerate() {
                span {
                    key: "{index}",
                    class: "timeline-mark",
                    style: "{place}",
                    title: "{text}",
                    aria_label: "{text}",
                }
            }
        }
        p { class: "mark-count", "{count}" }
    }
}

/// The style that puts `mark` on a strip for a recording of `duration`
/// seconds: its time as a percentage of the duration, from 0 to 100.
fn mark_place(mark: &Mark, duration: f64) -> String {
    let percent = if duration > 0.0 {
        (mark.time / duration * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    format!("left: {percent:.2}%")
}

/// The text of `mark`: its label and its time.
fn mark_text(mark: &Mark) -> String {
    format!("{} at {}", mark.label, format_duration(mark.time))
}

/// The metadata of the open recording, the speed panel, and the idle panel.
/// `on_result` gets the outcome of each speed change and each idle
/// compression: `Ok`, or the message of the banner.
#[component]
fn PropertiesPanel(
    loaded: Signal<Option<Document>>,
    on_result: EventHandler<Result<(), String>>,
) -> Element {
    let metadata = loaded.read().as_ref().map(|open| open.metadata.clone());
    rsx! {
        div { class: "properties-panel",
            h3 { "Properties" }
            if let Some(metadata) = metadata {
                {metadata_rows(&metadata)}
                SpeedPanel { loaded, on_result }
                IdlePanel { loaded, on_result }
            } else {
                div { class: "placeholder", "No recording loaded" }
            }
        }
    }
}

/// The speed panel: three fields for a region and a speed, the button that
/// applies them, and the time dilation filters of the project. `on_result`
/// gets the outcome of each Apply Speed and each Remove.
#[component]
fn SpeedPanel(
    loaded: Signal<Option<Document>>,
    on_result: EventHandler<Result<(), String>>,
) -> Element {
    let mut start = use_signal(String::new);
    let mut end = use_signal(String::new);
    let mut speed = use_signal(String::new);
    let edited = use_memo(move || edited_duration(&loaded.read()));
    let rows = speed_rows(&loaded.read());
    rsx! {
        div { class: "speed-panel",
            h3 { {SPEED_HEADING} }
            p { class: "speed-hint", {SPEED_HINT} }
            label { class: "speed-field",
                {START_LABEL}
                input {
                    r#type: "number",
                    step: "any",
                    min: "0",
                    value: "{start}",
                    oninput: move |event| start.set(event.value()),
                }
            }
            label { class: "speed-field",
                {END_LABEL}
                input {
                    r#type: "number",
                    step: "any",
                    min: "0",
                    value: "{end}",
                    oninput: move |event| end.set(event.value()),
                }
            }
            label { class: "speed-field",
                {SPEED_LABEL}
                input {
                    r#type: "number",
                    step: "any",
                    min: "0",
                    value: "{speed}",
                    oninput: move |event| speed.set(event.value()),
                }
            }
            button {
                onclick: move |_| {
                    let change = speed_change(&start.read(), &end.read(), &speed.read());
                    let outcome = change.and_then(|filter| append_filter(loaded, filter));
                    report(on_result, SPEED_FAILED_PREFIX, outcome);
                },
                {APPLY_SPEED_LABEL}
            }
            for (index, text) in rows {
                div { class: "speed-row", key: "{index}",
                    span { "{text}" }
                    button {
                        onclick: move |_| {
                            let outcome = edit_document(loaded, |_| Edit::RemoveFilter { index });
                            report(on_result, SPEED_FAILED_PREFIX, outcome);
                        },
                        {REMOVE_LABEL}
                    }
                }
            }
            div { class: "metadata-item",
                span { class: "metadata-label", {EDITED_DURATION_LABEL} }
                span { class: "metadata-value", "{edited}" }
            }
        }
    }
}

/// The idle panel: four fields for the settings of an idle compression of the
/// whole timeline, the button that applies them, and the idle compressions of
/// the project. The screen size of the settings is the size of the open
/// recording. `on_result` gets the outcome of each Compress Idle Time and each
/// Remove.
#[component]
fn IdlePanel(
    loaded: Signal<Option<Document>>,
    on_result: EventHandler<Result<(), String>>,
) -> Element {
    let mut threshold = use_signal(|| DEFAULT_THRESHOLD.to_owned());
    let mut compress_to = use_signal(|| DEFAULT_COMPRESS_TO.to_owned());
    let mut top = use_signal(|| DEFAULT_IGNORED_ROWS.to_owned());
    let mut bottom = use_signal(|| DEFAULT_IGNORED_ROWS.to_owned());
    let rows = idle_rows(&loaded.read());
    rsx! {
        div { class: "idle-panel",
            h3 { {IDLE_HEADING} }
            p { class: "speed-hint", {IDLE_HINT} }
            label { class: "speed-field",
                {THRESHOLD_LABEL}
                input {
                    r#type: "number",
                    step: "any",
                    min: "0",
                    value: "{threshold}",
                    oninput: move |event| threshold.set(event.value()),
                }
            }
            label { class: "speed-field",
                {COMPRESS_TO_LABEL}
                input {
                    r#type: "number",
                    step: "any",
                    min: "0",
                    value: "{compress_to}",
                    oninput: move |event| compress_to.set(event.value()),
                }
            }
            label { class: "speed-field",
                {IGNORE_TOP_LABEL}
                input {
                    r#type: "number",
                    step: "1",
                    min: "0",
                    value: "{top}",
                    oninput: move |event| top.set(event.value()),
                }
            }
            label { class: "speed-field",
                {IGNORE_BOTTOM_LABEL}
                input {
                    r#type: "number",
                    step: "1",
                    min: "0",
                    value: "{bottom}",
                    oninput: move |event| bottom.set(event.value()),
                }
            }
            button {
                onclick: move |_| {
                    // The read ends here, before the edit borrows the document.
                    let screen = loaded
                        .read()
                        .as_ref()
                        .map(|open| (open.metadata.width, open.metadata.height));
                    let Some((width, height)) = screen else {
                        return;
                    };
                    let change = idle_change(
                        [&threshold.read(), &compress_to.read(), &top.read(), &bottom.read()],
                        width,
                        height,
                    );
                    let outcome = change.and_then(|filter| append_filter(loaded, filter));
                    report(on_result, IDLE_FAILED_PREFIX, outcome);
                },
                {APPLY_IDLE_LABEL}
            }
            for (index, text) in rows {
                div { class: "speed-row", key: "{index}",
                    span { "{text}" }
                    button {
                        onclick: move |_| {
                            let outcome = edit_document(loaded, |_| Edit::RemoveFilter { index });
                            report(on_result, IDLE_FAILED_PREFIX, outcome);
                        },
                        {REMOVE_LABEL}
                    }
                }
            }
        }
    }
}

/// Adds `filter` at the end of the filters of the open document, by the rules
/// of `edit_document`.
fn append_filter(loaded: Signal<Option<Document>>, filter: AppliedFilter) -> Result<(), String> {
    edit_document(loaded, |project| Edit::AddFilter {
        index: project.filters.len(),
        filter,
    })
}

/// Applies the edit that `make` builds from the project to the editor of the
/// open document. The answer is the text of the error of a refused edit. An
/// edit after which the project does not render is refused too. With no open
/// document, nothing changes and the answer is `Ok`.
fn edit_document(
    mut loaded: Signal<Option<Document>>,
    make: impl FnOnce(&EditProject) -> Edit,
) -> Result<(), String> {
    // One write borrow holds the whole edit. No read borrow lives across it.
    loaded.with_mut(|document| {
        document.as_mut().map_or(Ok(()), |open| {
            let edit = make(open.editor.project());
            open.editor
                .apply_renderable(edit)
                .map_err(|failure| failure.to_string())
        })
    })
}

/// Sends the outcome of a change to `on_result`: `Ok` for a change that
/// applied, or the message of the banner, which starts with `prefix`, for a
/// change that did not.
fn report(on_result: EventHandler<Result<(), String>>, prefix: &str, outcome: Result<(), String>) {
    on_result.call(outcome.map_err(|reason| format!("{prefix}: {reason}")));
}

/// The text of the region of a filter in its row.
fn region_text(region: &FilterRegion) -> String {
    match region {
        FilterRegion::All => WHOLE_TIMELINE.to_owned(),
        // The seconds as the fields take them, so no two regions share a row text.
        FilterRegion::Range { start, end } => format!("{} s to {} s", start.get(), end.get()),
    }
}

/// One row for each time dilation filter of the project: the index of the
/// filter in the project, and the text of its row. Filters of other kinds
/// have no row, but they keep their place in the index.
fn speed_rows(document: &Option<Document>) -> Vec<(usize, String)> {
    let Some(open) = document else {
        return Vec::new();
    };
    open.editor
        .project()
        .filters
        .iter()
        .enumerate()
        .filter_map(|(index, filter)| {
            let speed = time_dilation::speed_of(filter)?;
            let region = region_text(&filter.region);
            Some((index, format!("{region} at {}\u{d7}", speed.get())))
        })
        .collect()
}

/// One row for each idle compression of the project: the index of the filter
/// in the project, and the text of its row. Filters of other kinds have no
/// row, but they keep their place in the index.
fn idle_rows(document: &Option<Document>) -> Vec<(usize, String)> {
    let Some(open) = document else {
        return Vec::new();
    };
    open.editor
        .project()
        .filters
        .iter()
        .enumerate()
        .filter_map(|(index, filter)| {
            let settings = idle_compression::settings_of(filter)?;
            let mut text = format!(
                "{}, idle of {} s or more to {} s",
                region_text(&filter.region),
                settings.threshold(),
                settings.compress_to()
            );
            let (top, bottom) = (settings.ignore_top_rows(), settings.ignore_bottom_rows());
            if top > 0 || bottom > 0 {
                text.push_str(&format!(", ignores {top} top and {bottom} bottom rows"));
            }
            Some((index, text))
        })
        .collect()
}

/// Reads the four fields of the idle panel: the threshold and the compressed
/// length in seconds, and the rows to ignore at the top and at the bottom.
/// The screen has `width` columns and `height` rows. The answer is the idle
/// compression of the whole timeline to add, or the reason why the fields do
/// not make one.
fn idle_change(fields: [&str; 4], width: u16, height: u16) -> Result<AppliedFilter, String> {
    let [threshold, compress_to, top, bottom] = fields;
    let threshold: f64 = threshold
        .trim()
        .parse()
        .map_err(|_| THRESHOLD_NOT_A_NUMBER)?;
    let compress_to: f64 = compress_to
        .trim()
        .parse()
        .map_err(|_| COMPRESS_TO_NOT_A_NUMBER)?;
    let top: u16 = top.trim().parse().map_err(|_| TOP_ROWS_NOT_A_COUNT)?;
    let bottom: u16 = bottom.trim().parse().map_err(|_| BOTTOM_ROWS_NOT_A_COUNT)?;
    let settings = IdleCompression::new(threshold, compress_to, width, height)
        .and_then(|settings| settings.ignoring_rows(top, bottom))
        .map_err(|error| error.to_string())?;
    Ok(idle_compression::applied(FilterRegion::All, settings))
}

/// Reads the three fields of the speed panel: the start and the end of a
/// region in seconds, and a speed. The answer is the filter to add, or the
/// reason why the fields do not make one.
fn speed_change(start: &str, end: &str, speed: &str) -> Result<AppliedFilter, String> {
    let start: f64 = start.trim().parse().map_err(|_| START_NOT_A_NUMBER)?;
    let end: f64 = end.trim().parse().map_err(|_| END_NOT_A_NUMBER)?;
    let speed: f64 = speed.trim().parse().map_err(|_| SPEED_NOT_A_NUMBER)?;
    let start = Seconds::new(start).map_err(|error| error.to_string())?;
    let end = Seconds::new(end).map_err(|error| error.to_string())?;
    if f64::from(end) <= f64::from(start) {
        return Err(END_BEFORE_START.to_owned());
    }
    let speed = Speed::new(speed).map_err(|error| error.to_string())?;
    Ok(time_dilation::applied(
        FilterRegion::Range { start, end },
        speed,
    ))
}

/// The duration of the rendered project of the document as text, or the text
/// of the error that the render gives.
fn edited_duration(document: &Option<Document>) -> String {
    match document {
        None => String::new(),
        Some(open) => match open.editor.rendered_duration() {
            Ok(duration) => format_duration(duration),
            Err(error) => error.to_string(),
        },
    }
}

/// The seven rows of the properties panel for one recording.
fn metadata_rows(metadata: &RecordingMetadata) -> Element {
    let title = if metadata.title.is_empty() {
        UNTITLED
    } else {
        metadata.title.as_str()
    };
    let rows = [
        ("Title", title.to_owned(), None),
        ("Command", metadata.command.clone(), None),
        ("Duration", format_duration(metadata.duration), None),
        ("Events", metadata.event_count.to_string(), None),
        (
            "Size",
            format!("{}x{}", metadata.width, metadata.height),
            None,
        ),
        ("Recorded", format_recorded(metadata.timestamp), None),
        (
            "File",
            file_name(&metadata.file_path).to_owned(),
            Some(metadata.file_path.clone()),
        ),
    ];
    rsx! {
        for (label, value, hover) in rows {
            div { class: "metadata-item", key: "{label}",
                span { class: "metadata-label", "{label}" }
                span { class: "metadata-value", title: hover, "{value}" }
            }
        }
    }
}

#[cfg(test)]
mod tests;
