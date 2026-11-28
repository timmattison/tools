//! Tests for the edit operations and the editor.

use super::*;
use crate::export::{ExportProgress, ExportRequest, ExportTarget};
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

// Tests of `Editor::new`.

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

// Tests of track edits.

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

// Tests of clip edits.

fn clip_ids_of(project: &EditProject, track: u64) -> Vec<ClipId> {
    project
        .timeline
        .iter()
        .find(|t| t.id == TrackId(track))
        .unwrap()
        .clips
        .clone()
}

fn clip_in(project: &EditProject, id: u64) -> &Clip {
    project.clips.iter().find(|c| c.id == ClipId(id)).unwrap()
}

#[test]
fn add_clip_appends_to_the_clips_and_to_the_end_of_the_track() {
    let mut editor = editor();
    let new = clip(2, &source(), 1.0, 2.0, 3.0);

    editor
        .apply(Edit::AddClip {
            clip: new.clone(),
            track: TrackId(1),
        })
        .unwrap();

    assert_eq!(editor.project().clips.last().unwrap(), &new);
    assert_eq!(clip_ids_of(editor.project(), 1), vec![ClipId(1), ClipId(2)]);
}

#[test]
fn add_clip_with_an_id_in_use_fails() {
    let mut editor = editor();

    assert_edit_error(editor.apply(Edit::AddClip {
        clip: clip(1, &source(), 0.0, 1.0, 0.0),
        track: TrackId(2),
    }));
}

#[test]
fn add_clip_to_a_missing_track_fails() {
    let mut editor = editor();

    assert_edit_error(editor.apply(Edit::AddClip {
        clip: clip(2, &source(), 0.0, 1.0, 0.0),
        track: TrackId(50),
    }));
}

#[test]
fn remove_clip_removes_it_from_the_clips_and_from_its_track() {
    let mut editor = editor();

    editor.apply(Edit::RemoveClip { id: ClipId(1) }).unwrap();

    assert!(editor.project().clips.is_empty());
    assert!(clip_ids_of(editor.project(), 1).is_empty());
}

#[test]
fn remove_clip_with_a_missing_id_fails() {
    let mut editor = editor();

    assert_edit_error(editor.apply(Edit::RemoveClip { id: ClipId(50) }));
}

#[test]
fn trim_clip_sets_the_in_and_out_points() {
    let mut editor = editor();

    editor
        .apply(Edit::TrimClip {
            id: ClipId(1),
            in_point: secs(1.0),
            out_point: secs(3.0),
        })
        .unwrap();

    let trimmed = clip_in(editor.project(), 1);
    assert_eq!(trimmed.in_point, secs(1.0));
    assert_eq!(trimmed.out_point, secs(3.0));
}

#[test]
fn trim_clip_past_the_source_duration_fails_as_an_invalid_project() {
    let mut editor = editor();

    assert_invalid_project(editor.apply(Edit::TrimClip {
        id: ClipId(1),
        in_point: secs(0.0),
        out_point: secs(5.5),
    }));
}

#[test]
fn trim_clip_with_a_missing_id_fails() {
    let mut editor = editor();

    assert_edit_error(editor.apply(Edit::TrimClip {
        id: ClipId(50),
        in_point: secs(0.0),
        out_point: secs(1.0),
    }));
}

#[test]
fn move_clip_sets_the_offset_and_moves_it_to_the_end_of_the_track() {
    let mut editor = editor();
    editor
        .apply(Edit::AddClip {
            clip: clip(2, &source(), 0.0, 1.0, 0.0),
            track: TrackId(2),
        })
        .unwrap();

    editor
        .apply(Edit::MoveClip {
            id: ClipId(1),
            track: TrackId(2),
            timeline_offset: secs(4.0),
        })
        .unwrap();

    assert_eq!(clip_in(editor.project(), 1).timeline_offset, secs(4.0));
    assert!(clip_ids_of(editor.project(), 1).is_empty());
    assert_eq!(clip_ids_of(editor.project(), 2), vec![ClipId(2), ClipId(1)]);
}

#[test]
fn move_clip_to_its_own_track_puts_it_at_the_end() {
    let mut editor = editor();
    editor
        .apply(Edit::AddClip {
            clip: clip(2, &source(), 0.0, 1.0, 0.0),
            track: TrackId(1),
        })
        .unwrap();

    editor
        .apply(Edit::MoveClip {
            id: ClipId(1),
            track: TrackId(1),
            timeline_offset: secs(2.0),
        })
        .unwrap();

    assert_eq!(clip_ids_of(editor.project(), 1), vec![ClipId(2), ClipId(1)]);
}

#[test]
fn move_clip_with_a_missing_clip_or_track_fails() {
    let mut editor = editor();

    assert_edit_error(editor.apply(Edit::MoveClip {
        id: ClipId(50),
        track: TrackId(1),
        timeline_offset: secs(0.0),
    }));
    assert_edit_error(editor.apply(Edit::MoveClip {
        id: ClipId(1),
        track: TrackId(50),
        timeline_offset: secs(0.0),
    }));
}

// Tests of filter edits.

fn filter_names(project: &EditProject) -> Vec<&str> {
    project.filters.iter().map(|f| f.kind.as_str()).collect()
}

fn editor_with_one_filter() -> Editor {
    let mut editor = editor();
    editor
        .apply(Edit::AddFilter {
            index: 0,
            filter: filter("a"),
        })
        .unwrap();
    editor
}

#[test]
fn add_filter_inserts_at_the_front_and_at_the_end() {
    let mut editor = editor_with_one_filter();

    editor
        .apply(Edit::AddFilter {
            index: 0,
            filter: filter("front"),
        })
        .unwrap();
    editor
        .apply(Edit::AddFilter {
            index: 2,
            filter: filter("back"),
        })
        .unwrap();

    assert_eq!(filter_names(editor.project()), vec!["front", "a", "back"]);
}

#[test]
fn add_filter_past_the_end_fails() {
    let mut editor = editor_with_one_filter();

    assert_edit_error(editor.apply(Edit::AddFilter {
        index: 2,
        filter: filter("b"),
    }));
}

#[test]
fn remove_filter_removes_at_a_valid_index_and_fails_past_the_end() {
    let mut editor = editor_with_one_filter();

    assert_edit_error(editor.apply(Edit::RemoveFilter { index: 1 }));
    editor.apply(Edit::RemoveFilter { index: 0 }).unwrap();

    assert!(editor.project().filters.is_empty());
}

#[test]
fn replace_filter_replaces_at_a_valid_index_and_fails_past_the_end() {
    let mut editor = editor_with_one_filter();

    assert_edit_error(editor.apply(Edit::ReplaceFilter {
        index: 1,
        filter: filter("b"),
    }));
    editor
        .apply(Edit::ReplaceFilter {
            index: 0,
            filter: filter("b"),
        })
        .unwrap();

    assert_eq!(filter_names(editor.project()), vec!["b"]);
}

#[test]
fn add_filter_with_a_backwards_range_fails_as_an_invalid_project() {
    let mut editor = editor();

    assert_invalid_project(editor.apply(Edit::AddFilter {
        index: 0,
        filter: AppliedFilter {
            kind: FilterKind::new("a"),
            region: FilterRegion::Range {
                start: secs(3.0),
                end: secs(1.0),
            },
            options: FilterOptions::new(),
        },
    }));
}

// Tests of undo and redo.

fn add_track(editor: &mut Editor, id: u64) {
    editor.apply(Edit::AddTrack { id: TrackId(id) }).unwrap();
}

#[test]
fn undo_restores_the_exact_project_before_the_edit() {
    let mut editor = editor();
    let before = editor.project().clone();
    editor
        .apply(Edit::TrimClip {
            id: ClipId(1),
            in_point: secs(1.0),
            out_point: secs(2.0),
        })
        .unwrap();

    assert!(editor.undo());

    assert_eq!(editor.project(), &before);
    assert!(!editor.can_undo());
    assert!(editor.can_redo());
}

#[test]
fn redo_replays_the_edit_to_the_exact_project_after_it() {
    let mut editor = editor();
    add_track(&mut editor, 7);
    let after = editor.project().clone();
    assert!(editor.undo());

    assert!(editor.redo().unwrap());

    assert_eq!(editor.project(), &after);
    assert!(editor.can_undo());
    assert!(!editor.can_redo());
}

#[test]
fn three_edits_undo_in_reverse_order_and_redo_in_order() {
    let mut editor = editor();
    let mut states = vec![editor.project().clone()];
    for id in [10, 11, 12] {
        add_track(&mut editor, id);
        states.push(editor.project().clone());
    }

    for expected in states.iter().rev().skip(1) {
        assert!(editor.undo());
        assert_eq!(editor.project(), expected);
    }
    assert!(!editor.undo());
    for expected in states.iter().skip(1) {
        assert!(editor.redo().unwrap());
        assert_eq!(editor.project(), expected);
    }
    assert!(!editor.redo().unwrap());
}

#[test]
fn undo_with_nothing_to_undo_is_false() {
    let mut editor = editor();

    assert!(!editor.undo());
    assert_eq!(editor.project(), &project());
}

#[test]
fn redo_with_nothing_to_redo_is_ok_false() {
    let mut editor = editor();

    assert!(!editor.redo().unwrap());
}

#[test]
fn a_new_edit_after_an_undo_clears_the_redo_history() {
    let mut editor = editor();
    add_track(&mut editor, 7);
    assert!(editor.undo());
    assert!(editor.can_redo());

    add_track(&mut editor, 8);

    assert!(!editor.can_redo());
    assert!(!editor.redo().unwrap());
}

// A refused edit changes nothing.

#[test]
fn a_refused_edit_changes_neither_the_project_nor_the_history() {
    let mut editor = editor();
    add_track(&mut editor, 7);
    add_track(&mut editor, 8);
    assert!(editor.undo());
    let project_before = editor.project().clone();

    // An EditError and an InvalidProject.
    assert_edit_error(editor.apply(Edit::RemoveClip { id: ClipId(50) }));
    assert_invalid_project(editor.apply(Edit::TrimClip {
        id: ClipId(1),
        in_point: secs(0.0),
        out_point: secs(9.0),
    }));

    assert_eq!(editor.project(), &project_before);
    assert!(editor.can_undo());
    assert!(editor.can_redo());
    assert!(editor.redo().unwrap());
    assert_eq!(
        editor.project().timeline.last().map(|t| t.id),
        Some(TrackId(8))
    );
}

// Tests of `Editor::render`.

#[test]
fn render_equals_the_registry_render_of_an_edited_project() {
    let mut editor = editor();
    editor
        .apply(Edit::TrimClip {
            id: ClipId(1),
            in_point: secs(0.0),
            out_point: secs(3.0),
        })
        .unwrap();
    editor
        .apply(Edit::AddFilter {
            index: 0,
            filter: filter("double"),
        })
        .unwrap();

    let rendered = editor.render().unwrap();

    assert_eq!(*rendered, registry().render(editor.project()).unwrap());
    // The filter doubled the time of the event at 1.5.
    assert!(rendered.events.iter().any(|e| e.time == 3.0));
}

// An edit never changes a source.

#[test]
fn edits_renders_and_undo_never_change_the_source() {
    let held = source();
    let deep_copy: Recording = (*held).clone();
    let project = EditProject {
        clips: vec![clip(1, &held, 0.0, 5.0, 0.0)],
        timeline: vec![TimelineTrack {
            id: TrackId(1),
            clips: vec![ClipId(1)],
        }],
        filters: vec![],
    };
    let mut editor = Editor::new(project, registry()).unwrap();

    editor
        .apply(Edit::TrimClip {
            id: ClipId(1),
            in_point: secs(1.0),
            out_point: secs(4.0),
        })
        .unwrap();
    editor.apply(Edit::AddTrack { id: TrackId(2) }).unwrap();
    editor
        .apply(Edit::MoveClip {
            id: ClipId(1),
            track: TrackId(2),
            timeline_offset: secs(2.0),
        })
        .unwrap();
    editor
        .apply(Edit::AddFilter {
            index: 0,
            filter: filter("double"),
        })
        .unwrap();
    editor.render().unwrap();
    while editor.undo() {}

    assert_eq!(*held, deep_copy);
    assert!(Arc::ptr_eq(&editor.project().clips[0].source, &held));
    assert_eq!(editor.project().clips[0].in_point, secs(0.0));
}

// Tests of `Editor::save_recording`.

fn trimmed_editor() -> Editor {
    let mut editor = editor();
    editor
        .apply(Edit::TrimClip {
            id: ClipId(1),
            in_point: secs(0.0),
            out_point: secs(1.0),
        })
        .unwrap();
    editor
}

#[tokio::test]
async fn save_recording_writes_the_rendered_recording() {
    let editor = trimmed_editor();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out.json");

    editor
        .save_recording(file.to_string_lossy().to_string(), None)
        .await
        .unwrap();

    let loaded = Recording::load(&file).unwrap();
    assert_eq!(loaded, *editor.render().unwrap());
    assert_ne!(loaded, Recording::test_fixture());
}

#[tokio::test]
async fn the_save_future_does_not_borrow_the_editor() {
    let editor = trimmed_editor();
    let expected = editor.render().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out.json");

    let future = editor.save_recording(file.to_string_lossy().to_string(), None);
    drop(editor);
    future.await.unwrap();

    assert_eq!(Recording::load(&file).unwrap(), *expected);
}

#[tokio::test]
async fn save_recording_gzips_a_gz_name_when_compress_is_absent() {
    let editor = trimmed_editor();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out.json.gz");

    editor
        .save_recording(file.to_string_lossy().to_string(), None)
        .await
        .unwrap();

    assert!(std::fs::read(&file).unwrap().starts_with(&[0x1f, 0x8b]));
}

#[tokio::test]
async fn save_recording_refuses_a_txt_name() {
    let editor = trimmed_editor();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out.txt");

    let result = editor
        .save_recording(file.to_string_lossy().to_string(), None)
        .await;

    assert!(matches!(result, Err(NleError::SaveError(_))), "{result:?}");
    assert!(!file.exists());
}

// Tests of `Editor::export`.

fn recording_request(file: &std::path::Path) -> ExportRequest {
    ExportRequest {
        output_path: file.to_path_buf(),
        target: ExportTarget::Recording {
            compress: Some(false),
        },
    }
}

#[tokio::test]
async fn export_reports_rendering_first_and_writes_the_rendered_recording() {
    let editor = trimmed_editor();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out.json");
    let reported = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let collected = Arc::clone(&reported);

    let result = editor
        .export(recording_request(&file), move |progress| {
            collected.lock().unwrap().push(progress);
        })
        .await;

    assert_eq!(
        *reported.lock().unwrap(),
        [ExportProgress::Rendering, ExportProgress::Writing]
    );
    assert_eq!(result.unwrap(), file);
    assert_eq!(Recording::load(&file).unwrap(), *editor.render().unwrap());
}

#[tokio::test]
async fn the_export_future_does_not_borrow_the_editor() {
    let editor = trimmed_editor();
    let expected = editor.render().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out.json");

    let future = editor.export(recording_request(&file), |_| {});
    drop(editor);
    future.await.unwrap();

    assert_eq!(Recording::load(&file).unwrap(), *expected);
}

#[tokio::test]
async fn export_writes_the_render_with_the_filters_of_the_project() {
    let mut editor = dilation_editor();
    editor.apply(dilation(10.0, 2.0)).unwrap();
    let unfiltered = dilation_editor().render().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out.json");

    editor
        .export(recording_request(&file), |_| {})
        .await
        .unwrap();

    let written = Recording::load(&file).unwrap();
    assert_eq!(written, *editor.render().unwrap());
    assert_ne!(written.duration, unfiltered.duration);
}

// Tests of `Editor::apply_renderable`.

fn dilation(end: f64, speed: f64) -> Edit {
    use crate::filters::time_dilation::{applied, Speed};
    Edit::AddFilter {
        index: 0,
        filter: applied(
            FilterRegion::Range {
                start: secs(0.0),
                end: secs(end),
            },
            Speed::new(speed).unwrap(),
        ),
    }
}

fn dilation_editor() -> Editor {
    Editor::new(project(), Arc::new(crate::filters::registry())).unwrap()
}

#[test]
fn apply_renderable_refuses_a_filter_that_does_not_render_and_keeps_the_history() {
    let mut editor = dilation_editor();
    editor.apply(Edit::AddTrack { id: TrackId(3) }).unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let could_undo = editor.can_undo();

    let result = editor.apply_renderable(dilation(10.0, 1e-320));

    assert!(
        matches!(result, Err(NleError::RenderError(_))),
        "{result:?}"
    );
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.can_undo(), could_undo);
    assert!(editor.can_redo());
}

#[test]
fn apply_renderable_applies_a_filter_that_renders_and_clears_the_redo_history() {
    let mut editor = dilation_editor();
    editor.apply(Edit::AddTrack { id: TrackId(3) }).unwrap();
    assert!(editor.undo());
    assert!(editor.can_redo());

    editor.apply_renderable(dilation(10.0, 2.0)).unwrap();

    assert_eq!(editor.project().filters.len(), 1);
    assert!(editor.can_undo());
    assert!(!editor.can_redo());
}

// Tests of the marks of the render.

/// The kind of the filter of `marking_registry`.
const MARK_KIND: &str = "mark";

/// A registry with one span filter that keeps its span and adds a mark at
/// 1 s, and the count of the renders that ran that filter.
fn marking_registry() -> (Arc<FilterRegistry>, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let renders = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&renders);
    let mut registry = FilterRegistry::new();
    registry.register_span(FilterKind::new(MARK_KIND), move |mut span, _, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        span.marks.push(Mark {
            time: 1.0,
            label: "m".to_string(),
        });
        Ok(span)
    });
    (Arc::new(registry), renders)
}

fn add_mark() -> Edit {
    Edit::AddFilter {
        index: 0,
        filter: filter(MARK_KIND),
    }
}

fn render_count(renders: &std::sync::atomic::AtomicUsize) -> usize {
    renders.load(std::sync::atomic::Ordering::SeqCst)
}

#[test]
fn rendered_marks_gives_the_marks_of_the_render() {
    let (filters, _) = marking_registry();
    let mut editor = Editor::new(project(), filters).unwrap();

    editor.apply_renderable(add_mark()).unwrap();

    assert_eq!(
        editor.rendered_marks().unwrap(),
        vec![Mark {
            time: 1.0,
            label: "m".to_string()
        }]
    );
}

#[test]
fn an_edit_that_renders_gives_its_duration_and_its_marks_in_one_render() {
    let (filters, renders) = marking_registry();
    let mut editor = Editor::new(project(), filters).unwrap();

    editor.apply_renderable(add_mark()).unwrap();
    editor.rendered_duration().unwrap();
    editor.rendered_marks().unwrap();

    assert_eq!(render_count(&renders), 1);
}

#[test]
fn the_recording_for_the_preview_after_an_edit_that_renders_makes_no_second_render() {
    let (filters, renders) = marking_registry();
    let mut editor = Editor::new(project(), filters).unwrap();

    editor.apply_renderable(add_mark()).unwrap();
    let recording = editor.render().unwrap();

    assert_eq!(render_count(&renders), 1);
    assert_eq!(recording.duration, editor.rendered_duration().unwrap());
}

#[test]
fn after_an_undo_one_render_gives_the_duration_and_the_recording_for_the_preview() {
    let (filters, renders) = marking_registry();
    let mut editor = Editor::new(project(), filters).unwrap();
    editor.apply_renderable(add_mark()).unwrap();
    editor.apply(Edit::AddTrack { id: TrackId(3) }).unwrap();
    assert!(editor.undo());

    editor.rendered_duration().unwrap();
    editor.render().unwrap();
    editor.render().unwrap();

    assert_eq!(render_count(&renders), 2);
}

#[test]
fn after_an_edit_with_no_render_one_render_gives_the_duration_and_the_marks() {
    let (filters, renders) = marking_registry();
    let mut editor = Editor::new(project(), filters).unwrap();
    editor.apply_renderable(add_mark()).unwrap();
    editor.apply(Edit::AddTrack { id: TrackId(3) }).unwrap();

    editor.rendered_marks().unwrap();
    editor.rendered_duration().unwrap();
    editor.rendered_marks().unwrap();

    assert_eq!(render_count(&renders), 2);
    assert_eq!(editor.rendered_marks().unwrap().len(), 1);
}

#[test]
fn an_undo_gives_the_marks_of_the_project_before_the_edit() {
    let (filters, _) = marking_registry();
    let mut editor = Editor::new(project(), filters).unwrap();
    editor.apply_renderable(add_mark()).unwrap();
    assert_eq!(editor.rendered_marks().unwrap().len(), 1);

    assert!(editor.undo());

    assert!(editor.rendered_marks().unwrap().is_empty());
    assert!(editor.redo().unwrap());
    assert_eq!(editor.rendered_marks().unwrap().len(), 1);
}

#[test]
fn a_render_that_fails_is_not_kept_and_the_next_read_renders_again() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let renders = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&renders);
    let mut registry = FilterRegistry::new();
    registry.register_span(FilterKind::new("flaky"), move |mut span, _, _| {
        if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err("the first render fails".to_string());
        }
        span.marks.push(Mark {
            time: 1.0,
            label: "m".to_string(),
        });
        Ok(span)
    });
    let mut editor = Editor::new(project(), Arc::new(registry)).unwrap();
    editor
        .apply(Edit::AddFilter {
            index: 0,
            filter: filter("flaky"),
        })
        .unwrap();

    assert!(editor.rendered_marks().is_err());

    assert_eq!(
        editor.rendered_marks().unwrap(),
        vec![Mark {
            time: 1.0,
            label: "m".to_string()
        }]
    );
    assert_eq!(render_count(&renders), 2);
}

#[tokio::test]
async fn an_export_after_a_kept_render_writes_that_render_with_no_second_render() {
    let (filters, renders) = marking_registry();
    let mut editor = Editor::new(project(), filters).unwrap();
    editor.apply_renderable(add_mark()).unwrap();
    let kept = editor.render().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out.json");

    editor
        .export(recording_request(&file), |_| {})
        .await
        .unwrap();

    assert_eq!(render_count(&renders), 1);
    assert_eq!(Recording::load(&file).unwrap(), *kept);
}

#[tokio::test]
async fn an_export_of_a_kept_render_writes_that_render_with_no_copy() {
    let (filters, _renders) = marking_registry();
    let mut editor = Editor::new(project(), filters).unwrap();
    editor.apply_renderable(add_mark()).unwrap();
    let kept = Arc::downgrade(&editor.render().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("out.json");
    let holders = Arc::new(std::sync::Mutex::new(None));
    let seen = Arc::clone(&holders);

    editor
        .export(recording_request(&file), move |progress| {
            if matches!(progress, ExportProgress::Writing) {
                *seen.lock().unwrap() = Some(kept.strong_count());
            }
        })
        .await
        .unwrap();

    // The editor holds the kept render. A second holder at the write is the
    // export, which thus writes that render and not a copy of it.
    let holders = *holders.lock().unwrap();
    assert!(
        matches!(holders, Some(count) if count > 1),
        "holders of the kept render at the write: {holders:?}"
    );
}
