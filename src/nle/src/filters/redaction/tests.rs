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

fn one(data: &str, find: &str, replacement: &str) -> String {
    let got = run(vec![out(1.0, data)], &text(find, replacement));
    got.into_iter().map(|e| e.data).collect()
}

#[test]
fn escape_codes_around_a_match_stay() {
    assert_eq!(
        one("\x1b[31mhunter2\x1b[0m", "hunter2", "[R]"),
        "\x1b[31m[R]\x1b[0m"
    );
}

#[test]
fn text_that_looks_like_the_parameters_of_an_escape_code_is_not_found() {
    assert_eq!(one("\x1b[31mred", "31", "X"), "\x1b[31mred");
    assert_eq!(one("\x1b[31mred", "[31m", "X"), "\x1b[31mred");
}

#[test]
fn a_match_across_an_escape_code_keeps_the_code() {
    assert_eq!(
        one("hun\x1b[1mter2", "hunter2", "[REDACTED]"),
        "[REDACTED]\x1b[1m"
    );
}

#[test]
fn an_escape_code_split_across_events_is_one_code() {
    let got = run(vec![out(1.0, "\x1b[3"), out(1.1, "1m x")], &text("1m", "Z"));
    assert_eq!(datas(&got), vec!["\x1b[3", "1m x"]);
}

#[test]
fn an_osc_title_is_searched() {
    assert_eq!(
        one("\x1b]0;user@prod-db\x07", "prod-db", "HOST"),
        "\x1b]0;user@HOST\x07"
    );
    assert_eq!(
        one("\x1b]0;user@prod-db\x1b\\x", "prod-db", "HOST"),
        "\x1b]0;user@HOST\x1b\\x"
    );
}

#[test]
fn a_match_does_not_span_visible_text_and_an_osc_payload() {
    assert_eq!(one("ab\x1b]0;cd\x07", "b0", "X"), "ab\x1b]0;cd\x07");
    assert_eq!(one("ab\x1b]0;cd\x07", "bcd", "X"), "ab\x1b]0;cd\x07");
}

#[test]
fn the_content_of_a_device_control_string_is_not_searched() {
    assert_eq!(
        one("\x1bPqhunter2\x1b\\ hunter2", "hunter2", "X"),
        "\x1bPqhunter2\x1b\\ X"
    );
    assert_eq!(
        one("\x1b_Gkitty-hunter2\x1b\\", "hunter2", "X"),
        "\x1b_Gkitty-hunter2\x1b\\"
    );
}

#[test]
fn a_charset_escape_is_not_text() {
    assert_eq!(one("\x1b(Bhello", "B", "X"), "\x1b(Bhello");
    assert_eq!(one("\x1b(BBello", "B", "X"), "\x1b(BXello");
}

#[test]
fn a_bare_escape_does_not_swallow_the_text_after_it() {
    assert_eq!(one("\x1b\x1b\rhunter2", "hunter2", "X"), "\x1b\x1b\rX");
    assert_eq!(one("a\x1b", "a", "X"), "X\x1b");
}

#[test]
fn multi_byte_text_beside_escape_codes_is_replaced() {
    assert_eq!(
        one("\x1b[1m日本語\x1b[0m café", "日本語", "?"),
        "\x1b[1m?\x1b[0m café"
    );
}
