//! Tests for the filter pipeline and the render.

use super::*;
use crate::types::{EventType, FilterRegion, Seconds};

fn ev(time: f64) -> Event {
    Event {
        time,
        event_type: EventType::Output,
        data: format!("e{time}"),
    }
}

fn events(times: &[f64]) -> Vec<Event> {
    times.iter().copied().map(ev).collect()
}

fn times(events: &[Event]) -> Vec<f64> {
    events.iter().map(|e| e.time).collect()
}

fn kind(name: &str) -> FilterKind {
    FilterKind::new(name)
}

fn applied(name: &str, region: FilterRegion) -> AppliedFilter {
    AppliedFilter {
        kind: kind(name),
        region,
        options: FilterOptions::new(),
    }
}

fn all(name: &str) -> AppliedFilter {
    applied(name, FilterRegion::All)
}

fn range(name: &str, start: f64, end: f64) -> AppliedFilter {
    applied(
        name,
        FilterRegion::Range {
            start: Seconds::new(start).unwrap(),
            end: Seconds::new(end).unwrap(),
        },
    )
}

/// Moves each event to `map(time)`.
fn mapper(map: fn(f64) -> f64) -> impl Fn(&[Event], &FilterOptions) -> Vec<Event> + Send + Sync {
    move |events, _| {
        events
            .iter()
            .map(|e| Event {
                time: map(e.time),
                ..e.clone()
            })
            .collect()
    }
}

fn double() -> impl Fn(&[Event], &FilterOptions) -> Vec<Event> + Send + Sync {
    mapper(|t| t * 2.0)
}

fn add_one() -> impl Fn(&[Event], &FilterOptions) -> Vec<Event> + Send + Sync {
    mapper(|t| t + 1.0)
}

// Group 1: the registry.

#[test]
fn contains_is_false_before_and_true_after_register() {
    let mut registry = FilterRegistry::new();
    assert!(!registry.contains(&kind("double")));

    registry.register(kind("double"), double());

    assert!(registry.contains(&kind("double")));
    assert!(!registry.contains(&kind("other")));
}

#[test]
fn a_second_register_under_one_kind_replaces_the_first() {
    let mut registry = FilterRegistry::new();
    registry.register(kind("f"), double());
    registry.register(kind("f"), add_one());

    let out = registry.run(events(&[1.0, 2.0]), &[all("f")]).unwrap();

    assert_eq!(times(&out), vec![2.0, 3.0]);
}

#[test]
fn run_with_an_unregistered_kind_fails_and_names_the_kind() {
    let registry = FilterRegistry::new();

    let result = registry.run(events(&[1.0]), &[all("wobble")]);

    match result {
        Err(NleError::RenderError(message)) => assert!(message.contains("wobble"), "{message}"),
        other => panic!("expected a RenderError, got {other:?}"),
    }
}
