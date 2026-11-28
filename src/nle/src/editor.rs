//! The editing state of the app: the edit operations, the project with its
//! undo and redo history, and the save of the rendered recording.

use std::sync::Arc;

use crate::error::NleError;
use crate::pipeline::FilterRegistry;
use crate::types::{
    AppliedFilter, Clip, ClipId, EditProject, Recording, Seconds, TimelineTrack, TrackId,
};

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
    /// Put a clip at a new place in the play order of its track, and put the
    /// clips of that track back to back. The play order of a track is the
    /// order of the starts of its clips, and a tie keeps the order on the
    /// track. The first clip in the new order starts at the earliest start on
    /// the track, and each next clip starts at the end of the clip before it.
    /// After the edit, the order on the track is the new play order. A place
    /// at or past the number of clips of the track is refused with
    /// `NleError::EditError`, and a start that is not a valid time with
    /// `NleError::InvalidTime`.
    ReorderClip {
        /// The clip to move.
        id: ClipId,
        /// The new place of the clip in the play order of its track, from 0.
        index: usize,
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

impl Edit {
    /// The edit that adds the whole of `source` as a new clip of `project`.
    ///
    /// # Errors
    /// Returns `NleError::EditError` for every call.
    pub fn append_recording(
        project: &EditProject,
        source: Arc<Recording>,
    ) -> Result<Self, NleError> {
        let _ = (project, source);
        Err(NleError::EditError("not yet".to_owned()))
    }

    /// Apply this edit to `project`. It may leave `project` half changed when
    /// it fails, so the caller gives it a copy. It does not validate the
    /// result.
    fn apply_to(&self, project: &mut EditProject) -> Result<(), NleError> {
        match self {
            Self::AddTrack { id } => {
                if project.timeline.iter().any(|track| track.id == *id) {
                    return Err(NleError::EditError(format!(
                        "track {} is already in use",
                        id.0
                    )));
                }
                project.timeline.push(TimelineTrack {
                    id: *id,
                    clips: Vec::new(),
                });
                Ok(())
            }
            Self::RemoveTrack { id } => {
                let position = track_position(project, *id)?;
                if project
                    .timeline
                    .get(position)
                    .is_some_and(|track| !track.clips.is_empty())
                {
                    return Err(NleError::EditError(format!("track {} holds a clip", id.0)));
                }
                project.timeline.remove(position);
                Ok(())
            }
            Self::AddClip { clip, track } => {
                if project.clips.iter().any(|existing| existing.id == clip.id) {
                    return Err(NleError::EditError(format!(
                        "clip {} is already in use",
                        clip.id.0
                    )));
                }
                let position = track_position(project, *track)?;
                let track_id = *track;
                let track = project
                    .timeline
                    .get_mut(position)
                    .ok_or_else(|| missing_track(track_id))?;
                track.clips.push(clip.id);
                project.clips.push(clip.clone());
                Ok(())
            }
            Self::RemoveClip { id } => {
                let position = clip_position(project, *id)?;
                project.clips.remove(position);
                for track in &mut project.timeline {
                    track.clips.retain(|clip| clip != id);
                }
                Ok(())
            }
            Self::TrimClip {
                id,
                in_point,
                out_point,
            } => {
                let position = clip_position(project, *id)?;
                let clip = project
                    .clips
                    .get_mut(position)
                    .ok_or_else(|| missing_clip(*id))?;
                clip.in_point = *in_point;
                clip.out_point = *out_point;
                Ok(())
            }
            Self::MoveClip {
                id,
                track,
                timeline_offset,
            } => {
                let clip_index = clip_position(project, *id)?;
                let track_index = track_position(project, *track)?;
                for other in &mut project.timeline {
                    other.clips.retain(|clip| clip != id);
                }
                project
                    .timeline
                    .get_mut(track_index)
                    .ok_or_else(|| missing_track(*track))?
                    .clips
                    .push(*id);
                project
                    .clips
                    .get_mut(clip_index)
                    .ok_or_else(|| missing_clip(*id))?
                    .timeline_offset = *timeline_offset;
                Ok(())
            }
            Self::ReorderClip { id, index } => reorder_clip(project, *id, *index),
            Self::AddFilter { index, filter } => {
                if *index > project.filters.len() {
                    return Err(missing_filter(*index));
                }
                project.filters.insert(*index, filter.clone());
                Ok(())
            }
            Self::RemoveFilter { index } => {
                if *index >= project.filters.len() {
                    return Err(missing_filter(*index));
                }
                project.filters.remove(*index);
                Ok(())
            }
            Self::ReplaceFilter { index, filter } => {
                let slot = project
                    .filters
                    .get_mut(*index)
                    .ok_or_else(|| missing_filter(*index))?;
                *slot = filter.clone();
                Ok(())
            }
        }
    }
}

/// The edit `Edit::ReorderClip`: put clip `id` at `index` in the play order
/// of its track, then start each clip of the track at the end of the clip
/// before it, from the earliest start on the track.
///
/// The play order of a track is the order of the starts of its clips. A tie
/// keeps the order on the track. After the edit, the order on the track is
/// the new play order.
fn reorder_clip(project: &mut EditProject, id: ClipId, index: usize) -> Result<(), NleError> {
    clip_position(project, id)?;
    let track = project
        .timeline
        .iter_mut()
        .find(|track| track.clips.contains(&id))
        .ok_or_else(|| NleError::EditError(format!("clip {} is on no track", id.0)))?;
    let clips = &mut project.clips;
    let start_of = |clip_id: &ClipId| {
        clips
            .iter()
            .find(|clip| clip.id == *clip_id)
            .map_or(f64::INFINITY, |clip| clip.timeline_offset.get())
    };
    let mut order = track.clips.clone();
    // A stable sort keeps the order on the track on a tie.
    order.sort_by(|a, b| start_of(a).total_cmp(&start_of(b)));
    if index >= order.len() {
        return Err(NleError::EditError(format!(
            "place {index} is past the end of track {}, which holds {} clips",
            track.id.0,
            order.len()
        )));
    }
    let mut at = order.iter().map(start_of).fold(f64::INFINITY, f64::min);
    let from = order
        .iter()
        .position(|clip_id| *clip_id == id)
        .ok_or_else(|| missing_clip(id))?;
    let moved = order.remove(from);
    order.insert(index, moved);
    for clip_id in &order {
        let clip = clips
            .iter_mut()
            .find(|clip| clip.id == *clip_id)
            .ok_or_else(|| missing_clip(*clip_id))?;
        clip.timeline_offset = Seconds::new(at)?;
        at += clip.out_point.get() - clip.in_point.get();
    }
    track.clips = order;
    Ok(())
}

/// The edit error for a filter index that is out of range.
fn missing_filter(index: usize) -> NleError {
    NleError::EditError(format!("filter index {index} is out of range"))
}

/// The edit error for a clip that does not exist.
fn missing_clip(id: ClipId) -> NleError {
    NleError::EditError(format!("clip {} does not exist", id.0))
}

/// The edit error for a track that does not exist.
fn missing_track(id: TrackId) -> NleError {
    NleError::EditError(format!("track {} does not exist", id.0))
}

/// The index of clip `id` in the clips of the project, or an edit error.
fn clip_position(project: &EditProject, id: ClipId) -> Result<usize, NleError> {
    project
        .clips
        .iter()
        .position(|clip| clip.id == id)
        .ok_or_else(|| missing_clip(id))
}

/// The index of track `id` in the timeline, or an edit error.
fn track_position(project: &EditProject, id: TrackId) -> Result<usize, NleError> {
    project
        .timeline
        .iter()
        .position(|track| track.id == id)
        .ok_or_else(|| missing_track(id))
}

/// The check of `Editor::apply` and `Editor::redo`: the rules of
/// `EditProject::validate`, with no render, so no duration to keep.
fn validate_without_render(next: &EditProject) -> Result<Option<f64>, NleError> {
    next.validate().map(|()| None)
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
    /// The duration of the render of `project`, when the check of the last
    /// edit rendered it. Each change of `project` sets it again.
    kept_duration: Option<f64>,
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
            kept_duration: None,
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
        self.commit(edit, validate_without_render)?;
        self.redo.clear();
        Ok(())
    }

    /// Apply `edit` like `apply`, and also refuse it when the project after
    /// the edit does not render. A project with no clip does not render, so
    /// use `apply` for an edit that can leave the project empty.
    ///
    /// A refused edit changes nothing: the project, the undo history, and the
    /// redo history stay as they were.
    ///
    /// # Errors
    /// Returns the errors of `apply`, and the errors of
    /// `FilterRegistry::render` for the project after the edit, for example
    /// `NleError::RenderError` when a filter answers a length that is not
    /// finite.
    pub fn apply_renderable(&mut self, edit: Edit) -> Result<(), NleError> {
        let filters = Arc::clone(&self.filters);
        self.commit(edit, move |next| {
            filters
                .render(next)
                .map(|recording| Some(recording.duration))
        })?;
        self.redo.clear();
        Ok(())
    }

    /// Apply `edit` to a copy of the project, run `check` on the copy, and
    /// only then make it the project and put the edit on the undo history.
    /// `check` answers the duration of the render of the copy when it
    /// rendered the copy. A failure of the edit or of `check` changes
    /// nothing. It leaves the redo history alone.
    fn commit(
        &mut self,
        edit: Edit,
        check: impl FnOnce(&EditProject) -> Result<Option<f64>, NleError>,
    ) -> Result<(), NleError> {
        let mut next = self.project.clone();
        edit.apply_to(&mut next)?;
        let kept_duration = check(&next)?;
        let before = std::mem::replace(&mut self.project, next);
        self.kept_duration = kept_duration;
        self.undo.push(Applied { edit, before });
        Ok(())
    }

    /// Undo the last edit. Returns `false` when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(applied) = self.undo.pop() else {
            return false;
        };
        self.project = applied.before;
        self.kept_duration = None;
        self.redo.push(applied.edit);
        true
    }

    /// Redo the last undone edit. Returns `Ok(false)` when there is nothing
    /// to redo.
    ///
    /// # Errors
    /// Returns the error of the edit, if it is refused now.
    pub fn redo(&mut self) -> Result<bool, NleError> {
        let Some(edit) = self.redo.last().cloned() else {
            return Ok(false);
        };
        // A refused edit stays on the redo history, and nothing changes.
        self.commit(edit, validate_without_render)?;
        self.redo.pop();
        Ok(true)
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
        self.filters.render(&self.project)
    }

    /// The duration of the render of the project in its present state. It
    /// renders the project only when the check of the last edit did not.
    ///
    /// # Errors
    /// Returns the errors of `FilterRegistry::render`.
    pub fn rendered_duration(&self) -> Result<f64, NleError> {
        self.kept_duration
            .map_or_else(|| self.render().map(|recording| recording.duration), Ok)
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
        let project = self.project.clone();
        let filters = Arc::clone(&self.filters);
        crate::commands::run_blocking(move || {
            let recording = filters.render(&project)?;
            crate::commands::save_recording_to(&path, &recording, compress)
        })
    }
}

#[cfg(test)]
mod tests;
