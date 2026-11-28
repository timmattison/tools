//! End-to-end tests of the keystroke HUD filter. Each test runs the filter
//! through the registry, and reads the screen that a viewer sees.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;
use crate::filters;
use crate::pipeline::TerminalSize;
use crate::types::{EditProject, EventType, Recording, Seconds};
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
    TerminalSize {
        width: columns,
        height: rows,
    }
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
        usize::from(size.width),
        usize::from(size.height),
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

/// Assert that the viewer of `shown` sees what the viewer of `reference` sees:
/// the grid, the cursor, and the visibility of the cursor.
fn assert_same_screen(shown: &TerminalState, reference: &TerminalState) {
    assert_eq!(shown.get_grid(), reference.get_grid());
    assert_eq!(shown.get_cursor_position(), reference.get_cursor_position());
    assert_eq!(shown.is_cursor_visible(), reference.is_cursor_visible());
}

const WIDE: TerminalSize = TerminalSize {
    width: 40,
    height: 10,
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
        (vec![inp(1.0, "ls\rpwd\r")], "ls Enter pwd Enter"),
        (vec![inp(1.0, "\r\r")], "Enter ×2"),
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
fn a_restore_of_the_program_across_a_box_is_repaired_after_its_event() {
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
fn a_restore_of_the_program_across_a_box_puts_back_its_character_set() {
    let events = ended(vec![
        out(0.0, "\x1b7\x1b(0"),
        inp(1.0, "a"),
        out(1.5, "\x1b8q"),
        out(4.0, "q"),
    ]);
    let result = hud(&events, &plain(), WIDE);
    let at_four: Vec<&Event> = result.iter().filter(|event| event.time == 4.0).collect();
    assert_eq!(at_four.len(), 1, "a repaint at 4.0: {at_four:?}");
    assert_same_screen(&replay(&result, 4.5, WIDE), &replay(&events, 4.5, WIDE));
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
    let (red, green, blue) = step.get_grid()[1][34].fg_color;
    for channel in [red, green, blue] {
        assert!(0 < channel && channel < 128, "channel {channel}");
    }
    assert_eq!(
        replay(&result, 1.5, WIDE).get_grid()[1][34].fg_color,
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

#[test]
fn a_paste_over_several_input_events_shows_as_one_paste() {
    let events = ended(vec![
        inp(1.0, "\x1b[200~secret"),
        inp(1.0001, "word\r"),
        inp(1.0002, "\x1b[201~"),
    ]);
    let result = hud(&events, &plain(), WIDE);
    let screen = replay(&result, 1.1, WIDE);
    assert_eq!(text(&screen, 2, 30, 39), "│ Paste │");
    for row in 0..10 {
        assert!(
            !text(&screen, row, 0, 40).contains("word"),
            "row {row} shows pasted text"
        );
    }
}

#[test]
fn a_box_that_goes_away_inside_a_sequence_of_the_program_waits_for_its_end() {
    let events = ended(vec![
        out(0.0, "ab"),
        inp(1.0, "a"),
        out(1.95, "\x1b[?25"),
        out(2.05, "l"),
    ]);
    let result = hud(&events, &plain(), WIDE);
    assert_same_screen(&replay(&result, 3.0, WIDE), &replay(&events, 3.0, WIDE));
}

#[test]
fn a_region_that_ends_inside_a_sequence_of_the_program_takes_its_box_away() {
    let events = ended(vec![
        out(0.0, "ab"),
        inp(1.0, "a"),
        out(1.4, "\x1b[?25"),
        out(1.6, "l"),
    ]);
    let region = FilterRegion::Range {
        start: Seconds::new(0.0).unwrap(),
        end: Seconds::new(1.5).unwrap(),
    };
    let result = run(&events, &plain(), region, WIDE);
    assert_same_screen(&replay(&result, 3.0, WIDE), &replay(&events, 3.0, WIDE));
}

/// A password that the presenter types. No other text on its screen holds one
/// of its characters: not the prompt, not the label `Enter`, and not the box.
const PASSWORD: &str = "xyzzy42";

/// A password prompt. The presenter types `PASSWORD` one key each 0.1 s from
/// 1.0 s, and presses Enter at 1.8 s. The terminal does not echo the keys.
fn password_typing() -> Vec<Event> {
    let mut events = vec![out(0.0, "Password: ")];
    let mut time = 1.0;
    for key in PASSWORD.chars() {
        events.push(inp(time, &key.to_string()));
        time += 0.1;
    }
    events.push(inp(1.8, "\r"));
    events.push(out(1.81, "\r\n$ "));
    ended(events)
}

/// A redaction that puts asterisks in place of `PASSWORD`, in the input
/// events too.
fn password_redaction() -> AppliedFilter {
    let redaction = filters::redaction::Redaction::new(
        filters::redaction::Find::Text(PASSWORD.to_string()),
        filters::redaction::Replacement::Asterisks,
        true,
    )
    .unwrap();
    filters::redaction::applied(FilterRegion::All, &redaction)
}

/// The rows of the screen of `events` at the time of each event.
fn screens(events: &[Event], size: TerminalSize) -> Vec<(f64, Vec<String>)> {
    events
        .iter()
        .map(|event| {
            let screen = replay(events, event.time, size);
            let rows = (0..usize::from(size.height))
                .map(|row| text(&screen, row, 0, usize::from(size.width)))
                .collect();
            (event.time, rows)
        })
        .collect()
}

#[test]
fn a_redaction_of_input_before_the_hud_hides_the_typed_keys() {
    let result = filters::registry()
        .run_on(
            password_typing(),
            &[password_redaction(), applied(FilterRegion::All, &plain())],
            WIDE,
        )
        .unwrap();
    assert_eq!(
        text(&replay(&result, 1.85, WIDE), 2, 22, 39),
        "│ ******* Enter │"
    );
    for (time, rows) in screens(&result, WIDE) {
        for (row, shown) in rows.iter().enumerate() {
            assert!(
                !shown.contains(|ch| PASSWORD.contains(ch)),
                "row {row} at {time} s shows a typed key: {shown:?}"
            );
        }
    }
}

#[test]
fn a_redaction_of_input_after_the_hud_does_not_hide_the_typed_keys() {
    let result = filters::registry()
        .run_on(
            password_typing(),
            &[applied(FilterRegion::All, &plain()), password_redaction()],
            WIDE,
        )
        .unwrap();
    assert_eq!(text(&replay(&result, 1.05, WIDE), 2, 34, 39), "│ x │");
    assert_eq!(text(&replay(&result, 1.55, WIDE), 2, 29, 39), "│ xyzzy4 │");
    // The box holds the whole password, and the redaction does not mask it.
    // The draw at 1.6 s writes only the cells that change, and the z at column
    // 34 does not change. Thus no output event holds the password in one run.
    assert_eq!(text(&replay(&result, 1.65, WIDE), 2, 28, 39), "│ xyzzy42 │");
}

/// The output events of `result` at `time`, other than the events of
/// `recording`.
fn draws_at(result: &[Event], recording: &[Event], time: f64) -> usize {
    let count = |events: &[Event]| {
        events
            .iter()
            .filter(|event| event.event_type == EventType::Output && event.time == time)
            .count()
    };
    count(result) - count(recording)
}

#[test]
fn a_wide_character_in_the_label_takes_two_cells_and_stays() {
    let events = ended(vec![
        out(0.0, "$ "),
        inp(1.0, "日a"),
        out(1.01, "日a"),
        out(1.2, "x"),
        out(1.3, "y"),
    ]);
    let result = hud(&events, &plain(), WIDE);

    // `日` covers two cells, thus the box is one cell wider than for two
    // narrow keys. The second cell of `日` holds the continuation mark.
    let screen = replay(&result, 1.5, WIDE);
    assert_eq!(text(&screen, 1, 32, 39), "╭─────╮");
    assert_eq!(text(&screen, 2, 32, 39), "│ 日\0a │");
    assert_eq!(text(&screen, 3, 32, 39), "╰─────╯");
    // The output of the program changes no cell of the box, thus the HUD
    // writes nothing more.
    assert_eq!(draws_at(&result, &events, 1.2), 0);
    assert_eq!(draws_at(&result, &events, 1.3), 0);
    assert!(result.iter().all(|event| !event.data.contains('\0')));
}

#[test]
fn a_wide_character_of_the_program_under_an_edge_of_the_box_shows_as_blank_halves() {
    // The box of `ls` stands in columns 33 to 38. `日` of the program covers
    // columns 32 and 33, and `本` covers columns 38 and 39.
    let events = ended(vec![
        out(0.0, "\x1b[2;33H日\x1b[2;39H本\x1b[1;1H"),
        inp(1.0, "l"),
        out(1.01, "l"),
        inp(1.1, "s"),
        out(1.11, "s"),
        out(1.2, "x"),
    ]);
    let result = hud(&events, &plain(), WIDE);

    let screen = replay(&result, 1.5, WIDE);
    assert_eq!(text(&screen, 1, 32, 40), " ╭────╮ ");
    assert_eq!(draws_at(&result, &events, 1.2), 0);
    assert!(result.iter().all(|event| !event.data.contains('\0')));

    // When the box goes away, both wide characters come back.
    let end = replay(&result, END, WIDE);
    assert_same_screen(&end, &replay(&events, END, WIDE));
}
