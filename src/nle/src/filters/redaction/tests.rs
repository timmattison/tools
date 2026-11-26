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

fn masked(find: Find, replacement: Replacement, events: Vec<Event>) -> Vec<Event> {
    run(events, &Redaction::new(find, replacement, false).unwrap())
}

fn find_text(text: &str) -> Find {
    Find::Text(text.to_string())
}

#[test]
fn asterisks_take_one_column_for_each_column_of_the_match() {
    let got = masked(
        find_text("hunter2"),
        Replacement::Asterisks,
        vec![out(1.0, "a hunter2 b")],
    );
    assert_eq!(datas(&got), vec!["a ******* b"]);
}

#[test]
fn blocks_take_one_column_for_each_column_of_the_match() {
    let got = masked(
        find_text("hunter2"),
        Replacement::Blocks,
        vec![out(1.0, "hunter2")],
    );
    assert_eq!(datas(&got), vec![&BLOCK.to_string().repeat(7)[..]]);
}

#[test]
fn a_wide_character_takes_two_columns() {
    let got = masked(
        find_text("パスワード"),
        Replacement::Asterisks,
        vec![out(1.0, "パスワード")],
    );
    assert_eq!(datas(&got), vec!["**********"]);
}

#[test]
fn accented_text_keeps_its_width() {
    let got = masked(
        find_text("café"),
        Replacement::Asterisks,
        vec![out(1.0, "un café!")],
    );
    assert_eq!(datas(&got), vec!["un ****!"]);
}

#[test]
fn a_combining_mark_adds_no_column() {
    let got = masked(
        find_text("cafe\u{301}"),
        Replacement::Asterisks,
        vec![out(1.0, "cafe\u{301}")],
    );
    assert_eq!(datas(&got), vec!["****"]);
}

#[test]
fn control_characters_inside_a_match_stay() {
    let got = masked(
        Find::Regex("a\r\nb".to_string()),
        Replacement::Asterisks,
        vec![out(1.0, "a\r\nb")],
    );
    assert_eq!(datas(&got), vec!["*\r\n*"]);
}

#[test]
fn a_match_across_events_is_masked_in_each_piece() {
    let got = masked(
        find_text("hunter2"),
        Replacement::Asterisks,
        vec![out(1.0, "hun"), out(1.1, "ter2")],
    );
    assert_eq!(datas(&got), vec!["***", "****"]);
}

#[test]
fn a_mask_skips_the_escape_codes_inside_a_match() {
    let got = masked(
        find_text("hunter2"),
        Replacement::Blocks,
        vec![out(1.0, "hun\x1b[1mter2")],
    );
    assert_eq!(
        datas(&got),
        vec![format!("{}\x1b[1m{}", "\u{2588}".repeat(3), "\u{2588}".repeat(4))]
    );
}

fn with_input(find: &str, replacement: &str, events: Vec<Event>) -> Vec<Event> {
    let redaction = Redaction::new(
        find_text(find),
        Replacement::Text(replacement.to_string()),
        true,
    )
    .unwrap();
    run(events, &redaction)
}

#[test]
fn input_events_are_redacted_when_the_option_says_so() {
    let got = with_input(
        "hunter2",
        "X",
        vec![out(1.0, "hunter2"), inp(2.0, "hunter2\r")],
    );
    assert_eq!(datas(&got), vec!["X", "X\r"]);
    assert_eq!(got[1].event_type, EventType::Input);
}

#[test]
fn input_events_stay_when_the_option_is_off() {
    let got = run(vec![inp(2.0, "hunter2\r")], &text("hunter2", "X"));
    assert_eq!(datas(&got), vec!["hunter2\r"]);
}

#[test]
fn output_and_input_are_separate_streams() {
    let got = with_input("hunter2", "X", vec![out(1.0, "hun"), inp(1.1, "ter2")]);
    assert_eq!(datas(&got), vec!["hun", "ter2"]);
}

#[test]
fn a_match_across_input_events_is_replaced() {
    let got = with_input(
        "hunter2",
        "X",
        vec![inp(1.0, "hun"), out(1.05, "echo"), inp(1.1, "ter2")],
    );
    assert_eq!(datas(&got), vec!["X", "echo", ""]);
}

fn refusal(find: Find, replacement: Replacement) -> String {
    match Redaction::new(find, replacement, false) {
        Err(NleError::InvalidRedaction(message)) => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn an_empty_pattern_is_refused() {
    let none = Replacement::Asterisks;
    assert_eq!(
        refusal(find_text(""), none.clone()),
        "the text to find is empty"
    );
    assert_eq!(
        refusal(Find::Regex(String::new()), none),
        "the text to find is empty"
    );
}

#[test]
fn a_regular_expression_that_does_not_compile_is_refused() {
    let message = refusal(Find::Regex("(unclosed".to_string()), Replacement::Asterisks);
    assert!(
        message.starts_with("the regular expression does not compile: "),
        "{message}"
    );
}

#[test]
fn a_replacement_with_a_control_character_is_refused() {
    for bad in ["a\u{1b}[2J", "a\nb", "\u{7}", "x\u{85}"] {
        assert_eq!(
            refusal(find_text("a"), Replacement::Text(bad.to_string())),
            "the replacement holds a control character",
            "{bad:?}"
        );
    }
}

#[test]
fn a_replacement_with_multi_byte_text_is_accepted() {
    let redaction = text("hunter2", "日本語 🎉");
    let got = run(vec![out(1.0, "hunter2")], &redaction);
    assert_eq!(datas(&got), vec!["日本語 🎉"]);
}

#[test]
fn a_regular_expression_finds_matches() {
    let redaction = Redaction::new(
        Find::Regex(r"\d{3}-\d{4}".to_string()),
        Replacement::Text("###".to_string()),
        false,
    )
    .unwrap();
    let got = run(vec![out(1.0, "call 555-1234 or 555-9876")], &redaction);
    assert_eq!(datas(&got), vec!["call ### or ###"]);
}

#[test]
fn only_the_redact_group_is_replaced() {
    let redaction = Redaction::new(
        Find::Regex(r"token=(?P<redact>\w+)".to_string()),
        Replacement::Asterisks,
        false,
    )
    .unwrap();
    let got = run(vec![out(1.0, "token=abc")], &redaction);
    assert_eq!(datas(&got), vec!["token=***"]);
}

#[test]
fn a_match_that_is_empty_changes_nothing() {
    let redaction = Redaction::new(
        Find::Regex("x*".to_string()),
        Replacement::Text("!".to_string()),
        false,
    )
    .unwrap();
    let got = run(vec![out(1.0, "abc")], &redaction);
    assert_eq!(datas(&got), vec!["abc"]);
}

#[test]
fn the_accessors_give_back_the_arguments() {
    let redaction = Redaction::new(find_text("a"), Replacement::Blocks, true).unwrap();
    assert_eq!(redaction.find(), &find_text("a"));
    assert_eq!(redaction.replacement(), &Replacement::Blocks);
    assert!(redaction.includes_input());
}
