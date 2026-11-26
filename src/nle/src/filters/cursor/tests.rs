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
    // The ESC that breaks the OSC starts the DECTCEM sequence. The filter
    // drops that sequence, and an ST where its ESC was keeps the end of the
    // OSC.
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
            out(1.0, "\x1b\\"),
            out(2.0, "z"),
        ]
    );
}

#[test]
fn a_c1_csi_inside_an_open_sequence_is_rewritten() {
    let events = vec![out(0.0, "\x1b[1\u{9b}?25ha")];

    let got = run(
        events,
        json!({ "visibility": "always-hidden" }),
        FilterRegion::All,
    )
    .unwrap();

    assert_eq!(got, vec![out(0.0, HIDE), out(0.0, "\x1b[1\x1b\\a")]);
}

#[test]
fn a_dropped_sequence_that_restarts_an_open_one_keeps_an_end_for_it() {
    // Without the ST, the rest reads as CSI 1 a, which moves the cursor.
    let events = vec![out(0.0, "\x1b[1\x1b[?25ha")];

    let got = run(
        events,
        json!({ "visibility": "always-hidden" }),
        FilterRegion::All,
    )
    .unwrap();

    assert_eq!(got, vec![out(0.0, HIDE), out(0.0, "\x1b[1\x1b\\a")]);
}

// Tests of the shape and the blink.

#[test]
fn a_shape_sets_decscusr_and_keeps_the_blink_of_the_recording() {
    let events = vec![out(0.0, "a"), out(1.0, "\x1b[4 qb"), out(2.0, "\x1b[ qc")];

    let got = run(events, json!({ "shape": "bar" }), FilterRegion::All).unwrap();

    // The default (0) blinks. 4 is a steady underline, thus a steady bar (6).
    assert_eq!(
        got,
        vec![
            out(0.0, "\x1b[5 q"),
            out(0.0, "a"),
            out(1.0, "\x1b[6 qb"),
            out(2.0, "\x1b[5 qc"),
        ]
    );
}

#[test]
fn a_blink_makes_each_shape_of_the_recording_steady_or_blinking() {
    let events = vec![out(
        0.0,
        "\x1b[1 qa\x1b[3 qb\x1b[5 qc\x1b[?12h\x1b[?12;25ld",
    )];

    let got = run(events, json!({ "blink": false }), FilterRegion::All).unwrap();

    // Mode 12 also starts and stops the blink, so the filter owns it too.
    assert_eq!(
        got,
        vec![
            out(0.0, "\x1b[2 q"),
            out(0.0, "a\x1b[4 qb\x1b[6 qc\x1b[?25ld"),
        ]
    );
}

#[test]
fn a_shape_and_a_blink_together_ignore_the_shape_of_the_recording() {
    let events = vec![out(0.0, "\x1b[6 qa\x1b[9 qb\x1b[70000 qc")];

    let got = run(
        events,
        json!({ "shape": "underline", "blink": true }),
        FilterRegion::All,
    )
    .unwrap();

    assert_eq!(got, vec![out(0.0, "\x1b[3 q"), out(0.0, "abc")]);
}

#[test]
fn a_decscusr_whose_first_parameter_is_not_a_number_is_dropped_too() {
    // The first value does not fit in a u64, and the second has a
    // sub-parameter.
    let events = vec![out(0.0, "\x1b[99999999999999999999 qa\x1b[2:1 qb")];

    let got = run(
        events,
        json!({ "shape": "underline", "blink": true }),
        FilterRegion::All,
    )
    .unwrap();

    assert_eq!(got, vec![out(0.0, "\x1b[3 q"), out(0.0, "ab")]);
}

#[test]
fn a_shape_alone_keeps_the_blink_mode_of_the_recording() {
    let events = vec![out(0.0, "\x1b[?12la")];

    let got = run(events, json!({ "shape": "block" }), FilterRegion::All).unwrap();

    assert_eq!(got, vec![out(0.0, "\x1b[1 q"), out(0.0, "\x1b[?12la")]);
}

#[test]
fn the_shape_of_the_recording_comes_back_at_the_end_of_a_range() {
    let events = vec![
        out(0.0, "a"),
        out(1.0, "\x1b[3 qb"),
        out(1.5, "\x1b[?12lc"),
        out(3.0, "d"),
    ];
    let steady_bar = json!({ "shape": "bar", "blink": false });

    assert_eq!(
        run(events, steady_bar.clone(), range(0.5, 2.0)).unwrap(),
        vec![
            out(0.0, "a"),
            out(0.5, "\x1b[6 q"),
            out(1.0, "b"),
            out(1.5, "c"),
            out(2.0, "\x1b[3 q\x1b[?12l"),
            out(3.0, "d"),
        ]
    );
    // A range that does not set the shape gives back the default: 0.
    assert_eq!(
        run(
            vec![out(1.0, "b"), out(3.0, "d")],
            steady_bar,
            range(0.5, 2.0)
        )
        .unwrap(),
        vec![
            out(0.5, "\x1b[6 q"),
            out(1.0, "b"),
            out(2.0, "\x1b[0 q"),
            out(3.0, "d"),
        ]
    );
}

#[test]
fn a_reset_of_the_terminal_gets_the_shape_again() {
    let events = vec![out(0.0, "a"), out(1.0, "\x1bcb")];

    let got = run(events, json!({ "shape": "bar" }), FilterRegion::All).unwrap();

    assert_eq!(
        got,
        vec![
            out(0.0, "\x1b[5 q"),
            out(0.0, "a"),
            out(1.0, "\x1bc\x1b[5 qb")
        ]
    );
}

// Tests of the color.

/// The OSC 12 sequence that the filter writes for `color`.
fn set_color(color: &str) -> String {
    format!("\x1b]12;{color}\x1b\\")
}

const RESET_COLOR: &str = "\x1b]112\x1b\\";

#[test]
fn a_color_sets_osc_12_and_drops_each_color_of_the_recording() {
    let events = vec![
        out(0.0, "a\x1b]12;#ff0000\x07b"),
        out(1.0, "\x1b]112\x1b\\c"),
        out(2.0, "\u{9d}12;red\u{9c}d"),
    ];

    let got = run(events, json!({ "color": "#00FF00" }), FilterRegion::All).unwrap();

    assert_eq!(
        got,
        vec![
            out(0.0, &set_color("#00FF00")),
            out(0.0, "ab"),
            out(1.0, "c"),
            out(2.0, "d"),
        ]
    );
}

#[test]
fn a_color_keeps_each_query_and_each_other_osc() {
    let events = vec![out(
        0.0,
        "\x1b]12;?\x07\x1b]10;?;?;?\x07\x1b]0;title\x07\x1b]4;1;#ff0000\x07\x1b]10;#111111\x1b\\",
    )];

    let got = run(
        events.clone(),
        json!({ "color": "#00ff00" }),
        FilterRegion::All,
    )
    .unwrap();

    assert_eq!(
        got,
        vec![out(0.0, &set_color("#00ff00")), events[0].clone()]
    );
}

#[test]
fn a_color_drops_the_cursor_slot_of_a_list_of_dynamic_colors() {
    // OSC 10 with four values sets the colors 10, 11, 12, and 13.
    let events = vec![out(
        0.0,
        "\x1b]10;#111111;#222222;#333333;#444444\x07\x1b]11;#222222;#333333\x1b\\\x1b]12;#333333;#444444\x07",
    )];

    let got = run(events, json!({ "color": "#00ff00" }), FilterRegion::All).unwrap();

    assert_eq!(
        got,
        vec![
            out(0.0, &set_color("#00ff00")),
            out(
                0.0,
                "\x1b]10;#111111;#222222\x07\x1b]13;#444444\x07\x1b]11;#222222\x1b\\\x1b]13;#444444\x07",
            ),
        ]
    );
}

#[test]
fn the_color_of_the_recording_comes_back_at_the_end_of_a_range() {
    let green = json!({ "color": "#00ff00" });
    let cases = [
        ("\x1b]12;#ff0000\x07b", set_color("#ff0000")),
        ("\x1b]10;#111111;#222222;#ff0000\x07b", set_color("#ff0000")),
        ("\x1b]12;#ff0000\x07\x1b]112\x07b", RESET_COLOR.to_string()),
        ("b", RESET_COLOR.to_string()),
    ];
    for (data, restore) in cases {
        let got = run(
            vec![out(1.0, data), out(3.0, "d")],
            green.clone(),
            range(0.5, 2.0),
        )
        .unwrap();

        assert_eq!(got[0], out(0.5, &set_color("#00ff00")), "{data:?}");
        assert_eq!(got[2], out(2.0, &restore), "{data:?}");
        assert_eq!(got[3], out(3.0, "d"), "{data:?}");
    }
}

#[test]
fn a_reset_of_the_terminal_gets_the_color_again() {
    let got = run(
        vec![out(1.0, "\x1bcb")],
        json!({ "color": "#00ff00" }),
        FilterRegion::All,
    )
    .unwrap();

    assert_eq!(
        got,
        vec![
            out(0.0, &set_color("#00ff00")),
            out(1.0, &format!("\x1bc{}b", set_color("#00ff00"))),
        ]
    );
}

// Tests of smart.

#[test]
fn smart_hides_the_cursor_during_a_burst_and_shows_it_after() {
    // Each key echoes after its input event, so the typing is not a burst.
    // The output of the command is.
    let events = vec![
        out(0.0, "$ "),
        input(1.0, "l"),
        out(1.0, "l"),
        input(1.05, "s"),
        out(1.05, "s"),
        input(1.4, "\r"),
        out(1.45, "\r\n"),
        out(1.5, "file1\r\n"),
        out(1.55, "file2\r\n"),
        out(1.6, "$ "),
        out(3.0, "x"),
    ];
    let mut expected = events.clone();
    expected[6] = out(1.45, "\x1b[?25l\r\n");
    expected[9] = out(1.6, "$ \x1b[?25h");

    let got = run(events, json!({ "visibility": "smart" }), FilterRegion::All).unwrap();

    assert_eq!(got, expected);
}

#[test]
fn smart_obeys_the_recording_outside_a_burst() {
    let events = vec![
        out(0.0, "\x1b[?25la"),
        out(1.0, "b"),
        out(1.05, "\x1b[?25hc"),
        out(1.1, "d"),
        out(2.0, "\x1b[?25le"),
        out(3.0, "\x1b[?25hf"),
    ];

    let got = run(events, json!({ "visibility": "smart" }), FilterRegion::All).unwrap();

    // The show of the recording in the burst waits for the end of the burst.
    assert_eq!(
        got,
        vec![
            out(0.0, "\x1b[?25la"),
            out(1.0, "b"),
            out(1.05, "c"),
            out(1.1, "d\x1b[?25h"),
            out(2.0, "\x1b[?25le"),
            out(3.0, "\x1b[?25hf"),
        ]
    );
}

#[test]
fn the_burst_gap_sets_what_counts_as_a_burst() {
    let events = vec![out(0.0, "a"), out(0.3, "b"), out(1.0, "c")];

    let wide = run(
        events.clone(),
        json!({ "visibility": "smart", "burst_gap": 0.5 }),
        FilterRegion::All,
    )
    .unwrap();
    let default = run(
        events.clone(),
        json!({ "visibility": "smart" }),
        FilterRegion::All,
    )
    .unwrap();

    assert_eq!(
        wide,
        vec![
            out(0.0, "\x1b[?25la"),
            out(0.3, "b\x1b[?25h"),
            out(1.0, "c")
        ]
    );
    assert_eq!(default, events);
}

#[test]
fn smart_waits_for_an_open_sequence_before_it_hides_or_shows_the_cursor() {
    let smart = json!({ "visibility": "smart" });

    // The burst ends inside a sequence, so the cursor shows after it.
    assert_eq!(
        run(
            vec![out(0.0, "a"), out(0.05, "b\x1b[3"), out(1.0, "1mc")],
            smart.clone(),
            FilterRegion::All,
        )
        .unwrap(),
        vec![
            out(0.0, "\x1b[?25la"),
            out(0.05, "b\x1b[3"),
            out(1.0, "1m\x1b[?25hc"),
        ]
    );
    // The burst starts inside a sequence, so the cursor hides after it, also
    // when the sequence is one that no filter rewrites.
    assert_eq!(
        run(
            vec![out(0.0, "a\x1b("), out(1.0, "Bb"), out(1.05, "c")],
            smart.clone(),
            FilterRegion::All,
        )
        .unwrap(),
        vec![
            out(0.0, "a\x1b("),
            out(1.0, "B\x1b[?25lb"),
            out(1.05, "c\x1b[?25h"),
        ]
    );
    // The region ends inside a sequence, so the cursor shows before it.
    assert_eq!(
        run(
            vec![out(0.0, "a"), out(0.05, "b\x1b[")],
            smart,
            FilterRegion::All,
        )
        .unwrap(),
        vec![out(0.0, "\x1b[?25la"), out(0.05, "b\x1b[?25h\x1b[")]
    );
}

#[test]
fn smart_in_a_range_leaves_the_cursor_as_the_recording_has_it() {
    let events = vec![out(1.0, "a"), out(1.05, "b"), out(3.0, "c")];

    let got = run(events, json!({ "visibility": "smart" }), range(0.5, 2.0)).unwrap();

    assert_eq!(
        got,
        vec![
            out(1.0, "\x1b[?25la"),
            out(1.05, "b\x1b[?25h"),
            out(3.0, "c"),
        ]
    );
}

// Tests of the save and the restore of a DEC private mode (XTSAVE, XTRESTORE).

#[test]
fn a_restore_of_a_saved_mode_cannot_bring_the_cursor_back() {
    let events = vec![out(0.0, "\x1b[?25;1049s\x1b[?25ha\x1b[?25;12r")];

    let got = run(
        events,
        json!({ "visibility": "always-hidden" }),
        FilterRegion::All,
    )
    .unwrap();

    // Mode 1049 and mode 12 are not the filter's, so they stay.
    assert_eq!(got, vec![out(0.0, HIDE), out(0.0, "\x1b[?1049sa\x1b[?12r")]);
}

#[test]
fn a_restore_of_a_saved_mode_sets_the_state_that_comes_back_after_a_range() {
    let events = vec![
        out(1.0, "\x1b[?25l\x1b[?25;12sb\x1b[?25h\x1b[?12h\x1b[?25;12rc"),
        out(3.0, "d"),
    ];

    let got = run(
        events,
        json!({ "visibility": "always-visible", "blink": true }),
        range(0.5, 2.0),
    )
    .unwrap();

    // The XTSAVE saves the blink that DECSCUSR 0 gives, thus the restore at the
    // end hides the cursor, writes DECSCUSR 0, and starts the blink with mode 12.
    assert_eq!(
        got,
        vec![
            out(0.5, "\x1b[?25h\x1b[1 q"),
            out(1.0, "bc"),
            out(2.0, "\x1b[?25l\x1b[0 q\x1b[?12h"),
            out(3.0, "d"),
        ]
    );
}

// Mode 12 resets the blink, then a DECSCUSR sets it. The last one wins, thus
// the restore writes the DECSCUSR of the recording after mode 12.
#[test]
fn the_restore_keeps_a_decscusr_that_comes_after_mode_12() {
    let events = vec![out(1.0, "\x1b[?12lb\x1b[1 qc"), out(3.0, "d")];

    let got = run(
        events,
        json!({ "shape": "bar", "blink": true }),
        range(0.5, 2.0),
    )
    .unwrap();

    assert_eq!(
        got,
        vec![
            out(0.5, "\x1b[5 q"),
            out(1.0, "bc"),
            out(2.0, "\x1b[1 q"),
            out(3.0, "d"),
        ]
    );
}

// A shape alone lets mode 12 pass. The restore writes DECSCUSR 0, then mode 12
// of the recording, so the cursor stays steady.
#[test]
fn a_shape_alone_sets_mode_12_of_the_recording_again_after_the_restore() {
    let events = vec![out(1.0, "\x1b[?12lb"), out(3.0, "d")];

    let got = run(events, json!({ "shape": "bar" }), range(0.5, 2.0)).unwrap();

    assert_eq!(
        got,
        vec![
            out(0.5, "\x1b[5 q"),
            out(1.0, "\x1b[?12lb"),
            out(2.0, "\x1b[0 q\x1b[?12l"),
            out(3.0, "d"),
        ]
    );
}

// Mode 12 resets the blink, then a DECSCUSR sets it. A shape alone keeps the
// blink of the DECSCUSR, thus the answer needs no second DECSCUSR.
#[test]
fn a_shape_alone_follows_a_decscusr_after_mode_12_of_the_recording() {
    let events = vec![out(0.0, "\x1b[?12la\x1b[5 qb")];

    let got = run(events, json!({ "shape": "bar" }), FilterRegion::All).unwrap();

    assert_eq!(
        got,
        vec![out(0.0, "\x1b[5 q"), out(0.0, "\x1b[?12la\x1b[5 qb")]
    );
}

// A DECSCUSR 1 blinks, so the XTSAVE saves a blink. The restore of mode 12
// brings it back after the DECSCUSR 2 and shows in the end state.
#[test]
fn an_xtsave_of_mode_12_saves_the_blink_that_a_decscusr_set() {
    let events = vec![
        out(1.0, "\x1b[1 qb\x1b[?12s\x1b[2 qc\x1b[?12rd"),
        out(3.0, "e"),
    ];

    let got = run(
        events,
        json!({ "shape": "bar", "blink": true }),
        range(0.5, 2.0),
    )
    .unwrap();

    assert_eq!(
        got,
        vec![
            out(0.5, "\x1b[5 q"),
            out(1.0, "bcd"),
            out(2.0, "\x1b[2 q\x1b[?12h"),
            out(3.0, "e"),
        ]
    );
}
