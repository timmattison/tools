//! The export bar of the menu bar: the kinds of file that it writes, the
//! settings that the user sets, and the text of the progress.
//!
//! The types here are plain values, so the tests of `ui/export/tests.rs`
//! need no window.

use std::path::PathBuf;

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
        ""
    }

    /// The path to write, when the user picked `picked`.
    pub(crate) fn output_path(self, picked: PathBuf) -> PathBuf {
        picked
    }
}

#[cfg(test)]
mod tests;
