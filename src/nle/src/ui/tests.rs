//! Tests of the regions of the window. Each test renders the components to
//! text with the Dioxus SSR renderer and reads that text.

use super::*;
use crate::format::format_recorded;
use crate::types::{Recording, RecordingMetadata};
use std::collections::BTreeMap;

const FIXTURE_PATH: &str = "/home/user/recordings/demo.json";
const FIXTURE_TIMESTAMP: f64 = 1_700_000_000.0;

/// A recording with the metadata of the React tests: 65.3 seconds, 42 events,
/// 120x40 cells.
fn fixture() -> LoadedRecording {
    LoadedRecording {
        recording: Recording {
            version: 2,
            width: 120,
            height: 40,
            timestamp: FIXTURE_TIMESTAMP,
            duration: 65.3,
            command: "echo hello".into(),
            title: "Demo Recording".into(),
            env: BTreeMap::new(),
            events: Vec::new(),
        },
        metadata: RecordingMetadata {
            file_path: FIXTURE_PATH.into(),
            title: "Demo Recording".into(),
            command: "echo hello".into(),
            duration: 65.3,
            event_count: 42,
            width: 120,
            height: 40,
            timestamp: FIXTURE_TIMESTAMP,
        },
    }
}

/// Renders `root` to text. The recording goes into the root context.
fn render_panels(recording: LoadedRecording) -> String {
    let mut dom = VirtualDom::new(panels_root).with_root_context(recording);
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

/// A test root that shows the three panels of a recording from the context.
fn panels_root() -> Element {
    let recording = use_context::<LoadedRecording>();
    let loaded = use_signal(|| Some(recording));
    rsx! {
        PreviewPane { loaded }
        Timeline { loaded }
        PropertiesPanel { loaded }
    }
}

/// Renders the `App` with no recording.
fn render_empty_app() -> String {
    let mut dom = VirtualDom::new(App);
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

#[test]
fn app_with_no_recording_shows_the_empty_regions() {
    let page = render_empty_app();
    assert!(page.contains("Open Recording"), "{page}");
    assert!(
        page.contains("Open a recording to preview it here"),
        "{page}"
    );
    assert!(page.contains("Properties"), "{page}");
    assert_eq!(page.matches("No recording loaded").count(), 2, "{page}");
    assert!(!page.contains("role=\"alert\""), "{page}");
}

#[test]
fn app_renders_the_style_sheet_first() {
    let page = render_empty_app();
    assert!(page.contains(".app-layout"), "{page}");
    assert!(
        page.find("<style")
            .is_some_and(|at| at < page.find("menu-bar\"").unwrap_or(0)),
        "{page}"
    );
}

#[test]
fn properties_show_the_metadata() {
    let page = render_panels(fixture());
    for text in [
        "Demo Recording",
        "echo hello",
        "1m 5.3s",
        "42",
        "120x40",
        "demo.json",
    ] {
        assert!(page.contains(text), "{text} missing in {page}");
    }
    assert!(page.contains(&format_recorded(FIXTURE_TIMESTAMP)), "{page}");
    assert!(
        page.contains(&format!("title=\"{FIXTURE_PATH}\"")),
        "{page}"
    );
}

#[test]
fn properties_show_untitled_for_an_empty_title() {
    let mut recording = fixture();
    recording.metadata.title = String::new();
    let page = render_panels(recording);
    assert!(page.contains("(untitled)"), "{page}");
}

#[test]
fn properties_show_the_file_name_of_a_windows_path() {
    let mut recording = fixture();
    recording.metadata.file_path = r"C:\Users\test\recordings\demo.json".into();
    let page = render_panels(recording);
    assert!(page.contains(">demo.json<"), "{page}");
}

#[test]
fn timeline_shows_the_event_count_and_the_duration() {
    let page = render_panels(fixture());
    assert!(page.contains("42 events"), "{page}");
    assert!(page.contains("Timeline editor"), "{page}");
}

#[test]
fn timeline_rounds_the_duration_to_tenths() {
    for (seconds, expected) in [(125.5, "2m 5.5s"), (59.96, "1m 0.0s")] {
        let mut recording = fixture();
        recording.recording.duration = seconds;
        let page = render_panels(recording);
        assert!(
            page.contains(&format!("| {expected}")),
            "{expected} missing in {page}"
        );
    }
}

#[test]
fn preview_shows_the_size_of_the_recording() {
    let page = render_panels(fixture());
    assert!(page.contains("coming in #151"), "{page}");
    assert!(page.contains("120x40"), "{page}");
}
