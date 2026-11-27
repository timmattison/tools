//! The Dioxus user interface of the NLE application.
//!
//! This module holds the window settings, the root component, and the four
//! regions of the window: the menu bar, the preview pane, the timeline, and
//! the properties panel, which holds the chapter panel. The open document lives
//! in the state of `App`: the `Editor` of its project, and the metadata of the
//! recording file that it came from.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;

use crate::editor::{Edit, Editor};
use crate::error::NleError;
use crate::filters::time_dilation::{self, Speed};
use crate::pipeline::FilterRegistry;
use crate::types::{AppliedFilter, EditProject, FilterRegion, Seconds};

use chapters::ChaptersPanel;

use crate::format::{file_name, format_duration, format_recorded};
use crate::types::{LoadedRecording, RecordingMetadata};

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
const SPEED_HINT: &str = "Times are in seconds, on the timeline after the speed changes above.";
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

/// The serial of the next document that `Document::open` builds.
static NEXT_DOCUMENT_SERIAL: AtomicU64 = AtomicU64::new(1);

/// The open document of the window: the editor of its project, and the
/// metadata of the recording file that it came from.
struct Document {
    editor: Editor,
    metadata: RecordingMetadata,
    /// A number that no other document of this process has. A component that
    /// keeps state about one document takes it as its key, thus each new
    /// document gets a new component.
    serial: u64,
}

impl Document {
    /// Builds the document of a loaded recording: a project of one clip that
    /// holds the whole recording, and an editor for it.
    fn open(loaded: LoadedRecording, filters: Arc<FilterRegistry>) -> Result<Self, NleError> {
        let project = EditProject::from_recording(Arc::new(loaded.recording))?;
        Ok(Self {
            editor: Editor::new(project, filters)?,
            metadata: loaded.metadata,
            serial: NEXT_DOCUMENT_SERIAL.fetch_add(1, Ordering::Relaxed),
        })
    }
}

/// The root component of the application.
#[component]
pub(crate) fn App() -> Element {
    let filters = use_hook(|| Arc::new(crate::filters::registry()));
    let mut loaded = use_signal(|| None::<Document>);
    let mut error = use_signal(|| None::<String>);

    rsx! {
        style { {STYLE} }
        div { class: "app-layout",
            MenuBar {
                on_loaded: move |recording| match Document::open(recording, filters.clone()) {
                    Ok(document) => {
                        loaded.set(Some(document));
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
            PreviewPane { loaded }
            Timeline { loaded }
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

/// The preview of the open recording.
#[component]
fn PreviewPane(loaded: ReadSignal<Option<Document>>) -> Element {
    let loaded = loaded.read();
    rsx! {
        div { class: "preview-pane",
            if let Some(open) = loaded.as_ref() {
                div { class: "placeholder",
                    "Terminal preview \u{2014} coming in #151"
                    br {}
                    "{open.metadata.width}x{open.metadata.height}"
                }
            } else {
                div { class: "placeholder", "Open a recording to preview it here" }
            }
        }
    }
}

/// The timeline of the open recording.
#[component]
fn Timeline(loaded: ReadSignal<Option<Document>>) -> Element {
    let loaded = loaded.read();
    rsx! {
        div { class: "timeline",
            if let Some(open) = loaded.as_ref() {
                h3 { "Timeline" }
                div { class: "placeholder",
                    "Timeline editor \u{2014} coming in #150"
                    br {}
                    "{summary(open)}"
                }
            } else {
                div { class: "placeholder", "No recording loaded" }
            }
        }
    }
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

/// The metadata of the open recording, the speed panel, and the chapter panel.
/// `on_result` gets the outcome of each speed change and each chapter change:
/// `Ok`, or the message of the banner.
#[component]
fn PropertiesPanel(
    loaded: Signal<Option<Document>>,
    on_result: EventHandler<Result<(), String>>,
) -> Element {
    let open = loaded
        .read()
        .as_ref()
        .map(|open| (open.metadata.clone(), open.serial));
    rsx! {
        div { class: "properties-panel",
            h3 { "Properties" }
            if let Some((metadata, serial)) = open {
                {metadata_rows(&metadata)}
                SpeedPanel { loaded, on_result }
                // A key counts only on the first node of a block. A keyed node
                // in its own block gets the keyed diff, thus each new document
                // gets a new panel, with no edit and no search.
                {rsx! { ChaptersPanel { key: "{serial}", loaded, on_result } }}
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
                    let outcome = change.and_then(|filter| {
                        edit_document(loaded, |project| Edit::AddFilter {
                            index: project.filters.len(),
                            filter,
                        })
                    });
                    report(on_result, outcome);
                },
                {APPLY_SPEED_LABEL}
            }
            for (index, text) in rows {
                div { class: "speed-row", key: "{index}",
                    span { "{text}" }
                    button {
                        onclick: move |_| {
                            let outcome = edit_document(loaded, |_| Edit::RemoveFilter { index });
                            report(on_result, outcome);
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

/// Sends the outcome of a speed change to `on_result`: `Ok` for a change that
/// applied, or the message of the banner for a change that did not.
fn report(on_result: EventHandler<Result<(), String>>, outcome: Result<(), String>) {
    on_result.call(outcome.map_err(|reason| format!("{SPEED_FAILED_PREFIX}: {reason}")));
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
            let region = match filter.region {
                FilterRegion::All => WHOLE_TIMELINE.to_owned(),
                // The seconds as the fields take them, so no two regions share a row text.
                FilterRegion::Range { start, end } => {
                    format!("{} s to {} s", start.get(), end.get())
                }
            };
            Some((index, format!("{region} at {}\u{d7}", speed.get())))
        })
        .collect()
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

mod chapters;

#[cfg(test)]
mod tests;
