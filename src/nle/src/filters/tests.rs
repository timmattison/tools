//! Tests of the registry of the filters that ship with the app.

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::types::{
    AppliedFilter, EditProject, Event, EventType, FilterKind, FilterRegion, Recording,
};

#[test]
fn the_app_registry_holds_the_cursor_filter() {
    assert!(registry().contains(&FilterKind::new(cursor::CURSOR)));
}

#[test]
fn a_project_with_the_cursor_filter_renders_through_the_app_registry() {
    let mut source = Recording::test_fixture();
    source.events = vec![Event {
        time: 1.0,
        event_type: EventType::Output,
        data: "\x1b[?25hx".to_string(),
    }];
    let mut project = EditProject::from_recording(Arc::new(source)).unwrap();
    project.filters.push(AppliedFilter {
        kind: FilterKind::new(cursor::CURSOR),
        region: FilterRegion::All,
        options: serde_json::from_value(json!({ "visibility": "always-hidden" })).unwrap(),
    });

    let rendered = registry().render(&project).unwrap();

    let data: Vec<(f64, &str)> = rendered
        .events
        .iter()
        .map(|event| (event.time, event.data.as_str()))
        .collect();
    assert_eq!(data, vec![(0.0, "\x1b[?25l"), (1.0, "x")]);
}

#[test]
fn the_app_registry_holds_the_color_scheme_filter() {
    assert!(registry().contains(&color_scheme::kind()));
}
