//! Tests for the redaction filter.

use super::*;
use crate::filters;
use crate::types::{Event, EventType};

fn out(time: f64, data: &str) -> Event {
    Event {
        time,
        event_type: EventType::Output,
        data: data.to_string(),
    }
}

fn inp(time: f64, data: &str) -> Event {
    Event {
        time,
        event_type: EventType::Input,
        data: data.to_string(),
    }
}

fn text(find: &str, replacement: &str) -> Redaction {
    Redaction::new(
        Find::Text(find.to_string()),
        Replacement::Text(replacement.to_string()),
        false,
    )
    .unwrap()
}

fn run(events: Vec<Event>, redaction: &Redaction) -> Vec<Event> {
    filters::registry()
        .run(events, &[applied(FilterRegion::All, redaction)])
        .unwrap()
}

fn datas(events: &[Event]) -> Vec<&str> {
    events.iter().map(|e| e.data.as_str()).collect()
}

#[test]
fn the_registry_holds_the_redaction_filter() {
    assert!(filters::registry().contains(&kind()));
}

#[test]
fn text_is_replaced_in_one_output_event() {
    let got = run(
        vec![out(1.0, "token=hunter2\r\n")],
        &text("hunter2", "[REDACTED]"),
    );
    assert_eq!(datas(&got), vec!["token=[REDACTED]\r\n"]);
}

#[test]
fn input_events_and_times_stay() {
    let got = run(
        vec![out(1.0, "a hunter2"), inp(2.5, "hunter2"), out(4.0, "b")],
        &text("hunter2", "x"),
    );
    assert_eq!(datas(&got), vec!["a x", "hunter2", "b"]);
    assert_eq!(
        got.iter().map(|e| e.time).collect::<Vec<_>>(),
        vec![1.0, 2.5, 4.0]
    );
    assert_eq!(got[1].event_type, EventType::Input);
}

#[test]
fn a_match_split_across_two_output_events_is_replaced() {
    let got = run(
        vec![out(1.0, "hun"), out(1.1, "ter2")],
        &text("hunter2", "[REDACTED]"),
    );
    assert_eq!(datas(&got), vec!["[REDACTED]", ""]);
}

#[test]
fn an_input_event_between_two_output_events_does_not_break_a_match() {
    let got = run(
        vec![out(1.0, "hun"), inp(1.05, "x"), out(1.1, "ter2")],
        &text("hunter2", "[R]"),
    );
    assert_eq!(datas(&got), vec!["[R]", "x", ""]);
}

#[test]
fn multi_byte_text_split_across_events_is_replaced() {
    let got = run(
        vec![out(1.0, "日本"), out(1.1, "語 🎉 café")],
        &text("日本語", "?"),
    );
    assert_eq!(datas(&got), vec!["?", " 🎉 café"]);
}
