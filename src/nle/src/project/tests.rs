//! Tests for the rules of the edit project.

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
fn seconds_from_whole_seconds_gives_that_many_seconds() {
    const TWO: Seconds = Seconds::from_whole_seconds(2);
    assert_eq!(TWO.get(), 2.0);
    assert_eq!(Seconds::from_whole_seconds(0), Seconds::ZERO);
    assert_eq!(Seconds::from_whole_seconds(u32::MAX).get(), 4_294_967_295.0);
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
    AppliedFilter, Clip, ClipId, EditProject, Event, EventType, FilterKind, FilterOptions,
    FilterRegion, Recording, TimelineTrack, TrackId,
};
use crate::types::{Chapter, ChapterId, ChapterMarker};
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

#[test]
fn from_recording_makes_one_marker_for_each_chapter_of_the_source() {
    let mut source = Recording::test_fixture();
    source.chapters = vec![
        Chapter {
            time: 2.5,
            label: "日本語テスト".to_string(),
        },
        Chapter {
            time: 1.0,
            label: "café 🎉".to_string(),
        },
    ];

    let project = EditProject::from_recording(Arc::new(source)).unwrap();

    assert_eq!(
        project.chapters,
        vec![
            ChapterMarker {
                id: ChapterId(1),
                time: Seconds::new(2.5).unwrap(),
                label: "日本語テスト".to_string(),
            },
            ChapterMarker {
                id: ChapterId(2),
                time: Seconds::new(1.0).unwrap(),
                label: "café 🎉".to_string(),
            },
        ]
    );
}

#[test]
fn from_recording_refuses_a_chapter_time_below_zero() {
    let mut source = Recording::test_fixture();
    source.chapters = vec![Chapter {
        time: -1.0,
        label: "early".to_string(),
    }];

    let result = EditProject::from_recording(Arc::new(source));

    assert!(
        matches!(result, Err(NleError::InvalidTime(_))),
        "{result:?}"
    );
}

#[test]
fn from_recording_refuses_a_chapter_with_a_blank_label() {
    for label in ["", "  \t"] {
        let mut source = Recording::test_fixture();
        source.chapters = vec![
            Chapter {
                time: 1.0,
                label: "Setup".to_string(),
            },
            Chapter {
                time: 2.0,
                label: label.to_string(),
            },
        ];

        let result = EditProject::from_recording(Arc::new(source));

        match result {
            Err(NleError::InvalidProject(message)) => {
                assert_eq!(message, "chapter 2 has an empty label", "{label:?}");
            }
            other => panic!("the label {label:?} gave {other:?}"),
        }
    }
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
        chapters: Vec::new(),
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

#[test]
fn next_ids_are_one_for_an_empty_project() {
    let project = EditProject::default();
    assert_eq!(project.next_clip_id(), ClipId(1));
    assert_eq!(project.next_track_id(), TrackId(1));
}

#[test]
fn next_ids_are_one_more_than_the_largest_id_in_use() {
    let mut project = valid_project();
    project.clips[0].id = ClipId(7);
    project.timeline[0].clips[0] = ClipId(7);
    project.timeline[1].id = TrackId(4);
    assert_eq!(project.next_clip_id(), ClipId(8));
    assert_eq!(project.next_track_id(), TrackId(5));
}

#[test]
fn next_ids_saturate_instead_of_overflowing() {
    let mut project = valid_project();
    project.clips[0].id = ClipId(u64::MAX);
    project.timeline[1].id = TrackId(u64::MAX);
    assert_eq!(project.next_clip_id(), ClipId(u64::MAX));
    assert_eq!(project.next_track_id(), TrackId(u64::MAX));
}

fn event(time: f64, data: &str) -> Event {
    Event {
        time,
        event_type: EventType::Output,
        data: data.to_string(),
    }
}

/// A source of 5 s with one event at each whole second from 0 to 5.
fn source_with_events() -> Arc<Recording> {
    let mut source = Recording::test_fixture();
    source.duration = 5.0;
    source.events = (0..=5)
        .map(|t| event(f64::from(t), &format!("e{t}")))
        .collect();
    Arc::new(source)
}

fn clip_of(source: &Arc<Recording>, in_point: f64, out_point: f64, offset: f64) -> Clip {
    Clip {
        source: Arc::clone(source),
        ..clip(1, in_point, out_point, offset)
    }
}

fn times_and_data(clip: &Clip) -> Vec<(f64, String)> {
    clip.timeline_events().map(|e| (e.time, e.data)).collect()
}

#[test]
fn a_clip_holds_the_event_at_its_in_point_and_not_the_event_at_its_out_point() {
    let source = source_with_events();
    let clip = clip_of(&source, 1.0, 3.0, 0.0);
    let held: Vec<String> = clip.timeline_events().map(|e| e.data).collect();
    assert_eq!(held, ["e1", "e2"]);
}

#[test]
fn a_clip_that_ends_at_the_end_of_its_source_holds_the_event_at_that_time() {
    let source = source_with_events();
    let clip = clip_of(&source, 3.0, 5.0, 0.0);
    let held: Vec<String> = clip.timeline_events().map(|e| e.data).collect();
    assert_eq!(held, ["e3", "e4", "e5"]);
}

#[test]
fn two_clips_cut_at_one_time_hold_the_event_at_that_time_once() {
    let source = source_with_events();
    let first = clip_of(&source, 0.0, 2.0, 0.0);
    let second = clip_of(&source, 2.0, 5.0, 2.0);
    let count = |data: &str| {
        first
            .timeline_events()
            .chain(second.timeline_events())
            .filter(|e| e.data == data)
            .count()
    };
    assert_eq!(count("e2"), 1);
    assert_eq!(count("e5"), 1);
}

#[test]
fn a_clip_moves_event_times_onto_the_timeline() {
    let source = source_with_events();
    let clip = clip_of(&source, 1.0, 3.0, 10.0);
    assert_eq!(
        times_and_data(&clip),
        [(10.0, "e1".to_string()), (11.0, "e2".to_string())]
    );
}

#[test]
fn a_clip_passes_multi_byte_event_data_through_unchanged() {
    let mut source = Recording::test_fixture();
    source.duration = 2.0;
    source.events = vec![event(1.0, "日本語🎉")];
    let source = Arc::new(source);
    let clip = clip_of(&source, 0.0, 2.0, 0.5);
    assert_eq!(times_and_data(&clip), [(1.5, "日本語🎉".to_string())]);
}

/// Track 1 holds a clip of e1..e2 at 0 s (ends at 2 s). Track 2 holds a clip
/// of e3..e5 at 4 s (ends at 6 s).
fn two_clip_project() -> EditProject {
    let source = source_with_events();
    let mut first = clip_of(&source, 1.0, 3.0, 0.0);
    first.id = ClipId(1);
    let mut second = clip_of(&source, 3.0, 5.0, 4.0);
    second.id = ClipId(2);
    EditProject {
        clips: vec![first, second],
        timeline: vec![track(1, &[1]), track(2, &[2])],
        filters: Vec::new(),
        chapters: Vec::new(),
    }
}

#[test]
fn duration_is_the_end_of_the_last_clip_on_the_timeline() {
    assert_eq!(two_clip_project().duration(), 6.0);
}

#[test]
fn event_count_adds_the_events_of_each_clip() {
    // The first clip holds e1 and e2. The second holds e3, e4 and e5.
    assert_eq!(two_clip_project().event_count(), 5);
}

#[test]
fn an_empty_project_has_no_duration_and_no_events() {
    let project = EditProject::default();
    assert_eq!(project.duration(), 0.0);
    assert_eq!(project.event_count(), 0);
}

#[test]
fn a_project_survives_a_save_and_a_load_plain_and_gzip() {
    let dir = tempfile::tempdir().unwrap();
    let mut project = two_clip_project();
    project.filters.push(AppliedFilter {
        kind: FilterKind::new("speed"),
        region: FilterRegion::Range {
            start: Seconds::new(1.0).unwrap(),
            end: Seconds::new(2.0).unwrap(),
        },
        options: FilterOptions::new().with("factor", 2.0),
    });
    for compress in [false, true] {
        let path = dir.path().join(format!("project-{compress}.json"));

        project.save(&path, compress).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.starts_with(&[0x1f, 0x8b]), compress);
        assert_eq!(EditProject::load(&path).unwrap(), project);
    }
}

#[test]
fn clips_that_share_a_source_still_share_it_after_a_save_and_a_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.json");
    two_clip_project().save(&path, false).unwrap();

    let loaded = EditProject::load(&path).unwrap();

    assert!(std::sync::Arc::ptr_eq(
        &loaded.clips[0].source,
        &loaded.clips[1].source
    ));
}

#[test]
fn a_save_writes_a_shared_source_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("once.json");
    two_clip_project().save(&path, false).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();

    assert_eq!(text.matches("\"e3\"").count(), 1);
}

#[test]
fn load_refuses_a_clip_that_names_a_missing_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing-source.json");
    let mut value = serde_json::to_value(two_clip_project()).unwrap();
    value["clips"][1]["source"] = serde_json::json!(7);
    std::fs::write(&path, value.to_string()).unwrap();

    assert!(matches!(
        EditProject::load(&path),
        Err(NleError::InvalidFormat(_))
    ));
}

#[test]
fn load_refuses_a_file_that_parses_but_breaks_a_rule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.json");
    let mut project = valid_project();
    project.timeline[1].clips.clear();
    project.save(&path, false).unwrap();

    assert!(matches!(
        EditProject::load(&path),
        Err(NleError::InvalidProject(_))
    ));
}

#[test]
fn load_refuses_a_file_with_a_negative_in_point() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("negative.json");
    let mut value = serde_json::to_value(valid_project()).unwrap();
    value["clips"][0]["in_point"] = serde_json::json!(-1.0);
    std::fs::write(&path, value.to_string()).unwrap();

    assert!(matches!(
        EditProject::load(&path),
        Err(NleError::InvalidFormat(_))
    ));
}

// Tests of the chapter markers.

fn chapter(id: u64, time: f64, label: &str) -> ChapterMarker {
    ChapterMarker {
        id: ChapterId(id),
        time: Seconds::new(time).unwrap(),
        label: label.to_string(),
    }
}

#[test]
fn validate_refuses_two_chapters_with_one_id() {
    let mut project = valid_project();
    project.chapters = vec![chapter(7, 1.0, "one"), chapter(7, 2.0, "two")];

    let result = project.validate();

    match result {
        Err(NleError::InvalidProject(message)) => {
            assert!(message.contains('7'), "{message}");
            assert!(message.contains("chapter"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn validate_refuses_a_chapter_with_a_blank_label() {
    for label in ["", "  \t"] {
        let mut project = valid_project();
        project.chapters = vec![chapter(1, 1.0, label)];
        assert_invalid(&project, &format!("the label {label:?}"));
    }
}

#[test]
fn validate_accepts_a_chapter_after_the_end_of_the_timeline() {
    let mut project = valid_project();
    project.chapters = vec![chapter(1, project.duration() + 100.0, "late")];

    project.validate().unwrap();
}

#[test]
fn next_chapter_id_is_one_with_no_chapter_and_one_more_than_the_largest_otherwise() {
    let mut project = valid_project();
    assert_eq!(project.next_chapter_id(), ChapterId(1));

    project.chapters = vec![
        chapter(3, 0.0, "a"),
        chapter(9, 1.0, "b"),
        chapter(4, 2.0, "c"),
    ];
    assert_eq!(project.next_chapter_id(), ChapterId(10));
}

#[test]
fn next_chapter_id_saturates_at_the_largest_id() {
    let mut project = valid_project();
    project.chapters = vec![chapter(u64::MAX, 0.0, "a")];

    assert_eq!(project.next_chapter_id(), ChapterId(u64::MAX));
}

#[test]
fn chapters_in_order_sorts_by_time_and_then_by_id() {
    let mut project = valid_project();
    project.chapters = vec![
        chapter(5, 3.0, "late"),
        chapter(4, 1.0, "tie b"),
        chapter(2, 1.0, "tie a"),
        chapter(9, 0.5, "first"),
    ];

    let ids: Vec<u64> = project.chapters_in_order().iter().map(|c| c.id.0).collect();

    assert_eq!(ids, [9, 2, 4, 5]);
}

#[test]
fn chapters_survive_a_save_and_a_load_with_multibyte_labels() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chapters.json");
    let mut project = two_clip_project();
    project.chapters = vec![
        chapter(1, 0.5, "日本語テスト"),
        chapter(2, 1.5, "café 🎉"),
        chapter(3, 9.0, "plain"),
    ];

    project.save(&path, false).unwrap();

    assert_eq!(EditProject::load(&path).unwrap(), project);
}

#[test]
fn a_project_with_no_chapter_saves_no_chapters_key_and_loads_with_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.json");
    let project = two_clip_project();

    project.save(&path, false).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("\"chapters\""), "{text}");
    assert!(EditProject::load(&path).unwrap().chapters.is_empty());
}

#[test]
fn load_refuses_a_chapter_with_a_negative_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("negative-chapter.json");
    let mut project = two_clip_project();
    project.chapters = vec![chapter(1, 123.25, "x")];
    project.save(&path, false).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("123.25"), "{text}");
    std::fs::write(&path, text.replace("123.25", "-123.25")).unwrap();

    assert!(matches!(
        EditProject::load(&path),
        Err(NleError::InvalidFormat(_))
    ));
}
