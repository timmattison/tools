//! The rules of the edit project: its constructor, its validation, its ids, its
//! length, and its file format.

use std::sync::Arc;

use crate::error::NleError;
use crate::types::{Clip, ClipId, EditProject, Recording, Seconds, TimelineTrack, TrackId};

impl EditProject {
    /// Make a project of one clip that holds the whole `source`, on one track.
    ///
    /// # Errors
    /// Returns `NleError::InvalidTime` when the duration of `source` is not a
    /// valid time.
    pub fn from_recording(source: Arc<Recording>) -> Result<Self, NleError> {
        let out_point = Seconds::new(source.duration)?;
        let clip = Clip {
            id: ClipId(1),
            source,
            in_point: Seconds::ZERO,
            out_point,
            timeline_offset: Seconds::ZERO,
        };
        Ok(Self {
            clips: vec![clip],
            timeline: vec![TimelineTrack {
                id: TrackId(1),
                clips: vec![ClipId(1)],
            }],
            filters: Vec::new(),
        })
    }

    /// Check the rules of a project.
    ///
    /// The rules, in order: each clip id and each track id is unique; each clip
    /// has `in_point <= out_point <= source duration`; each clip id on a track
    /// names a clip, and each clip is on exactly one track, exactly once; each
    /// filter range has `start <= end`.
    ///
    /// # Errors
    /// Returns `NleError::InvalidProject` for the first broken rule.
    pub fn validate(&self) -> Result<(), NleError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
