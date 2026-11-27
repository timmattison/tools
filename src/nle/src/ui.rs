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
use crate::filters::redaction::{self, Find, Preset, Redaction, Replacement};
use crate::filters::time_dilation::{self, Speed};
use crate::pipeline::FilterRegistry;
use crate::types::{AppliedFilter, EditProject, FilterRegion, Seconds};

use crate::format::{file_name, format_duration, format_recorded};
use crate::types::{LoadedRecording, RecordingMetadata};

mod history;
use history::{
    keep_keys_in_field, key_forwarder, step_history, window_shortcut, HistoryButtons, HistoryStep,
};

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

/// The heading of the redaction panel.
const REDACTION_HEADING: &str = "Redaction";
/// The hint of the redaction panel.
const REDACTION_HINT: &str =
    "Each redaction covers the whole timeline. Escape sequences do not change.";
/// The label of the menu for what to find.
const FIND_LABEL: &str = "Find";
/// The value of the menu entry that finds an exact text.
const FIND_TEXT: &str = "text";
/// The text of the menu entry that finds an exact text.
const FIND_TEXT_LABEL: &str = "Text";
/// The value of the menu entry that finds a regular expression.
const FIND_REGEX: &str = "regex";
/// The text of the menu entry that finds a regular expression.
const FIND_REGEX_LABEL: &str = "Regular expression";
/// The label of the field for the text or the regular expression.
const PATTERN_LABEL: &str = "Pattern";
/// The label of the menu for what to put in place of a match.
const REPLACE_LABEL: &str = "Replace with";
/// The value of the menu entry for asterisks.
const REPLACE_ASTERISKS: &str = "asterisks";
/// The text of the menu entry for asterisks.
const REPLACE_ASTERISKS_LABEL: &str = "Asterisks";
/// The value of the menu entry for blocks.
const REPLACE_BLOCKS: &str = "blocks";
/// The text of the menu entry for blocks.
const REPLACE_BLOCKS_LABEL: &str = "Blocks";
/// The value of the menu entry for a replacement text.
const REPLACE_TEXT: &str = "text";
/// The text of the menu entry for a replacement text.
const REPLACE_TEXT_LABEL: &str = "Text";
/// The label of the field for the replacement text.
const REPLACEMENT_LABEL: &str = "Replacement text";
/// The label of the menu for the events that a redaction changes.
const EVENTS_LABEL: &str = "Events";
/// The value of the menu entry for the output events.
const EVENTS_OUTPUT: &str = "output";
/// The text of the menu entry for the output events.
const EVENTS_OUTPUT_LABEL: &str = "Output";
/// The value of the menu entry for the output and the input events.
const EVENTS_OUTPUT_AND_INPUT: &str = "output-and-input";
/// The text of the menu entry for the output and the input events.
const EVENTS_OUTPUT_AND_INPUT_LABEL: &str = "Output and input";
/// The text of a redaction row for the output and the input events.
const ROW_EVENTS_OUTPUT_AND_INPUT: &str = "output and input";
/// The start of the reason for a value of the Find menu that no entry has.
const UNKNOWN_FIND: &str = "unknown find";
/// The start of the reason for a value of the Replace with menu that no entry has.
const UNKNOWN_REPLACEMENT: &str = "unknown replacement";
/// The text of the button that applies the redaction.
const APPLY_REDACTION_LABEL: &str = "Apply Redaction";
/// The start of the message for a redaction that does not apply.
const REDACTION_FAILED_PREFIX: &str = "Failed to redact";

/// The accessible label of the playhead control.
const PLAYHEAD_LABEL: &str = "Playhead";

/// The id of the root element of the app. The script of `key_forwarder` sends
/// to it each key press that lands outside the app.
const APP_ROOT_ID: &str = "nle-app";

/// The title of the application window.
const WINDOW_TITLE: &str = "NLE - Terminal Recording Editor";
/// The width of the application window, in logical pixels.
const WINDOW_WIDTH: f64 = 1280.0;
/// The height of the application window, in logical pixels.
const WINDOW_HEIGHT: f64 = 800.0;

/// The window settings of the application: a resizable window with the title
/// and the size above. The head of the page holds the script of
/// `key_forwarder`, so the keys of the window work when no element has the
/// focus.
pub(crate) fn window_config() -> Config {
    Config::new()
        .with_window(
            WindowBuilder::new()
                .with_title(WINDOW_TITLE)
                .with_resizable(true)
                .with_inner_size(LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT)),
        )
        .with_custom_head(key_forwarder(APP_ROOT_ID))
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
    // A step that changed the project clears the banner, as an edit does.
    let take_step = use_callback(move |step: HistoryStep| match step_history(loaded, step) {
        Ok(true) => error.set(None),
        Ok(false) => {}
        Err(message) => error.set(Some(message)),
    });

    rsx! {
        style { {STYLE} }
        div {
            id: APP_ROOT_ID,
            class: "app-layout",
            // Each key press of the window comes here, except the keys of a
            // text field, which stay in the field.
            onkeydown: move |event| {
                if let Some(step) = window_shortcut(&event) {
                    take_step.call(step);
                }
            },
            MenuBar {
                loaded,
                on_step: take_step,
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

/// The menu bar: the Open Recording button, then the Undo button and the Redo
/// button of the editor of `loaded`. Each click of Undo or Redo sends its step
/// to `on_step`.
#[component]
fn MenuBar(
    loaded: ReadSignal<Option<Document>>,
    on_step: EventHandler<HistoryStep>,
    on_loaded: EventHandler<LoadedRecording>,
    on_error: EventHandler<String>,
) -> Element {
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
            HistoryButtons { loaded, on_step }
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

/// The metadata of the open recording, the speed panel, and the redaction
/// panel. `on_result` gets the outcome of each edit: `Ok`, or the message of
/// the banner.
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
                RedactionPanel { loaded, on_result }
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
                    onkeydown: keep_keys_in_field,
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
                    onkeydown: keep_keys_in_field,
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
                    onkeydown: keep_keys_in_field,
                }
            }
            button {
                onclick: move |_| {
                    let change = speed_change(&start.read(), &end.read(), &speed.read());
                    add_filter(loaded, on_result, SPEED_FAILED_PREFIX, change);
                },
                {APPLY_SPEED_LABEL}
            }
            {filter_rows(loaded, on_result, "speed-row", SPEED_FAILED_PREFIX, rows)}
            div { class: "metadata-item",
                span { class: "metadata-label", {EDITED_DURATION_LABEL} }
                span { class: "metadata-value", "{edited}" }
            }
        }
    }
}

/// The redaction panel: the fields for what to find and what to put in its
/// place, and the button that applies them. `on_result` gets the outcome of
/// each Apply Redaction and each Remove.
#[component]
fn RedactionPanel(
    loaded: Signal<Option<Document>>,
    on_result: EventHandler<Result<(), String>>,
) -> Element {
    let mut find = use_signal(|| FIND_TEXT.to_owned());
    let mut pattern = use_signal(String::new);
    let mut replace = use_signal(|| REPLACE_ASTERISKS.to_owned());
    let mut replacement = use_signal(String::new);
    let mut events = use_signal(|| EVENTS_OUTPUT.to_owned());
    // A keystroke in a field renders the panel again. The memo builds the rows,
    // and compiles the pattern of each one, only when the project changes.
    let rows = use_memo(move || redaction_rows(&loaded.read()));
    rsx! {
        div { class: "redaction-panel",
            h3 { {REDACTION_HEADING} }
            p { class: "redaction-hint", {REDACTION_HINT} }
            label { class: "redaction-field",
                {FIND_LABEL}
                select { value: "{find}", oninput: move |event| find.set(event.value()),
                    option { value: FIND_TEXT, {FIND_TEXT_LABEL} }
                    option { value: FIND_REGEX, {FIND_REGEX_LABEL} }
                    for preset in Preset::ALL {
                        option { key: "{preset.key()}", value: preset.key(), {preset.label()} }
                    }
                }
            }
            label { class: "redaction-field",
                {PATTERN_LABEL}
                input {
                    r#type: "text",
                    value: "{pattern}",
                    disabled: find() != FIND_TEXT && find() != FIND_REGEX,
                    oninput: move |event| pattern.set(event.value()),
                    onkeydown: keep_keys_in_field,
                }
            }
            label { class: "redaction-field",
                {REPLACE_LABEL}
                select {
                    value: "{replace}",
                    oninput: move |event| replace.set(event.value()),
                    option { value: REPLACE_ASTERISKS, {REPLACE_ASTERISKS_LABEL} }
                    option { value: REPLACE_BLOCKS, {REPLACE_BLOCKS_LABEL} }
                    option { value: REPLACE_TEXT, {REPLACE_TEXT_LABEL} }
                }
            }
            label { class: "redaction-field",
                {REPLACEMENT_LABEL}
                input {
                    r#type: "text",
                    value: "{replacement}",
                    disabled: replace() != REPLACE_TEXT,
                    oninput: move |event| replacement.set(event.value()),
                    onkeydown: keep_keys_in_field,
                }
            }
            label { class: "redaction-field",
                {EVENTS_LABEL}
                select {
                    value: "{events}",
                    oninput: move |event| events.set(event.value()),
                    option { value: EVENTS_OUTPUT, {EVENTS_OUTPUT_LABEL} }
                    option { value: EVENTS_OUTPUT_AND_INPUT, {EVENTS_OUTPUT_AND_INPUT_LABEL} }
                }
            }
            button {
                onclick: move |_| {
                    let change = redaction_change(
                        &find.read(),
                        &pattern.read(),
                        &replace.read(),
                        &replacement.read(),
                        &events.read(),
                    );
                    add_filter(loaded, on_result, REDACTION_FAILED_PREFIX, change);
                },
                {APPLY_REDACTION_LABEL}
            }
            {filter_rows(loaded, on_result, "redaction-row", REDACTION_FAILED_PREFIX, rows())}
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

/// Sends the outcome of an edit to `on_result`: `Ok` for an edit that applied,
/// or the message of the banner for an edit that did not. The message starts
/// with `prefix`.
fn report(on_result: EventHandler<Result<(), String>>, prefix: &str, outcome: Result<(), String>) {
    on_result.call(outcome.map_err(|reason| format!("{prefix}: {reason}")));
}

/// Adds the filter of `change` at the end of the filter list of the open
/// document, and sends the outcome to `on_result` with `prefix`. A `change`
/// that is an error adds nothing, and its text goes to the banner.
fn add_filter(
    loaded: Signal<Option<Document>>,
    on_result: EventHandler<Result<(), String>>,
    prefix: &str,
    change: Result<AppliedFilter, String>,
) {
    let outcome = change.and_then(|filter| {
        edit_document(loaded, |project| Edit::AddFilter {
            index: project.filters.len(),
            filter,
        })
    });
    report(on_result, prefix, outcome);
}

/// The rows of the filters of one panel. Each row shows its text and a
/// Remove button that removes the filter at its index in the project.
/// `class` is the class of each row, and `prefix` starts the banner text of
/// a remove that fails.
fn filter_rows(
    loaded: Signal<Option<Document>>,
    on_result: EventHandler<Result<(), String>>,
    class: &'static str,
    prefix: &'static str,
    rows: Vec<(usize, String)>,
) -> Element {
    rsx! {
        for (index, text) in rows {
            div { class: "{class}", key: "{index}",
                span { "{text}" }
                button {
                    onclick: move |_| {
                        let outcome = edit_document(loaded, |_| Edit::RemoveFilter { index });
                        report(on_result, prefix, outcome);
                    },
                    {REMOVE_LABEL}
                }
            }
        }
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

/// One row for each redaction filter of the project: the index of the filter
/// in the project, and the text of its row. Filters of other kinds have no
/// row, but they keep their place in the index.
fn redaction_rows(document: &Option<Document>) -> Vec<(usize, String)> {
    let Some(open) = document else {
        return Vec::new();
    };
    open.editor
        .project()
        .filters
        .iter()
        .enumerate()
        .filter_map(|(index, filter)| {
            let found = redaction::redaction_of(filter)?;
            let find = match found.find() {
                Find::Text(text) => format!("text \u{201c}{text}\u{201d}"),
                Find::Regex(pattern) => format!("regex /{pattern}/"),
                Find::Preset(preset) => preset.label().to_owned(),
            };
            let replacement = match found.replacement() {
                Replacement::Asterisks => REPLACE_ASTERISKS.to_owned(),
                Replacement::Blocks => REPLACE_BLOCKS.to_owned(),
                Replacement::Text(text) => format!("\u{201c}{text}\u{201d}"),
            };
            let events = if found.includes_input() {
                ROW_EVENTS_OUTPUT_AND_INPUT
            } else {
                EVENTS_OUTPUT
            };
            let region = match filter.region {
                FilterRegion::All => String::new(),
                FilterRegion::Range { start, end } => {
                    format!(", {} s to {} s", start.get(), end.get())
                }
            };
            Some((
                index,
                format!("{find} \u{2192} {replacement} ({events}){region}"),
            ))
        })
        .collect()
}

/// Reads the five fields of the redaction panel. `find` is the value of the
/// Find menu, `replace` is the value of the Replace with menu, and `events`
/// is the value of the Events menu. The pattern and the replacement text keep
/// their spaces, because a space can matter. The answer is the filter to add,
/// or the reason why the fields do not make one.
fn redaction_change(
    find: &str,
    pattern: &str,
    replace: &str,
    replacement: &str,
    events: &str,
) -> Result<AppliedFilter, String> {
    let find = match find {
        FIND_TEXT => Find::Text(pattern.to_owned()),
        FIND_REGEX => Find::Regex(pattern.to_owned()),
        key => Find::Preset(Preset::from_key(key).ok_or_else(|| format!("{UNKNOWN_FIND}: {key}"))?),
    };
    let replacement = match replace {
        REPLACE_ASTERISKS => Replacement::Asterisks,
        REPLACE_BLOCKS => Replacement::Blocks,
        REPLACE_TEXT => Replacement::Text(replacement.to_owned()),
        other => return Err(format!("{UNKNOWN_REPLACEMENT}: {other}")),
    };
    let redaction = Redaction::new(find, replacement, events == EVENTS_OUTPUT_AND_INPUT)
        .map_err(|error| error.to_string())?;
    Ok(redaction::applied(FilterRegion::All, &redaction))
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
