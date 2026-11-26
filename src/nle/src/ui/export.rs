//! The export bar of the menu bar: the kinds of file that it writes, the
//! settings that the user sets, and the text of the progress.
//!
//! The types here are plain values, so the tests of `ui/export/tests.rs`
//! need no window.

use std::ffi::OsString;
use std::path::PathBuf;

/// The stem of a file name that has no stem left after the endings go.
const DEFAULT_STEM: &str = "recording";
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

impl ExportKind {
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

#[cfg(test)]
mod tests;
