//! The editing state of the app: the edit operations, the project with its
//! undo and redo history, and the save of the rendered recording.

use std::sync::Arc;

use crate::error::NleError;
use crate::pipeline::FilterRegistry;
use crate::types::{AppliedFilter, Clip, ClipId, EditProject, Recording, Seconds, TrackId};

/// One edit of a project.
///
/// An edit that names a clip, a track, or a filter index that does not exist
/// is refused with `NleError::EditError`. So is an edit that adds an id that
/// is already in use. An edit that leaves the project in a state that breaks
/// a rule of `EditProject::validate` is refused with `NleError::InvalidProject`.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Append an empty track.
    AddTrack {
        /// The id of the new track.
        id: TrackId,
    },
    /// Remove a track. It is refused when the track holds a clip.
    RemoveTrack {
        /// The track to remove.
        id: TrackId,
    },
    /// Append a clip to the clips of the project and to the end of a track.
    AddClip {
        /// The new clip.
        clip: Clip,
        /// The track that gets the clip.
        track: TrackId,
    },
    /// Remove a clip from the clips of the project and from its track.
    RemoveClip {
        /// The clip to remove.
        id: ClipId,
    },
    /// Set the range of the source that a clip uses.
    TrimClip {
        /// The clip to trim.
        id: ClipId,
        /// The new start within the source.
        in_point: Seconds,
        /// The new end within the source.
        out_point: Seconds,
    },
    /// Place a clip at the end of a track, at a time on the timeline. The
    /// track may be the one that holds the clip now.
    MoveClip {
        /// The clip to move.
        id: ClipId,
        /// The track that gets the clip.
        track: TrackId,
        /// The new start of the clip on the timeline.
        timeline_offset: Seconds,
    },
    /// Insert a filter at an index. The index may be the length of the list.
    AddFilter {
        /// Where the filter goes in the list.
        index: usize,
        /// The filter to insert.
        filter: AppliedFilter,
    },
    /// Remove the filter at an index.
    RemoveFilter {
        /// The index of the filter.
        index: usize,
    },
    /// Replace the filter at an index.
    ReplaceFilter {
        /// The index of the filter.
        index: usize,
        /// The new filter.
        filter: AppliedFilter,
    },
}

/// An edit that was applied, and the project as it was before.
#[derive(Debug)]
struct Applied {
    edit: Edit,
    before: EditProject,
}

/// The editing state of the app: the project, its undo and redo history, and
/// the filters that render it.
#[derive(Debug)]
pub struct Editor {
    project: EditProject,
    undo: Vec<Applied>,
    redo: Vec<Edit>,
    filters: Arc<FilterRegistry>,
}

impl Editor {
    /// Make an editor for `project`, with an empty history.
    ///
    /// # Errors
    /// Returns the `NleError::InvalidProject` of `EditProject::validate`.
    pub fn new(project: EditProject, filters: Arc<FilterRegistry>) -> Result<Self, NleError> {
        project.validate()?;
        Ok(Self {
            project,
            undo: Vec::new(),
            redo: Vec::new(),
            filters,
        })
    }

    /// The project in its present state.
    #[must_use]
    pub fn project(&self) -> &EditProject {
        &self.project
    }

    /// Apply `edit`. On success the edit goes on the undo history and the
    /// redo history is cleared.
    ///
    /// # Errors
    /// Returns `NleError::EditError` when the edit names something that does
    /// not exist or an id that is in use, and `NleError::InvalidProject` when
    /// the result breaks a rule of `EditProject::validate`. A refused edit
    /// changes nothing.
    pub fn apply(&mut self, edit: Edit) -> Result<(), NleError> {
        let _ = edit;
        todo!()
    }

    /// Undo the last edit. Returns `false` when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        todo!()
    }

    /// Redo the last undone edit. Returns `Ok(false)` when there is nothing
    /// to redo.
    ///
    /// # Errors
    /// Returns the error of the edit, if it is refused now.
    pub fn redo(&mut self) -> Result<bool, NleError> {
        todo!()
    }

    /// Whether `undo` has an edit to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether `redo` has an edit to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Render the project in its present state into one recording.
    ///
    /// # Errors
    /// Returns the errors of `FilterRegistry::render`.
    pub fn render(&self) -> Result<Recording, NleError> {
        todo!()
    }

    /// Write the rendered recording of this state to `path`.
    ///
    /// The future holds a copy of the state, so it borrows nothing from the
    /// editor. `compress` is as for the save of a recording: `None` follows
    /// the `.gz` ending of the file name.
    ///
    /// # Errors
    /// The future gives `NleError::SaveError` when the path is refused or the
    /// file cannot be written, the errors of `FilterRegistry::render`, and
    /// `NleError::TaskError` when the work panics.
    pub fn save_recording(
        &self,
        path: String,
        compress: Option<bool>,
    ) -> impl std::future::Future<Output = Result<(), NleError>> + Send + 'static {
        let _ = (path, compress);
        async { todo!() }
    }
}

#[cfg(test)]
mod tests;
