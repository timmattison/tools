//! Tests of the cursor filter.

use serde_json::json;

use super::*;
use crate::error::NleError;
use crate::types::{AppliedFilter, EventType, FilterRegion, Seconds};

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

/// The options that `value`, a JSON object, gives.
fn options(value: serde_json::Value) -> FilterOptions {
    serde_json::from_value(value).unwrap()
}

fn range(start: f64, end: f64) -> FilterRegion {
    FilterRegion::Range {
        start: Seconds::new(start).unwrap(),
        end: Seconds::new(end).unwrap(),
    }
}

/// Run the cursor filter with `options` on `region` of `events`.
fn run(
    events: Vec<Event>,
    options: serde_json::Value,
    region: FilterRegion,
) -> Result<Vec<Event>, NleError> {
    let mut registry = FilterRegistry::new();
    register(&mut registry);
    registry.run(
        events,
        &[AppliedFilter {
            kind: FilterKind::new(CURSOR),
            region,
            options: self::options(options),
        }],
    )
}

/// The render error of `options`, or a panic.
fn refusal(options: serde_json::Value) -> String {
    match run(vec![out(0.0, "hi")], options.clone(), FilterRegion::All) {
        Err(NleError::RenderError(message)) => message,
        other => panic!("{options}: expected a RenderError, got {other:?}"),
    }
}

// Tests of the options.

#[test]
fn no_options_give_the_events_back_unchanged() {
    let events = vec![
        out(0.0, "\x1b[?25l\x1b[5 q\x1b]12;#ff0000\x07hi"),
        input(0.5, "q"),
        out(1.0, "\x1b[?25h"),
    ];

    for value in [json!({}), json!({ "visibility": "honor" })] {
        assert_eq!(
            run(events.clone(), value, FilterRegion::All).unwrap(),
            events
        );
    }
}

#[test]
fn an_unknown_option_is_refused_and_named() {
    let message = refusal(json!({ "colour": "#ffffff" }));

    assert!(message.contains("\"cursor\""), "{message}");
    assert!(message.contains("colour"), "{message}");
}

#[test]
fn a_value_that_an_option_does_not_know_is_refused_and_named() {
    for (value, word) in [
        (json!({ "visibility": "sometimes" }), "sometimes"),
        (json!({ "shape": "triangle" }), "triangle"),
        (json!({ "blink": "yes" }), "yes"),
        (json!({ "color": "red" }), "red"),
        (json!({ "color": "#12345" }), "#12345"),
        (json!({ "color": "#12345g" }), "#12345g"),
        (json!({ "color": "#日本語日本" }), "#日本語日本"),
    ] {
        let message = refusal(value.clone());

        assert!(message.contains("\"cursor\""), "{value}: {message}");
        assert!(message.contains(word), "{value}: {message}");
    }
}

#[test]
fn a_burst_gap_is_refused_unless_the_visibility_is_smart() {
    let message = refusal(json!({ "visibility": "always-hidden", "burst_gap": 0.2 }));

    assert!(message.contains("burst_gap"), "{message}");
    assert!(message.contains("smart"), "{message}");
}

#[test]
fn a_burst_gap_must_be_above_zero() {
    for gap in [json!(0.0), json!(-0.1)] {
        let message = refusal(json!({ "visibility": "smart", "burst_gap": gap }));

        assert!(message.contains("\"cursor\""), "{gap}: {message}");
    }
}

// Tests of always-visible and always-hidden.

const SHOW: &str = "\x1b[?25h";
const HIDE: &str = "\x1b[?25l";

#[test]
fn always_hidden_hides_the_cursor_at_the_start_and_drops_each_change() {
    let events = vec![
        out(0.0, "日本\x1b[?25h語"),
        input(0.5, "\x1b[?25h"),
        out(1.0, "\x1b[?25l\x1b[?25h c"),
    ];

    let got = run(
        events,
        json!({ "visibility": "always-hidden" }),
        FilterRegion::All,
    )
    .unwrap();

    // The input event goes to the program, not to the screen, so it stays.
    assert_eq!(
        got,
        vec![
            out(0.0, HIDE),
            out(0.0, "日本語"),
            input(0.5, "\x1b[?25h"),
            out(1.0, " c"),
        ]
    );
}

#[test]
fn always_visible_keeps_the_other_modes_of_a_shared_sequence() {
    let events = vec![
        out(0.0, "\x1b[?1049;25l"),
        out(1.0, "\x1b[?025;2004h"),
        out(2.0, "\u{9b}?25l\x1b[?2\r5la"),
    ];

    let got = run(
        events,
        json!({ "visibility": "always-visible" }),
        FilterRegion::All,
    )
    .unwrap();

    // A control inside a dropped sequence stays where the terminal runs it.
    assert_eq!(
        got,
        vec![
            out(0.0, SHOW),
            out(0.0, "\x1b[?1049l"),
            out(1.0, "\x1b[?2004h"),
            out(2.0, "\ra"),
        ]
    );
}

#[test]
fn the_cursor_gets_the_state_of_the_recording_back_at_the_end_of_a_range() {
    let events = vec![
        out(0.0, "a"),
        out(1.0, "\x1b[?25lb"),
        out(1.5, "c"),
        out(3.0, "d"),
    ];

    let got = run(
        events,
        json!({ "visibility": "always-visible" }),
        range(0.5, 2.0),
    )
    .unwrap();

    // The range hid the cursor, so the cursor is hidden after the range.
    assert_eq!(
        got,
        vec![
            out(0.0, "a"),
            out(0.5, SHOW),
            out(1.0, "b"),
            out(1.5, "c"),
            out(2.0, HIDE),
            out(3.0, "d"),
        ]
    );

    // A range that does not change the cursor gives back the default: visible.
    let got = run(
        vec![out(0.0, "a"), out(1.0, "b"), out(3.0, "d")],
        json!({ "visibility": "always-hidden" }),
        range(0.5, 2.0),
    )
    .unwrap();
    assert_eq!(
        got,
        vec![
            out(0.0, "a"),
            out(0.5, HIDE),
            out(1.0, "b"),
            out(2.0, SHOW),
            out(3.0, "d"),
        ]
    );
}

#[test]
fn the_cursor_stays_as_the_filter_set_it_when_nothing_comes_after_the_region() {
    let got = run(
        vec![out(0.0, "a"), out(1.0, "b")],
        json!({ "visibility": "always-hidden" }),
        range(0.5, 10.0),
    )
    .unwrap();

    assert_eq!(got, vec![out(0.0, "a"), out(0.5, HIDE), out(1.0, "b")]);
}

#[test]
fn an_empty_region_sets_the_cursor_only_when_a_later_part_follows() {
    let events = vec![out(1.0, "a"), out(3.0, "b")];
    let hidden = json!({ "visibility": "always-hidden" });

    // A silent gap inside the timeline.
    assert_eq!(
        run(events.clone(), hidden.clone(), range(1.2, 1.8)).unwrap(),
        vec![out(1.0, "a"), out(1.2, HIDE), out(1.8, SHOW), out(3.0, "b")]
    );
    // After the end of the timeline, and a range of no length.
    for region in [range(5.0, 6.0), range(2.0, 2.0)] {
        assert_eq!(
            run(events.clone(), hidden.clone(), region.clone()).unwrap(),
            events,
            "{region:?}"
        );
    }
}

#[test]
fn a_sequence_that_two_events_split_is_rewritten_in_the_event_that_completes_it() {
    let events = vec![
        out(0.0, "a\x1b[?2"),
        input(0.2, "x"),
        out(0.5, "5h\x1b[?20"),
        out(0.7, "04hz"),
    ];

    let got = run(
        events,
        json!({ "visibility": "always-hidden" }),
        FilterRegion::All,
    )
    .unwrap();

    // The other sequence that two events split stays as it was.
    assert_eq!(
        got,
        vec![
            out(0.0, HIDE),
            out(0.0, "a"),
            input(0.2, "x"),
            out(0.5, "\x1b[?20"),
            out(0.7, "04hz"),
        ]
    );
}

#[test]
fn a_sequence_open_at_the_end_of_a_range_gets_the_restore_before_it() {
    let events = vec![out(0.0, "a"), out(1.0, "b\x1b[?20"), out(2.5, "04h")];

    let got = run(
        events,
        json!({ "visibility": "always-hidden" }),
        range(0.0, 2.0),
    )
    .unwrap();

    assert_eq!(
        got,
        vec![
            out(0.0, HIDE),
            out(0.0, "a"),
            out(1.0, "b\x1b[?25h\x1b[?20"),
            out(2.5, "04h"),
        ]
    );
}

#[test]
fn a_reset_of_the_terminal_gets_the_forced_cursor_again() {
    let events = vec![out(0.0, "a"), out(1.0, "\x1bcb"), out(2.0, "\x1b[!pc")];

    let got = run(
        events,
        json!({ "visibility": "always-hidden" }),
        FilterRegion::All,
    )
    .unwrap();

    assert_eq!(
        got,
        vec![
            out(0.0, HIDE),
            out(0.0, "a"),
            out(1.0, &format!("\x1bc{HIDE}b")),
            out(2.0, &format!("\x1b[!p{HIDE}c")),
        ]
    );
}

#[test]
fn a_sequence_that_an_osc_break_starts_is_rewritten_whole() {
    // The ESC that breaks the OSC starts the DECTCEM sequence.
    let events = vec![out(0.0, "\x1b]0;t"), out(1.0, "\x1b"), out(2.0, "[?25lz")];

    let got = run(
        events,
        json!({ "visibility": "always-visible" }),
        FilterRegion::All,
    )
    .unwrap();

    assert_eq!(
        got,
        vec![
            out(0.0, SHOW),
            out(0.0, "\x1b]0;t"),
            out(1.0, ""),
            out(2.0, "z"),
        ]
    );
}
