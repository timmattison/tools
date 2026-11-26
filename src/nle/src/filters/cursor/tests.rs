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
