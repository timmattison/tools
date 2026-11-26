//! The rules of the edit project: its constructor, its validation, its ids, its
//! length, and its file format.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::error::NleError;
use crate::types::{
    Clip, ClipId, EditProject, FilterRegion, Recording, Seconds, TimelineTrack, TrackId,
};

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
        let invalid = |message: String| Err(NleError::InvalidProject(message));

        let mut clip_ids = HashSet::new();
        for clip in &self.clips {
            if !clip_ids.insert(clip.id) {
                return invalid(format!("clip id {} is used twice", clip.id.0));
            }
        }
        let mut track_ids = HashSet::new();
        for track in &self.timeline {
            if !track_ids.insert(track.id) {
                return invalid(format!("track id {} is used twice", track.id.0));
            }
        }

        for clip in &self.clips {
            let (in_point, out_point) = (clip.in_point.get(), clip.out_point.get());
            let duration = clip.source.duration;
            if in_point > out_point {
                return invalid(format!(
                    "clip {} starts at {in_point} s, after its end at {out_point} s",
                    clip.id.0
                ));
            }
            // A NaN or a negative duration fails the comparison too.
            if !(out_point <= duration) {
                return invalid(format!(
                    "clip {} ends at {out_point} s, after the end of its source at {duration} s",
                    clip.id.0
                ));
            }
        }

        let mut uses: HashMap<ClipId, usize> = HashMap::new();
        for track in &self.timeline {
            for id in &track.clips {
                if !clip_ids.contains(id) {
                    return invalid(format!(
                        "track {} names clip {}, which does not exist",
                        track.id.0, id.0
                    ));
                }
                *uses.entry(*id).or_insert(0) += 1;
            }
        }
        for clip in &self.clips {
            match uses.get(&clip.id).copied().unwrap_or(0) {
                1 => {}
                0 => return invalid(format!("clip {} is on no track", clip.id.0)),
                n => return invalid(format!("clip {} is on a track {n} times", clip.id.0)),
            }
        }

        for filter in &self.filters {
            if let FilterRegion::Range { start, end } = &filter.region {
                if start > end {
                    return invalid(format!(
                        "filter {} has a range that starts at {} s, after its end at {} s",
                        filter.kind.as_str(),
                        start.get(),
                        end.get()
                    ));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests;
