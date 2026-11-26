//! The export bar of the menu bar: the kinds of file that it writes, the
//! settings that the user sets, and the text of the progress.
//!
//! The types here are plain values, so the tests of `ui/export/tests.rs`
//! need no window.

use std::ffi::OsString;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use crate::error::NleError;
use crate::export::{ExportProgress, ExportRequest, ExportTarget, Resolution, Theme};
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
    /// comes. A name with no stem left becomes `recording`. A file name that
    /// is not valid UTF-8 only gets the ending.
    pub(crate) fn output_path(self, picked: PathBuf) -> PathBuf {
        let Some(name) = picked.file_name() else {
            return picked;
        };
        let Some(text) = name.to_str() else {
            let mut name: OsString = name.to_owned();
            name.push(self.ending());
            return picked.with_file_name(name);
        };
        if ends_with_ignoring_case(text, self.ending()) {
            return picked;
        }
        let stem = stem_or_default(text);
        picked.with_file_name(format!("{stem}{}", self.ending()))
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
        tail.eq_ignore_ascii_case(ending).then(|| name.get(..start))?
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

/// The text of the progress before the first stage.
pub(crate) fn progress_text(progress: Option<ExportProgress>) -> String {
    String::new()
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
    /// Returns the errors of `ExportSettings::target`.
    pub(crate) fn request(&self, picked: PathBuf) -> Result<ExportRequest, NleError> {
        Ok(ExportRequest {
            output_path: self.kind.output_path(picked),
            target: self.target()?,
        })
    }
}

#[cfg(test)]
mod tests;
