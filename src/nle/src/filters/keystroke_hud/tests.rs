//! End-to-end tests of the keystroke HUD filter. Each test runs the filter
//! through the registry, and reads the screen that a viewer sees.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;
use crate::filters;
use crate::types::{EditProject, EventType, Recording, Seconds, TerminalSize};
use termstate::{TerminalState, TerminalTheme};

/// The time of the last event of a test: it keeps the region long.
const END: f64 = 5.0;

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

/// `events` with an empty output at `END`.
fn ended(mut events: Vec<Event>) -> Vec<Event> {
    events.push(out(END, ""));
    events
}

fn size(columns: u16, rows: u16) -> TerminalSize {
    TerminalSize { columns, rows }
}

/// Options with no fade and a duration of one second.
fn plain() -> HudOptions {
    HudOptions {
        fade: 0.0,
        duration: 1.0,
        ..HudOptions::default()
    }
}

fn run(
    events: &[Event],
    options: &HudOptions,
    region: FilterRegion,
    size: TerminalSize,
) -> Vec<Event> {
    filters::registry()
        .run_on(events.to_vec(), &[applied(region, options)], size)
        .unwrap()
}

fn hud(events: &[Event], options: &HudOptions, size: TerminalSize) -> Vec<Event> {
    run(events, options, FilterRegion::All, size)
}

/// The screen after the output events with a time at or before `at`.
fn replay(events: &[Event], at: f64, size: TerminalSize) -> TerminalState {
    let mut state = TerminalState::new(
        usize::from(size.columns),
        usize::from(size.rows),
        TerminalTheme::auto(),
    );
    for event in events {
        if event.event_type == EventType::Output && event.time <= at {
            state.process_output(&event.data).unwrap();
        }
    }
    state
}

fn text(state: &TerminalState, row: usize, from: usize, to: usize) -> String {
    state.get_grid()[row][from..to]
        .iter()
        .map(|cell| cell.ch)
        .collect()
}

/// Assert that the viewer of `shown` sees what the viewer of `reference` sees.
fn assert_same_screen(shown: &TerminalState, reference: &TerminalState) {
    assert_eq!(shown.get_grid(), reference.get_grid());
    assert_eq!(shown.get_cursor_position(), reference.get_cursor_position());
}

const WIDE: TerminalSize = TerminalSize {
    columns: 40,
    rows: 10,
};

fn typing() -> Vec<Event> {
    ended(vec![
        out(0.0, "$ "),
        inp(1.0, "l"),
        out(1.01, "l"),
        inp(1.1, "s"),
        out(1.11, "s"),
    ])
}

#[test]
fn typed_text_shows_in_a_box_at_the_top_right() {
    let result = hud(&typing(), &plain(), WIDE);
    let screen = replay(&result, 1.5, WIDE);
    assert_eq!(text(&screen, 1, 33, 39), "╭────╮");
    assert_eq!(text(&screen, 2, 33, 39), "│ ls │");
    assert_eq!(text(&screen, 3, 33, 39), "╰────╯");
}

#[test]
fn the_box_goes_away_and_the_viewer_sees_the_recording_again() {
    let events = typing();
    let result = hud(&events, &plain(), WIDE);
    assert_same_screen(&replay(&result, 2.2, WIDE), &replay(&events, 2.2, WIDE));
}

#[test]
fn special_keys_show_their_labels() {
    let cases = [
        (vec![inp(1.0, "\x03")], "Ctrl+C"),
        (
            vec![inp(1.0, "\x1b[B"), inp(1.1, "\x1b[B"), inp(1.2, "\x1b[B")],
            "↓ ×3",
        ),
        (vec![inp(1.0, "ls"), inp(1.1, "\r")], "ls Enter"),
    ];
    for (presses, label) in cases {
        let result = hud(&ended(presses), &plain(), WIDE);
        let screen = replay(&result, 1.5, WIDE);
        let width = label.chars().count() + 4;
        let left = 40 - 1 - width;
        let row = text(&screen, 2, left, left + width);
        assert_eq!(row, format!("│ {label} │"));
    }
}

#[test]
fn the_box_stays_in_place_when_the_output_scrolls() {
    let size = size(30, 6);
    let options = HudOptions {
        position: Position::BottomRight,
        ..plain()
    };
    let mut events = vec![inp(1.0, "a")];
    for line in 0..10 {
        events.push(out(
            1.1 + 0.04 * f64::from(line),
            &format!("line{line}\r\n"),
        ));
    }
    let events = ended(events);
    let result = hud(&events, &options, size);

    let shown = replay(&result, 1.6, size);
    let reference = replay(&events, 1.6, size);
    assert_eq!(text(&shown, 2, 24, 29), "╭───╮");
    assert_eq!(text(&shown, 3, 24, 29), "│ a │");
    assert_eq!(text(&shown, 4, 24, 29), "╰───╯");
    assert!(!text(&shown, 1, 0, 30).contains(['╭', '│', '╰']));
    for row in 0..6 {
        for column in 0..30 {
            if (2..=4).contains(&row) && (24..29).contains(&column) {
                continue;
            }
            assert_eq!(
                shown.get_grid()[row][column],
                reference.get_grid()[row][column],
                "cell {row},{column}"
            );
        }
    }
    assert_same_screen(&replay(&result, 2.1, size), &replay(&events, 2.1, size));
}

#[test]
fn a_range_keeps_the_screen_from_before_the_region() {
    let mut fill = String::new();
    for row in 2..=4 {
        fill.push_str(&format!("\x1b[{row};34HXXXXXX"));
    }
    fill.push_str("\x1b[H");
    let events = vec![out(0.0, &fill), inp(2.0, "ls"), out(6.0, "z")];
    let region = FilterRegion::Range {
        start: Seconds::new(1.0).unwrap(),
        end: Seconds::new(10.0).unwrap(),
    };
    let result = run(&events, &plain(), region, WIDE);

    let during = replay(&result, 2.5, WIDE);
    assert_eq!(text(&during, 2, 33, 39), "│ ls │");
    assert_same_screen(&replay(&result, 3.5, WIDE), &replay(&events, 3.5, WIDE));
    assert_eq!(text(&replay(&result, 3.5, WIDE), 2, 33, 39), "XXXXXX");
}

#[test]
fn the_program_keeps_its_colors_after_a_box() {
    let events = ended(vec![out(0.0, "\x1b[31m"), inp(1.0, "a"), out(1.5, "x")]);
    let result = hud(&events, &plain(), WIDE);
    let screen = replay(&result, 1.5, WIDE);
    let cell = &screen.get_grid()[0][0];
    assert_eq!(cell.ch, 'x');
    assert_eq!(cell.fg_color, TerminalTheme::auto().red);
}

#[test]
fn the_label_ignores_the_line_drawing_set_of_the_program() {
    let events = ended(vec![out(0.0, "\x1b(0"), inp(1.0, "lq"), out(1.5, "q")]);
    let result = hud(&events, &plain(), WIDE);
    assert_eq!(text(&replay(&result, 1.4, WIDE), 2, 33, 39), "│ lq │");
    let shown = replay(&result, 1.5, WIDE);
    let reference = replay(&events, 1.5, WIDE);
    assert_eq!(shown.get_grid()[0][0].ch, '─');
    assert_eq!(shown.get_grid()[0][0], reference.get_grid()[0][0]);
}

#[test]
fn the_cursor_comes_back_when_a_full_screen_program_exits() {
    let events = ended(vec![
        out(0.0, "$ "),
        out(0.5, "\x1b[?1049h\x1b[2J"),
        inp(1.0, "j"),
        out(1.5, "\x1b[?1049l"),
        out(1.6, "X"),
    ]);
    let result = hud(&events, &plain(), WIDE);
    let shown = replay(&result, 3.0, WIDE);
    assert_same_screen(&shown, &replay(&events, 3.0, WIDE));
    assert_eq!(shown.get_grid()[0][2].ch, 'X');
}

#[test]
fn a_save_of_the_program_across_a_box_still_restores_its_cursor() {
    let events = ended(vec![
        out(0.0, "\x1b[5;5H\x1b7\x1b[1;1H"),
        inp(1.0, "a"),
        out(1.5, "\x1b8X"),
    ]);
    let result = hud(&events, &plain(), WIDE);
    assert_eq!(replay(&result, 1.5, WIDE).get_grid()[4][4].ch, 'X');
    assert_same_screen(&replay(&result, 3.0, WIDE), &replay(&events, 3.0, WIDE));
}

#[test]
fn the_fade_steps_blend_the_colors() {
    let options = HudOptions {
        fade: 0.2,
        ..plain()
    };
    let events = ended(vec![inp(1.0, "a")]);
    let result = hud(&events, &options, WIDE);

    assert_same_screen(&replay(&result, 1.0, WIDE), &replay(&events, 1.0, WIDE));
    let step = replay(&result, 1.0 + 0.2 / 6.0 + 1e-6, WIDE);
    let (red, green, blue) = step.get_grid()[1][33].fg_color;
    for channel in [red, green, blue] {
        assert!(0 < channel && channel < 128, "channel {channel}");
    }
    assert_eq!(
        replay(&result, 1.5, WIDE).get_grid()[1][33].fg_color,
        (128, 128, 128)
    );
    assert_same_screen(
        &replay(&result, 2.0 + 0.2, WIDE),
        &replay(&events, 2.0 + 0.2, WIDE),
    );
}

#[test]
fn the_recording_keeps_its_times() {
    let events = typing();
    let result = hud(&events, &plain(), WIDE);
    let mut rest = result.iter();
    for original in &events {
        assert!(
            rest.any(|event| event == original),
            "the event at {} s is missing or out of order",
            original.time
        );
    }

    let recording = Recording {
        version: 2,
        width: 40,
        height: 10,
        timestamp: 0.0,
        duration: END,
        command: String::new(),
        title: String::new(),
        env: BTreeMap::new(),
        events,
    };
    let mut project = EditProject::from_recording(Arc::new(recording)).unwrap();
    project.filters.push(applied(FilterRegion::All, &plain()));
    let rendered = filters::registry().render(&project).unwrap();
    assert!((rendered.duration - project.duration()).abs() < 1e-9);
    let size = rendered.size();
    assert_eq!(size, WIDE);
    assert_eq!(
        text(&replay(&rendered.events, 1.5, size), 2, 33, 39),
        "│ ls │"
    );
}

#[test]
fn a_long_text_shows_its_end() {
    let size = size(20, 6);
    let typed = "abcdefghijklmnopqrstuvwxyz0123";
    let result = hud(&ended(vec![inp(1.0, typed)]), &plain(), size);
    let end: String = typed.chars().skip(typed.chars().count() - 13).collect();
    assert_eq!(
        text(&replay(&result, 1.5, size), 2, 1, 19),
        format!("│ …{end} │")
    );
}

#[test]
fn a_screen_with_no_room_gets_no_box() {
    let events = ended(vec![out(0.0, "$ "), inp(1.0, "a"), out(1.1, "a")]);
    assert_eq!(hud(&events, &plain(), size(6, 10)), events);
}

#[test]
fn a_screen_that_is_too_large_fails_the_render() {
    let events = ended(vec![inp(1.0, "a")]);
    let error = filters::registry()
        .run_on(
            events,
            &[applied(FilterRegion::All, &plain())],
            size(2000, 2000),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("keystroke-hud"), "{error}");
    assert!(error.contains("1000000"), "{error}");
}

#[test]
fn the_registry_holds_the_keystroke_hud() {
    assert!(filters::registry().contains(&kind()));
}

#[test]
fn bad_options_fail_the_render_and_name_the_option() {
    let bad = AppliedFilter {
        options: FilterOptions::new().with(POSITION_OPTION, "middle"),
        ..applied(FilterRegion::All, &plain())
    };
    let error = filters::registry()
        .run_on(ended(vec![inp(1.0, "a")]), &[bad], WIDE)
        .unwrap_err()
        .to_string();
    assert!(error.contains("position"), "{error}");
}
