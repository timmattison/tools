//! The export of an edited project: the stages of an export, the settings of
//! a `beta export` run, and the read of the progress that `beta` reports.

/// A stage of an export, for a progress display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportProgress {
    /// The editor renders the project into one recording.
    Rendering,
    /// The rendered recording goes to its file. For a web page or a video,
    /// `beta` runs, and it reported no frame yet.
    Writing,
    /// `beta` wrote `done` of `total` frames.
    Frames {
        /// The number of frames that `beta` wrote.
        done: u64,
        /// The number of frames in the video.
        total: u64,
    },
}

/// Read one line of the stdout of `beta`.
pub(crate) fn parse_progress_line(_line: &str) -> Option<ExportProgress> {
    None
}

#[cfg(test)]
mod tests;
