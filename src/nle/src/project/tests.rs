//! Tests for the rules of the edit project.

use super::*;
use crate::error::NleError;
use crate::types::Seconds;

#[test]
fn seconds_refuses_a_time_that_is_not_finite_or_is_below_zero() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.5] {
        assert!(
            matches!(Seconds::new(bad), Err(NleError::InvalidTime(_))),
            "{bad} was accepted"
        );
    }
}

#[test]
fn seconds_accepts_zero_and_a_positive_time() {
    assert_eq!(Seconds::new(0.0).unwrap().get(), 0.0);
    assert_eq!(Seconds::new(1.5).unwrap().get(), 1.5);
}

#[test]
fn seconds_is_a_plain_json_number_and_refuses_a_negative_one() {
    assert_eq!(
        serde_json::to_string(&Seconds::new(1.5).unwrap()).unwrap(),
        "1.5"
    );
    assert_eq!(serde_json::from_str::<Seconds>("2.5").unwrap().get(), 2.5);
    assert!(serde_json::from_str::<Seconds>("-1.0").is_err());
}

use crate::types::{
    AppliedFilter, Clip, ClipId, EditProject, FilterKind, FilterOptions, FilterRegion, Recording,
    TimelineTrack, TrackId,
};
use std::sync::Arc;

#[test]
fn from_recording_makes_one_track_with_one_whole_clip() {
    let source = Arc::new(Recording::test_fixture());

    let project = EditProject::from_recording(Arc::clone(&source)).unwrap();

    assert_eq!(
        project.timeline,
        vec![TimelineTrack {
            id: TrackId(1),
            clips: vec![ClipId(1)]
        }]
    );
    assert_eq!(project.clips.len(), 1);
    let clip: &Clip = &project.clips[0];
    assert_eq!(clip.id, ClipId(1));
    assert_eq!(clip.in_point.get(), 0.0);
    assert_eq!(clip.out_point.get(), source.duration);
    assert_eq!(clip.timeline_offset.get(), 0.0);
    assert!(Arc::ptr_eq(&clip.source, &source));
    assert!(project.filters.is_empty());
}

#[test]
fn from_recording_refuses_a_negative_duration() {
    let mut source = Recording::test_fixture();
    source.duration = -1.0;

    let result = EditProject::from_recording(Arc::new(source));

    assert!(matches!(result, Err(NleError::InvalidTime(_))));
}

fn clip(id: u64, in_point: f64, out_point: f64, offset: f64) -> Clip {
    Clip {
        id: ClipId(id),
        source: Arc::new(Recording::test_fixture()),
        in_point: Seconds::new(in_point).unwrap(),
        out_point: Seconds::new(out_point).unwrap(),
        timeline_offset: Seconds::new(offset).unwrap(),
    }
}

fn track(id: u64, clips: &[u64]) -> TimelineTrack {
    TimelineTrack {
        id: TrackId(id),
        clips: clips.iter().copied().map(ClipId).collect(),
    }
}

/// Two tracks: clips 1 and 2 on track 1, clip 3 on track 2. The source lasts 5 s.
fn valid_project() -> EditProject {
    EditProject {
        clips: vec![
            clip(1, 0.0, 2.0, 0.0),
            clip(2, 1.0, 5.0, 2.0),
            clip(3, 0.0, 5.0, 0.0),
        ],
        timeline: vec![track(1, &[1, 2]), track(2, &[3])],
        filters: Vec::new(),
    }
}

fn assert_invalid(project: &EditProject, rule: &str) {
    assert!(
        matches!(project.validate(), Err(NleError::InvalidProject(_))),
        "{rule} was accepted"
    );
}

#[test]
fn validate_accepts_a_valid_two_track_project() {
    valid_project().validate().unwrap();
}

#[test]
fn validate_refuses_a_duplicate_clip_id() {
    let mut project = valid_project();
    project.clips[2].id = ClipId(1);
    assert_invalid(&project, "a duplicate clip id");
}

#[test]
fn validate_refuses_a_duplicate_track_id() {
    let mut project = valid_project();
    project.timeline[1].id = TrackId(1);
    assert_invalid(&project, "a duplicate track id");
}

#[test]
fn validate_refuses_an_in_point_after_the_out_point() {
    let mut project = valid_project();
    project.clips[0] = clip(1, 3.0, 2.0, 0.0);
    assert_invalid(&project, "in_point > out_point");
}

#[test]
fn validate_refuses_an_out_point_after_the_source_duration() {
    let mut project = valid_project();
    project.clips[0] = clip(1, 0.0, 5.5, 0.0);
    assert_invalid(&project, "out_point > source duration");
}

#[test]
fn validate_refuses_a_track_that_names_a_missing_clip() {
    let mut project = valid_project();
    project.timeline[1].clips.push(ClipId(99));
    assert_invalid(&project, "a track that names a missing clip");
}

#[test]
fn validate_refuses_a_clip_on_no_track() {
    let mut project = valid_project();
    project.timeline[1].clips.clear();
    assert_invalid(&project, "a clip on no track");
}

#[test]
fn validate_refuses_a_clip_on_two_tracks() {
    let mut project = valid_project();
    project.timeline[1].clips.push(ClipId(1));
    assert_invalid(&project, "a clip on two tracks");
}

#[test]
fn validate_refuses_a_clip_twice_on_one_track() {
    let mut project = valid_project();
    project.timeline[0].clips.push(ClipId(1));
    assert_invalid(&project, "a clip twice on one track");
}

#[test]
fn validate_refuses_a_filter_range_with_start_after_end() {
    let mut project = valid_project();
    project.filters.push(AppliedFilter {
        kind: FilterKind::new("speed"),
        region: FilterRegion::Range {
            start: Seconds::new(2.0).unwrap(),
            end: Seconds::new(1.0).unwrap(),
        },
        options: FilterOptions::new(),
    });
    assert_invalid(&project, "a range with start > end");
}
