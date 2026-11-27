//! The rules of the edit project: its constructor, its validation, its ids, its
//! length, and its file format.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::NleError;
use crate::json_file;
use crate::types::{
    AppliedFilter, ChapterId, ChapterMarker, Clip, ClipId, EditProject, Event, FilterRegion,
    Recording, Seconds, TimelineTrack, TrackId,
};

/// The form of a project in its file. Each source is written once, and each
/// clip names its source by its index in `sources`.
#[derive(Serialize, Deserialize)]
pub(crate) struct ProjectFile {
    sources: Vec<Arc<Recording>>,
    clips: Vec<ClipFile>,
    timeline: Vec<TimelineTrack>,
    filters: Vec<AppliedFilter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    chapters: Vec<ChapterMarker>,
}

/// A clip in the file of a project: `source` is an index in `ProjectFile::sources`.
#[derive(Serialize, Deserialize)]
struct ClipFile {
    id: ClipId,
    source: usize,
    in_point: Seconds,
    out_point: Seconds,
    timeline_offset: Seconds,
}

impl From<EditProject> for ProjectFile {
    /// Share sources by `Arc::ptr_eq` only. Two different `Arc`s with equal
    /// content stay two entries, because a compare of the events is costly.
    fn from(project: EditProject) -> Self {
        let mut sources: Vec<Arc<Recording>> = Vec::new();
        let mut clips = Vec::with_capacity(project.clips.len());
        for clip in project.clips {
            let index = sources
                .iter()
                .position(|known| Arc::ptr_eq(known, &clip.source))
                .unwrap_or_else(|| {
                    sources.push(Arc::clone(&clip.source));
                    sources.len() - 1
                });
            clips.push(ClipFile {
                id: clip.id,
                source: index,
                in_point: clip.in_point,
                out_point: clip.out_point,
                timeline_offset: clip.timeline_offset,
            });
        }
        Self {
            sources,
            clips,
            timeline: project.timeline,
            filters: project.filters,
            chapters: project.chapters,
        }
    }
}

impl TryFrom<ProjectFile> for EditProject {
    type Error = String;

    /// A clip that names a source the table does not hold is an error. A source
    /// that no clip names is dropped.
    fn try_from(file: ProjectFile) -> Result<Self, Self::Error> {
        let mut clips = Vec::with_capacity(file.clips.len());
        for clip in file.clips {
            let source = file.sources.get(clip.source).ok_or_else(|| {
                format!(
                    "clip {} names source {}, but the file holds {} sources",
                    clip.id.0,
                    clip.source,
                    file.sources.len()
                )
            })?;
            clips.push(Clip {
                id: clip.id,
                source: Arc::clone(source),
                in_point: clip.in_point,
                out_point: clip.out_point,
                timeline_offset: clip.timeline_offset,
            });
        }
        Ok(Self {
            clips,
            timeline: file.timeline,
            filters: file.filters,
            chapters: file.chapters,
        })
    }
}

impl EditProject {
    /// Make a project of one clip that holds the whole `source`, on one track.
    /// The project has one chapter marker for each chapter of `source`, in the
    /// order of `source`, with the ids 1, 2, 3, and so on.
    ///
    /// # Errors
    /// Returns `NleError::InvalidTime` when the duration of `source` or the
    /// time of one of its chapters is not a valid time, and
    /// `NleError::InvalidProject` when the project breaks a rule of
    /// `EditProject::validate`, for example a chapter label that holds only
    /// white space.
    pub fn from_recording(source: Arc<Recording>) -> Result<Self, NleError> {
        let out_point = Seconds::new(source.duration)?;
        let chapters = (1..)
            .zip(&source.chapters)
            .map(|(id, chapter)| {
                Ok(ChapterMarker {
                    id: ChapterId(id),
                    time: Seconds::new(chapter.time)?,
                    label: chapter.label.clone(),
                })
            })
            .collect::<Result<Vec<_>, NleError>>()?;
        let clip = Clip {
            id: ClipId(1),
            source,
            in_point: Seconds::ZERO,
            out_point,
            timeline_offset: Seconds::ZERO,
        };
        let project = Self {
            clips: vec![clip],
            timeline: vec![TimelineTrack {
                id: TrackId(1),
                clips: vec![ClipId(1)],
            }],
            filters: Vec::new(),
            chapters,
        };
        project.validate()?;
        Ok(project)
    }

    /// The id for a new clip: one more than the largest id in use, or 1 when
    /// there is no clip. The id saturates at `u64::MAX`.
    #[must_use]
    pub fn next_clip_id(&self) -> ClipId {
        ClipId(
            self.clips
                .iter()
                .map(|c| c.id.0)
                .max()
                .map_or(1, |m| m.saturating_add(1)),
        )
    }

    /// The id for a new chapter marker: one more than the largest id in use, or
    /// 1 when there is no marker. The id saturates at `u64::MAX`.
    #[must_use]
    pub fn next_chapter_id(&self) -> ChapterId {
        ChapterId(
            self.chapters
                .iter()
                .map(|c| c.id.0)
                .max()
                .map_or(1, |m| m.saturating_add(1)),
        )
    }

    /// The chapter markers sorted by time, then by id.
    #[must_use]
    pub fn chapters_in_order(&self) -> Vec<&ChapterMarker> {
        let mut ordered: Vec<&ChapterMarker> = self.chapters.iter().collect();
        ordered.sort_by(|a, b| {
            a.time
                .get()
                .total_cmp(&b.time.get())
                .then_with(|| a.id.cmp(&b.id))
        });
        ordered
    }

    /// The id for a new track: one more than the largest id in use, or 1 when
    /// there is no track. The id saturates at `u64::MAX`.
    #[must_use]
    pub fn next_track_id(&self) -> TrackId {
        TrackId(
            self.timeline
                .iter()
                .map(|t| t.id.0)
                .max()
                .map_or(1, |m| m.saturating_add(1)),
        )
    }

    /// The end of the timeline in seconds: the largest
    /// `timeline_offset + (out_point - in_point)` over all clips, or 0.0 when
    /// the project has no clip.
    #[must_use]
    pub fn duration(&self) -> f64 {
        self.clips
            .iter()
            .map(|clip| clip.timeline_offset.get() + (clip.out_point.get() - clip.in_point.get()))
            .fold(0.0, f64::max)
    }

    /// The number of events that the clips of the project hold.
    #[must_use]
    pub fn event_count(&self) -> usize {
        self.clips
            .iter()
            .map(|clip| clip.timeline_events().count())
            .sum()
    }

    /// Load a project from a JSON file, plain or gzip (found by its magic
    /// bytes), and check its rules. The clips that name one source of the file
    /// share one `Arc` of it.
    ///
    /// # Errors
    /// Returns `NleError::LoadError` when the file cannot be read,
    /// `NleError::InvalidFormat` when it is empty or does not parse as a project
    /// (a negative time is a parse failure, and so is a clip that names a source
    /// that the file does not hold), and `NleError::InvalidProject`
    /// when it parses but breaks a rule of `validate`.
    pub fn load(path: &Path) -> Result<Self, NleError> {
        let project: Self = json_file::load(path, "project")?;
        project.validate()?;
        Ok(project)
    }

    /// Save the project to a JSON file, optionally gzip compressed. The save is
    /// atomic, as the save of a recording is. The file holds each source once:
    /// the clips that share one `Arc` name one entry.
    ///
    /// # Errors
    /// Returns `NleError::SaveError` when the project cannot be serialized,
    /// compressed, or written.
    pub fn save(&self, path: &Path, compress: bool) -> Result<(), NleError> {
        json_file::save(self, path, compress, "project")
    }

    /// Check the rules of a project.
    ///
    /// The rules, in order: each clip id and each track id is unique; each clip
    /// has `in_point <= out_point <= source duration`; each clip id on a track
    /// names a clip, and each clip is on exactly one track, exactly once; each
    /// filter range has `start <= end`; each chapter id is unique; each chapter
    /// label holds a character that is not white space.
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
            if !matches!(
                out_point.partial_cmp(&duration),
                Some(Ordering::Less | Ordering::Equal)
            ) {
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

        let mut chapter_ids = HashSet::new();
        for chapter in &self.chapters {
            if !chapter_ids.insert(chapter.id) {
                return invalid(format!("chapter id {} is used twice", chapter.id.0));
            }
        }
        for chapter in &self.chapters {
            if chapter.label.trim().is_empty() {
                return invalid(format!("chapter {} has an empty label", chapter.id.0));
            }
        }

        Ok(())
    }
}

impl Clip {
    /// The events that this clip holds, with their times moved onto the timeline.
    ///
    /// A clip holds each event of its source with `in_point <= time < out_point`.
    /// A clip whose `out_point` is the end of its source
    /// (`out_point >= source.duration`) also holds an event at exactly
    /// `out_point`. A recording sets its duration to the time of the last
    /// event, so a clip of the whole recording keeps the last event, and two
    /// clips that a cut splits at one time do not both hold an event at that
    /// time. An event at time `t` goes to `t - in_point + timeline_offset`.
    pub fn timeline_events(&self) -> impl Iterator<Item = Event> + '_ {
        let in_point = self.in_point.get();
        self.source
            .events
            .iter()
            .filter(move |event| self.holds(event.time))
            .map(move |event| Event {
                time: event.time - in_point + self.timeline_offset.get(),
                ..event.clone()
            })
    }

    /// True when this clip holds an event of its source at `time`, by the
    /// rule of `timeline_events`.
    #[must_use]
    pub fn holds(&self, time: f64) -> bool {
        let (in_point, out_point) = (self.in_point.get(), self.out_point.get());
        let to_the_end = out_point >= self.source.duration;
        time >= in_point && (time < out_point || (to_the_end && time == out_point))
    }
}

#[cfg(test)]
mod tests;
