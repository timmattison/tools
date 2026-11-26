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
}

#[cfg(test)]
mod tests;
