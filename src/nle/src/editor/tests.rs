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
