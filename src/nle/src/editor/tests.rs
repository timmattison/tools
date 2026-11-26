//! Tests for the edit operations and the editor.

use super::*;
use crate::types::{EventType, FilterKind, FilterOptions, FilterRegion, TimelineTrack};

fn secs(value: f64) -> Seconds {
    Seconds::new(value).unwrap()
}

fn source() -> Arc<Recording> {
    Arc::new(Recording::test_fixture())
}

fn clip(id: u64, source: &Arc<Recording>, in_point: f64, out_point: f64, offset: f64) -> Clip {
    Clip {
        id: ClipId(id),
        source: Arc::clone(source),
        in_point: secs(in_point),
        out_point: secs(out_point),
        timeline_offset: secs(offset),
    }
}

fn filter(name: &str) -> AppliedFilter {
    AppliedFilter {
        kind: FilterKind::new(name),
        region: FilterRegion::All,
        options: FilterOptions::new(),
    }
}

/// Clip 1 (0..5 of the fixture) on track 1, and an empty track 2.
fn project() -> EditProject {
    let source = source();
    EditProject {
        clips: vec![clip(1, &source, 0.0, 5.0, 0.0)],
        timeline: vec![
            TimelineTrack {
                id: TrackId(1),
                clips: vec![ClipId(1)],
            },
            TimelineTrack {
                id: TrackId(2),
                clips: vec![],
            },
        ],
        filters: vec![],
    }
}

fn registry() -> Arc<FilterRegistry> {
    let mut registry = FilterRegistry::new();
    registry.register(FilterKind::new("double"), |events, _| {
        events
            .iter()
            .map(|e| crate::types::Event {
                time: e.time * 2.0,
                event_type: EventType::Output,
                data: e.data.clone(),
            })
            .collect()
    });
    Arc::new(registry)
}

fn editor() -> Editor {
    Editor::new(project(), registry()).unwrap()
}

// Group 1: new.

#[test]
fn new_refuses_an_invalid_project() {
    let mut bad = project();
    bad.timeline[0].clips.push(ClipId(99));

    let result = Editor::new(bad, registry());

    assert!(
        matches!(result, Err(NleError::InvalidProject(_))),
        "{result:?}"
    );
}

#[test]
fn new_keeps_a_valid_project_with_no_history() {
    let editor = editor();

    assert_eq!(editor.project(), &project());
    assert!(!editor.can_undo());
    assert!(!editor.can_redo());
}

fn assert_edit_error(result: Result<(), NleError>) {
    assert!(matches!(result, Err(NleError::EditError(_))), "{result:?}");
}

fn assert_invalid_project(result: Result<(), NleError>) {
    assert!(
        matches!(result, Err(NleError::InvalidProject(_))),
        "{result:?}"
    );
}

// Group 2: track edits.

#[test]
fn add_track_appends_an_empty_track() {
    let mut editor = editor();

    editor.apply(Edit::AddTrack { id: TrackId(7) }).unwrap();

    let last = editor.project().timeline.last().unwrap();
    assert_eq!(last.id, TrackId(7));
    assert!(last.clips.is_empty());
    assert_eq!(editor.project().timeline.len(), 3);
}

#[test]
fn add_track_with_an_id_in_use_fails() {
    let mut editor = editor();

    assert_edit_error(editor.apply(Edit::AddTrack { id: TrackId(1) }));
}

#[test]
fn remove_track_removes_an_empty_track() {
    let mut editor = editor();

    editor.apply(Edit::RemoveTrack { id: TrackId(2) }).unwrap();

    let ids: Vec<TrackId> = editor.project().timeline.iter().map(|t| t.id).collect();
    assert_eq!(ids, vec![TrackId(1)]);
}

#[test]
fn remove_track_that_holds_a_clip_fails() {
    let mut editor = editor();

    assert_edit_error(editor.apply(Edit::RemoveTrack { id: TrackId(1) }));
}

#[test]
fn remove_track_with_a_missing_id_fails() {
    let mut editor = editor();

    assert_edit_error(editor.apply(Edit::RemoveTrack { id: TrackId(50) }));
}
