//! Tests of the backspace removal filter.

use super::*;
use crate::types::{AppliedFilter, Event, EventType, FilterRegion};

/// An output event.
fn out(time: f64, data: &str) -> Event {
    Event {
        time,
        event_type: EventType::Output,
        data: data.to_string(),
    }
}

/// An input event.
fn input(time: f64, data: &str) -> Event {
    Event {
        time,
        event_type: EventType::Input,
        data: data.to_string(),
    }
}

/// The filter on the region `All` of `events`.
fn applied_to_all() -> AppliedFilter {
    AppliedFilter {
        kind: kind(),
        region: FilterRegion::All,
        options: FilterOptions::new(),
    }
}

/// Run the filter on the region `All` of `events`, on a terminal of 80 by 24.
fn run(events: Vec<Event>) -> Vec<Event> {
    let mut registry = FilterRegistry::new();
    register(&mut registry);
    registry.run(events, &[applied_to_all()]).unwrap()
}

/// The events of the issue: a typo `r`, its backspace `key`, and the key `e`.
fn typo_then_backspace(key: &str) -> Vec<Event> {
    vec![
        out(0.0, "$ "),
        input(1.0, "h"),
        out(1.0, "h"),
        input(1.5, "r"),
        out(1.5, "r"),
        input(2.0, key),
        out(2.0, "\x08 \x08"),
        input(2.5, "e"),
        out(2.5, "e"),
    ]
}

/// The events of the issue without the typo and its backspace.
fn without_the_typo() -> Vec<Event> {
    vec![
        out(0.0, "$ "),
        input(1.0, "h"),
        out(1.0, "h"),
        input(1.5, "e"),
        out(1.5, "e"),
    ]
}

#[test]
fn the_app_registry_holds_the_backspace_removal_filter() {
    assert!(crate::filters::registry().contains(&FilterKind::new("backspace-removal")));
}

#[test]
fn a_typo_and_its_backspace_leave_the_recording() {
    assert_eq!(run(typo_then_backspace("\x7f")), without_the_typo());
}
