//! The export bar of the menu bar: the kinds of file that it writes, the
//! settings that the user sets, and the text of the progress.
//!
//! The types here are plain values, so the tests of `ui/export/tests.rs`
//! need no window.

use std::ffi::OsStr;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use dioxus::prelude::*;

use super::history::keep_keys_in_field;
use super::{Document, Services};
use crate::error::NleError;
use crate::export::{
    output_path_text, ExportProgress, ExportRequest, ExportTarget, Resolution, Theme,
};
use crate::format::file_name;

/// The stem of a file name that has no stem left after the endings go.
const DEFAULT_STEM: &str = "recording";
/// The text between the stem and the ending of the name that the dialog offers.
const EDITED_MARKER: &str = "-edited";
/// Each ending that some kind uses, in the order that a strip tries them.
const KIND_ENDINGS: [&str; 5] = [".json", ".gz", ".html", ".mp4", ".gif"];

/// A kind of file that the export bar writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ExportKind {
    /// A `beta` recording, as plain JSON.
    #[default]
    Recording,
    /// A `beta` recording, compressed with gzip.
    CompressedRecording,
    /// A web page with a player.
    Web,
    /// An MP4 video.
    Mp4,
    /// A GIF animation.
    Gif,
}

/// What the save dialog of an export shows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SaveDialog {
    /// The name that the dialog offers first.
    pub(crate) file_name: String,
    /// The folder that the dialog opens in. `None` leaves it to the system.
    pub(crate) directory: Option<PathBuf>,
    /// The name of the file filter.
    pub(crate) filter_name: &'static str,
    /// The extensions that the filter accepts, without a dot.
    pub(crate) extensions: &'static [&'static str],
}

impl ExportKind {
    /// Each kind, in the order of the menu.
    pub(crate) const ALL: [ExportKind; 5] = [
        Self::Recording,
        Self::CompressedRecording,
        Self::Web,
        Self::Mp4,
        Self::Gif,
    ];

    /// The `value` of the menu entry of this kind.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Recording => "json",
            Self::CompressedRecording => "json-gz",
            Self::Web => "html",
            Self::Mp4 => "mp4",
            Self::Gif => "gif",
        }
    }

    /// The kind with the menu `value` `key`, or `None`.
    pub(crate) fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    /// The text of the menu entry of this kind.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Recording => "Beta recording (.json)",
            Self::CompressedRecording => "Beta recording, gzip (.json.gz)",
            Self::Web => "Web page (.html)",
            Self::Mp4 => "MP4 video (.mp4)",
            Self::Gif => "GIF animation (.gif)",
        }
    }

    /// The name and the extensions of the file filter of the save dialog.
    fn filter(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Self::Recording => ("Beta recording", &["json"]),
            Self::CompressedRecording => ("Gzip Beta recording", &["gz"]),
            Self::Web => ("Web page", &["html"]),
            Self::Mp4 => ("MP4 video", &["mp4"]),
            Self::Gif => ("GIF animation", &["gif"]),
        }
    }

    /// What the save dialog shows for this kind, when the open recording
    /// comes from `source_path`. The name is the name of the source, with
    /// `-edited` and the ending of this kind. The folder is the folder of the
    /// source.
    pub(crate) fn save_dialog(self, source_path: &str) -> SaveDialog {
        let stem = stem_or_default(file_name(source_path));
        let directory = Path::new(source_path)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf);
        let (filter_name, extensions) = self.filter();
        SaveDialog {
            file_name: format!("{stem}{EDITED_MARKER}{}", self.ending()),
            directory,
            filter_name,
            extensions,
        }
    }

    /// The ending of the file name of this kind.
    pub(crate) fn ending(self) -> &'static str {
        match self {
            Self::Recording => ".json",
            Self::CompressedRecording => ".json.gz",
            Self::Web => ".html",
            Self::Mp4 => ".mp4",
            Self::Gif => ".gif",
        }
    }

    /// The path to write, when the user picked `picked`.
    ///
    /// A file name that already ends with the ending of this kind stays as it
    /// is. Otherwise the endings of all kinds go, and the ending of this kind
    /// comes. A name with no stem left becomes `recording`.
    ///
    /// # Errors
    /// Returns `NleError::ExportError` when `picked` is not valid UTF-8,
    /// because the export takes the path as text.
    pub(crate) fn output_path(self, picked: PathBuf) -> Result<PathBuf, NleError> {
        output_path_text(&picked)?;
        // The whole path is valid UTF-8, thus only a path with no file name
        // takes this branch.
        let Some(text) = picked.file_name().and_then(OsStr::to_str) else {
            return Ok(picked);
        };
        if ends_with_ignoring_case(text, self.ending()) {
            return Ok(picked);
        }
        let stem = stem_or_default(text);
        Ok(picked.with_file_name(format!("{stem}{}", self.ending())))
    }
}

/// Whether `text` ends with `ending`, with no difference between ASCII
/// capitals and small letters. It never splits a multi-byte character.
fn ends_with_ignoring_case(text: &str, ending: &str) -> bool {
    text.len()
        .checked_sub(ending.len())
        .and_then(|start| text.get(start..))
        .is_some_and(|tail| tail.eq_ignore_ascii_case(ending))
}

/// Removes each trailing ending of `KIND_ENDINGS` from `name`, again and again.
fn strip_kind_endings(mut name: &str) -> &str {
    while let Some(shorter) = KIND_ENDINGS.iter().find_map(|ending| {
        let start = name.len().checked_sub(ending.len())?;
        let tail = name.get(start..)?;
        tail.eq_ignore_ascii_case(ending)
            .then(|| name.get(..start))?
    }) {
        name = shorter;
    }
    name
}

/// `strip_kind_endings`, and `recording` when nothing is left.
fn stem_or_default(name: &str) -> &str {
    match strip_kind_endings(name) {
        "" => DEFAULT_STEM,
        stem => stem,
    }
}

/// Reads the text of the frame rate field. Empty text gives `None`.
fn parse_fps(text: &str) -> Result<Option<NonZeroU32>, NleError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse().map(Some).map_err(|_| {
        NleError::ExportError(format!(
            "the frame rate \"{text}\" is not a whole number above zero"
        ))
    })
}

/// Reads the text of the resolution field. Empty text gives `None`.
fn parse_resolution(text: &str) -> Result<Option<Resolution>, NleError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse().map(Some)
}

/// The text of the progress bar for the newest stage. `None` means that no
/// stage arrived yet.
pub(crate) fn progress_text(progress: Option<ExportProgress>) -> String {
    match progress {
        None => "Export started".to_owned(),
        Some(ExportProgress::Rendering) => "Step 1 of 2: render the edit".to_owned(),
        Some(ExportProgress::Writing) => "Step 2 of 2: write the file".to_owned(),
        Some(ExportProgress::Frames { done, total }) => {
            format!("Step 2 of 2: frame {done} of {total}")
        }
    }
}

/// The settings of the export bar, as the user set them.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ExportSettings {
    /// The kind of file to write.
    pub(crate) kind: ExportKind,
    /// The theme of a web page or a video.
    pub(crate) theme: Theme,
    /// The text of the frame rate field. Empty text leaves the rate to `beta`.
    pub(crate) fps: String,
    /// The text of the resolution field. Empty text leaves the size to `beta`.
    pub(crate) resolution: String,
    /// Whether `beta` compresses the recording in a web page.
    pub(crate) compress_web: bool,
    /// Whether `beta` optimizes a video for the web.
    pub(crate) optimize_web: bool,
}

impl ExportSettings {
    /// What the settings ask `beta` to write.
    ///
    /// A setting that does not apply to the kind is ignored.
    ///
    /// # Errors
    /// Returns `NleError::ExportError` when the kind is a video and the frame
    /// rate or the resolution is not valid.
    pub(crate) fn target(&self) -> Result<ExportTarget, NleError> {
        match self.kind {
            ExportKind::Recording => Ok(ExportTarget::Recording {
                compress: Some(false),
            }),
            ExportKind::CompressedRecording => Ok(ExportTarget::Recording {
                compress: Some(true),
            }),
            ExportKind::Web => Ok(ExportTarget::Web {
                theme: self.theme,
                compress: self.compress_web,
            }),
            ExportKind::Mp4 | ExportKind::Gif => Ok(ExportTarget::Video {
                theme: self.theme,
                fps: parse_fps(&self.fps)?,
                resolution: parse_resolution(&self.resolution)?,
                optimize_web: self.optimize_web,
            }),
        }
    }

    /// The request for a file that the user picked: the path gets the ending
    /// of the kind.
    ///
    /// # Errors
    /// Returns the errors of `ExportKind::output_path` and of
    /// `ExportSettings::target`.
    pub(crate) fn request(&self, picked: PathBuf) -> Result<ExportRequest, NleError> {
        Ok(ExportRequest {
            output_path: self.kind.output_path(picked)?,
            target: self.target()?,
        })
    }
}

/// The text of the Export button.
const EXPORT_LABEL: &str = "Export";
/// The start of the text that names the file that an export wrote.
const EXPORTED_PREFIX: &str = "Exported to";
/// The `title` of the kind menu.
const KIND_TITLE: &str = "Export format";
/// The name of the theme menu.
const THEME_LABEL: &str = "Theme";
/// The name of the frame rate field.
const FPS_LABEL: &str = "FPS";
/// The hint of the frame rate field.
const FPS_PLACEHOLDER: &str = "60";
/// The name of the resolution field.
const RESOLUTION_LABEL: &str = "Resolution";
/// The hint of the resolution field.
const RESOLUTION_PLACEHOLDER: &str = "auto";
/// The `title` of the resolution field.
const RESOLUTION_TITLE: &str = "WIDTHxHEIGHT, or empty for the size of the recording";
/// The name of the check box for a video that `beta` optimizes for the web.
const OPTIMIZE_LABEL: &str = "Optimize for web";
/// The name of the check box for a web page that `beta` compresses.
const COMPRESS_LABEL: &str = "Compress";

/// The export bar of the menu bar. With no open document, it shows nothing.
///
/// The bar holds the controls for the kind of file and its settings, and the
/// Export button. A click on Export asks for a file, exports the open project
/// there, and shows the progress and then the written file. `on_result` gets
/// the end of each export.
///
/// The Export button is disabled while `busy` is true. Thus no export starts
/// during a dialog, a load, or another export.
#[component]
pub(super) fn ExportBar(
    loaded: ReadSignal<Option<Document>>,
    /// True from a click on Open Recording, Add Recording or Export until its
    /// dialog and its work end. Each of the three buttons is disabled while it
    /// is true.
    mut busy: Signal<bool>,
    on_result: EventHandler<Result<(), String>>,
) -> Element {
    let services = use_context::<Services>();
    let mut settings = use_signal(ExportSettings::default);
    // True while the export runs, and the newest stage that it reported.
    let exporting = use_signal(|| false);
    let progress = use_signal(|| None::<ExportProgress>);
    let mut status = use_signal(|| None::<String>);
    if loaded.read().is_none() {
        return rsx! {};
    }
    let current = settings();
    let kind = current.kind;
    let is_video = matches!(kind, ExportKind::Mp4 | ExportKind::Gif);
    let has_theme = is_video || kind == ExportKind::Web;
    let frames = match progress() {
        Some(ExportProgress::Frames { done, total }) => Some((done.to_string(), total.to_string())),
        _ => None,
    };
    rsx! {
        span { class: "export-bar",
            select {
                title: KIND_TITLE,
                onchange: move |event| {
                    if let Some(kind) = ExportKind::from_key(&event.value()) {
                        settings.write().kind = kind;
                    }
                },
                for option_kind in ExportKind::ALL {
                    option {
                        key: "{option_kind.key()}",
                        value: option_kind.key(),
                        selected: option_kind == kind,
                        "{option_kind.label()}"
                    }
                }
            }
            if has_theme {
                label { "{THEME_LABEL}"
                    select {
                        onchange: move |event| {
                            if let Ok(theme) = event.value().parse::<Theme>() {
                                settings.write().theme = theme;
                            }
                        },
                        for theme in Theme::ALL {
                            option {
                                key: "{theme.as_str()}",
                                value: theme.as_str(),
                                selected: theme == current.theme,
                                "{theme.as_str()}"
                            }
                        }
                    }
                }
            }
            if is_video {
                label { "{FPS_LABEL}"
                    input {
                        r#type: "text",
                        placeholder: FPS_PLACEHOLDER,
                        value: "{current.fps}",
                        oninput: move |event| settings.write().fps = event.value(),
                        onkeydown: keep_keys_in_field,
                    }
                }
                label { "{RESOLUTION_LABEL}"
                    input {
                        r#type: "text",
                        placeholder: RESOLUTION_PLACEHOLDER,
                        title: RESOLUTION_TITLE,
                        value: "{current.resolution}",
                        oninput: move |event| settings.write().resolution = event.value(),
                        onkeydown: keep_keys_in_field,
                    }
                }
                label {
                    input {
                        r#type: "checkbox",
                        checked: current.optimize_web,
                        onchange: move |event| settings.write().optimize_web = event.checked(),
                    }
                    "{OPTIMIZE_LABEL}"
                }
            }
            if kind == ExportKind::Web {
                label {
                    input {
                        r#type: "checkbox",
                        checked: current.compress_web,
                        onchange: move |event| settings.write().compress_web = event.checked(),
                    }
                    "{COMPRESS_LABEL}"
                }
            }
            button {
                disabled: busy(),
                onclick: move |_| {
                    // A click that arrives during a dialog, a load, or an export starts nothing.
                    if busy() {
                        return;
                    }
                    // Each click removes the old status, also a click that a bad setting stops.
                    status.set(None);
                    let settings = settings();
                    // A bad setting is an error before any dialog.
                    if let Err(error) = settings.target() {
                        on_result.call(Err(error.to_string()));
                        return;
                    }
                    let Some(source) = loaded
                        .read()
                        .as_ref()
                        .map(|document| document.metadata.file_path.clone())
                    else {
                        return;
                    };
                    busy.set(true);
                    let services = services.clone();
                    spawn(async move {
                        let view = ExportView { progress, exporting, status };
                        let result = export_once(&services, loaded, &settings, &source, view).await;
                        if let Some(result) = result {
                            on_result.call(result);
                        }
                        busy.set(false);
                    });
                },
                "{EXPORT_LABEL}"
            }
            if exporting() {
                span { class: "export-progress",
                    progress {
                        value: frames.as_ref().map(|(done, _)| done.clone()),
                        max: frames.as_ref().map(|(_, total)| total.clone()),
                    }
                    "{progress_text(progress())}"
                }
            }
            if let Some(text) = status() {
                span { class: "export-status", role: "status", "{text}" }
            }
        }
    }
}

/// The signals of the export bar that show one export as it runs.
#[derive(Clone, Copy)]
struct ExportView {
    /// The newest stage that the export reported.
    progress: Signal<Option<ExportProgress>>,
    /// True while the export runs.
    exporting: Signal<bool>,
    /// The text that names the written file, after a success.
    status: Signal<Option<String>>,
}

/// Asks for a file, then exports the open project there and shows each stage
/// in `view`.
///
/// Returns `None` when the user cancels the dialog or no document is open,
/// `Some(Ok(()))` after a success, and `Some(Err(message))` after a failure.
async fn export_once(
    services: &Services,
    loaded: ReadSignal<Option<Document>>,
    settings: &ExportSettings,
    source: &str,
    mut view: ExportView,
) -> Option<Result<(), String>> {
    let picked = services
        .pick_export_path(settings.kind.save_dialog(source))
        .await?;
    let request = match settings.request(picked) {
        Ok(request) => request,
        Err(error) => return Some(Err(error.to_string())),
    };
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    // The read guard ends in this statement, so no guard lives across an await.
    let mut export = loaded
        .read()
        .as_ref()
        .map(|document| services.export(&document.editor, request, sender))?;
    view.progress.set(None);
    view.exporting.set(true);
    let result = loop {
        tokio::select! {
            biased;
            Some(stage) = receiver.recv() => view.progress.set(Some(stage)),
            result = &mut export => break result,
        }
    };
    view.exporting.set(false);
    Some(match result {
        Ok(path) => {
            view.status
                .set(Some(format!("{EXPORTED_PREFIX} {}", path.display())));
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    })
}

#[cfg(test)]
mod tests;
