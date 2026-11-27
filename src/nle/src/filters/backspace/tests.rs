//! Tests of the backspace removal filter.

use std::sync::Arc;

use super::*;
use crate::error::NleError;
use crate::types::{
    AppliedFilter, EditProject, Event, EventType, FilterRegion, Recording, Seconds,
};

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

/// The filter on the region `All`, with no option.
fn applied_to_all() -> AppliedFilter {
    applied_to(FilterRegion::All)
}

/// The filter on `region`, with no option.
fn applied_to(region: FilterRegion) -> AppliedFilter {
    AppliedFilter {
        kind: kind(),
        region,
        options: FilterOptions::new(),
    }
}

/// The region from `start` to `end`, in seconds.
fn range(start: f64, end: f64) -> FilterRegion {
    FilterRegion::Range {
        start: Seconds::new(start).unwrap(),
        end: Seconds::new(end).unwrap(),
    }
}

/// Run the filter on the region `All` of `events`, on a terminal of 80 by 24.
fn run(events: Vec<Event>) -> Vec<Event> {
    let mut registry = FilterRegistry::new();
    register(&mut registry);
    registry.run(events, &[applied_to_all()]).unwrap()
}

/// Render a recording of `width` by `height` with `events` through the
/// filter on the region `All`. The duration of the source is the largest event
/// time.
fn render(width: u16, height: u16, events: Vec<Event>) -> Result<Recording, NleError> {
    render_in(width, height, events, FilterRegion::All)
}

/// Render like `render`, with the filter on `region`.
fn render_in(
    width: u16,
    height: u16,
    events: Vec<Event>,
    region: FilterRegion,
) -> Result<Recording, NleError> {
    let mut source = Recording::test_fixture();
    source.width = width;
    source.height = height;
    source.duration = events.iter().map(|event| event.time).fold(0.0, f64::max);
    source.events = events;
    let mut project = EditProject::from_recording(Arc::new(source))?;
    project.filters.push(applied_to(region));
    let mut registry = FilterRegistry::new();
    register(&mut registry);
    registry.render(&project)
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

#[test]
fn the_rendered_recording_is_shorter_by_the_removed_time() {
    let rendered = render(80, 24, typo_then_backspace("\x7f")).unwrap();
    assert_eq!(rendered.events, without_the_typo());
    assert_eq!(rendered.duration, 1.5);
}

#[test]
fn ctrl_h_erases_like_the_backspace_key() {
    assert_eq!(run(typo_then_backspace("\x08")), without_the_typo());
}

#[test]
fn a_typed_multi_byte_character_goes_with_its_backspace() {
    // `é` (U+00E9) is one character of two bytes.
    let mut events = vec![out(0.0, "$ ")];
    events.extend(typed(&[("h", 1.0), ("\u{e9}", 1.5)]));
    events.push(input(2.0, "\x7f"));
    events.push(out(2.0, "\x08 \x08"));
    events.extend(typed(&[("e", 2.5)]));
    assert_eq!(run(events), without_the_typo());
}

#[test]
fn a_combining_sequence_in_one_input_event_stays_and_ends_the_stack() {
    // A terminal draws `e` and U+0301 in one cell, and the emulator draws
    // them in two. The echo of the Ctrl+U erases `a` and the sequence in a
    // terminal, but leaves `a` on the screen of the emulator. If the sequence
    // were a typed key, the filter would remove it and the Ctrl+U, and keep
    // the `a` that the terminal erased.
    let events = vec![
        out(0.0, "$ "),
        input(1.0, "a"),
        out(1.0, "a"),
        input(1.5, "e\u{301}"),
        out(1.5, "e\u{301}"),
        input(2.0, "\x15"),
        out(2.0, "\x08\x08  \x08\x08"),
    ];
    assert_eq!(run(events.clone()), events);
}

#[test]
fn a_wide_character_and_its_shell_erase_stay_with_the_key_before_it() {
    // A terminal draws `日` (U+65E5) in two cells, and the emulator draws it
    // in one. The shell erases it with two backspaces, which in the emulator
    // also erase `a`. The screen then equals the screen before `a`, but the
    // terminal still shows `a`.
    let events = vec![
        out(0.0, "$ "),
        input(1.0, "a"),
        out(1.0, "a"),
        input(1.5, "\u{65e5}"),
        out(1.5, "\u{65e5}"),
        input(2.0, "\x7f"),
        out(2.0, "\x08\x08  \x08\x08"),
    ];
    assert_eq!(run(events.clone()), events);
}

#[test]
fn output_backspaces_of_a_program_with_no_input_stay() {
    let events = vec![out(1.0, "Loading... 50%"), out(1.5, "\x08\x08\x0875%")];
    assert_eq!(run(events.clone()), events);
}

#[test]
fn a_typed_key_and_output_backspaces_with_no_erase_key_stay() {
    let events = vec![
        out(0.0, "$ "),
        input(1.0, "a"),
        out(1.0, "a"),
        out(1.5, "\x08\x08"),
    ];
    assert_eq!(run(events.clone()), events);
}

#[test]
fn consecutive_backspaces_erase_a_word() {
    let mut events = vec![out(0.0, "$ ")];
    let keys = [
        ("c", 1.0),
        ("d", 1.25),
        (" ", 1.5),
        ("f", 1.75),
        ("o", 2.0),
        ("o", 2.25),
    ];
    for (key, time) in keys {
        events.push(input(time, key));
        events.push(out(time, key));
    }
    for time in [2.5, 2.75, 3.0] {
        events.push(input(time, "\x7f"));
        events.push(out(time, "\x08 \x08"));
    }
    for (key, time) in [("b", 3.25), ("a", 3.5), ("r", 3.75)] {
        events.push(input(time, key));
        events.push(out(time, key));
    }

    let mut expected = vec![out(0.0, "$ ")];
    for (key, time) in [
        ("c", 1.0),
        ("d", 1.25),
        (" ", 1.5),
        ("b", 1.75),
        ("a", 2.0),
        ("r", 2.25),
    ] {
        expected.push(input(time, key));
        expected.push(out(time, key));
    }
    assert_eq!(run(events), expected);
}

#[test]
fn a_backspace_that_does_not_restore_the_screen_stays_with_its_typed_key() {
    let events = vec![
        out(0.0, "$ "),
        input(1.0, "x"),
        out(1.0, "x"),
        input(2.0, "\x7f"),
        out(2.0, "\x1b[2J"),
    ];
    assert_eq!(run(events.clone()), events);
}

#[test]
fn a_key_between_the_typo_and_the_backspace_keeps_both() {
    // The echo of the right arrow is `CSI 1 C`. The emulator reads a `CSI C`
    // with no number as a move of zero columns.
    let events = vec![
        out(0.0, "$ "),
        input(1.0, "a"),
        out(1.0, "a"),
        input(1.25, "\x1b[D"),
        out(1.25, "\x08"),
        input(1.5, "\x1b[C"),
        out(1.5, "\x1b[1C"),
        input(1.75, "\x7f"),
        out(1.75, "\x08 \x08"),
    ];
    assert_eq!(run(events.clone()), events);
}

#[test]
fn a_backspace_that_erases_text_no_typed_key_wrote_stays() {
    let events = vec![out(0.0, "$ abc"), input(1.0, "\x7f"), out(1.0, "\x08 \x08")];
    assert_eq!(run(events.clone()), events);
}

#[test]
fn a_backspace_that_changes_nothing_on_the_screen_goes_with_its_output() {
    let events = vec![
        out(0.0, "$ "),
        input(1.0, "\x7f"),
        out(1.0, "\x07"),
        input(2.0, "l"),
        out(2.0, "l"),
    ];
    assert_eq!(
        run(events),
        vec![out(0.0, "$ "), input(1.0, "l"), out(1.0, "l")]
    );
}

#[test]
fn a_backspace_that_changes_nothing_keeps_the_stack() {
    // Mutant that was run: the rule for a key that changes nothing clears the
    // stack. Then the second backspace stays, and the test fails.
    let events = vec![
        out(0.0, "$ "),
        input(1.0, "a"),
        out(1.0, "a"),
        input(1.25, "\x7f"),
        out(1.25, "\x07"),
        input(1.5, "\x7f"),
        out(1.5, "\x08 \x08"),
        input(1.75, "b"),
        out(1.75, "b"),
    ];
    assert_eq!(
        run(events),
        vec![out(0.0, "$ "), input(1.0, "b"), out(1.0, "b")]
    );
}

/// The events of the issue with type-ahead: the echo of the Backspace comes
/// after the input of the next key `e`.
fn typo_then_backspace_with_type_ahead() -> Vec<Event> {
    vec![
        out(0.0, "$ "),
        input(1.0, "h"),
        out(1.0, "h"),
        input(1.5, "r"),
        out(1.5, "r"),
        input(2.0, "\x7f"),
        input(2.05, "e"),
        out(2.06, "\x08 \x08"),
        out(2.07, "e"),
    ]
}

#[test]
fn a_backspace_whose_echo_comes_after_the_next_key_stays() {
    // The unit of the Backspace has no output, so its screen does not change.
    // Its echo is in the unit of the `e`. If the Backspace went, its echo
    // would stay with no key press before it.
    let events = typo_then_backspace_with_type_ahead();
    assert_eq!(run(events.clone()), events);
}

#[test]
fn a_range_that_ends_before_the_late_echo_of_a_backspace_keeps_the_backspace() {
    // The first event after the range is the input of `e`, so the end of the
    // range cuts no unit. The echo of the Backspace comes after that input.
    let events = typo_then_backspace_with_type_ahead();
    let rendered = render_in(80, 24, events.clone(), range(1.0, 2.025)).unwrap();
    assert_eq!(rendered.events, events);
}

#[test]
fn with_no_echo_a_backspace_erases_the_newest_typed_key() {
    // Mutant that was run: the rule for a key that changes nothing runs before
    // the rule for the newest typed key. Then the backspace goes alone and the
    // typed key `a` stays, and the test fails.
    let events = vec![
        out(0.0, "Password: "),
        input(1.0, "a"),
        input(1.25, "\x7f"),
        input(1.5, "b"),
    ];
    assert_eq!(run(events), vec![out(0.0, "Password: "), input(1.0, "b")]);
}

/// Events that type each of `keys` after the prompt `$ `. Each key has its
/// echo at its time.
fn typed(keys: &[(&str, f64)]) -> Vec<Event> {
    keys.iter()
        .flat_map(|&(key, time)| [input(time, key), out(time, key)])
        .collect()
}

#[test]
fn ctrl_w_erases_the_typed_word() {
    let mut events = vec![out(0.0, "$ ")];
    events.extend(typed(&[
        ("l", 1.0),
        ("s", 1.25),
        (" ", 1.5),
        ("f", 1.75),
        ("o", 2.0),
        ("o", 2.25),
    ]));
    events.push(input(2.5, "\x17"));
    events.push(out(2.5, "\x08\x08\x08   \x08\x08\x08"));
    events.extend(typed(&[("x", 2.75)]));

    let mut expected = vec![out(0.0, "$ ")];
    expected.extend(typed(&[("l", 1.0), ("s", 1.25), (" ", 1.5), ("x", 1.75)]));
    assert_eq!(run(events), expected);
}

#[test]
fn ctrl_u_erases_the_typed_line() {
    let mut events = vec![out(0.0, "$ ")];
    events.extend(typed(&[("l", 1.0), ("s", 1.25)]));
    events.push(input(1.5, "\x15"));
    events.push(out(1.5, "\x08\x08  \x08\x08"));
    events.extend(typed(&[("p", 1.75), ("w", 2.0), ("d", 2.25)]));

    let mut expected = vec![out(0.0, "$ ")];
    expected.extend(typed(&[("p", 1.0), ("w", 1.25), ("d", 1.5)]));
    assert_eq!(run(events), expected);
}

#[test]
fn with_no_echo_ctrl_w_keeps_everything() {
    // Mutant that was run: with more than one match, the rule takes the newest
    // match. Then `b` goes and the test fails.
    let events = vec![
        out(0.0, "Password: "),
        input(1.0, "a"),
        input(1.25, "b"),
        input(1.5, "\x17"),
    ];
    assert_eq!(run(events.clone()), events);
}

#[test]
fn a_ctrl_w_that_erases_text_no_typed_key_wrote_stays() {
    // Mutant that was run: with no match, the rule removes the unit. Then the
    // key and its echo go and the test fails.
    let events = vec![
        out(0.0, "$ ls foo"),
        input(1.0, "\x17"),
        out(1.0, "\x08\x08\x08   \x08\x08\x08"),
    ];
    assert_eq!(run(events.clone()), events);
}

#[test]
fn a_ctrl_w_that_changes_nothing_goes() {
    // Mutant that was run: with no match, the rule never removes. Then the key
    // and its output stay and the test fails.
    let events = vec![
        out(0.0, "$ "),
        input(1.0, "\x17"),
        out(1.0, "\x07"),
        input(2.0, "l"),
        out(2.0, "l"),
    ];
    assert_eq!(
        run(events),
        vec![out(0.0, "$ "), input(1.0, "l"), out(1.0, "l")]
    );
}

#[test]
fn on_a_range_the_screen_before_the_region_counts() {
    // The `x` overwrites the `a` of the text before the region. The erase
    // leaves a space where the `a` was, so the screen differs from the screen
    // before the `x`.
    let events = vec![
        out(0.5, "$ abc\x08\x08\x08"),
        input(2.0, "x"),
        out(2.0, "x"),
        input(3.0, "\x7f"),
        out(3.0, "\x08 \x08"),
    ];
    let rendered = render_in(80, 24, events.clone(), range(1.0, 10.0)).unwrap();
    assert_eq!(rendered.events, events);
}

#[test]
fn a_typo_inside_a_range_goes_and_the_events_after_the_range_move_earlier() {
    // Mutant that was run: the filter answers `length: span.length`. Then the
    // events after the range stay at their old times and the test fails.
    let events = vec![
        out(0.0, "$ "),
        input(1.0, "h"),
        out(1.0, "h"),
        input(1.5, "r"),
        out(1.5, "r"),
        input(2.0, "\x7f"),
        out(2.0, "\x08 \x08"),
        input(2.5, "e"),
        out(2.5, "e"),
        input(4.0, "\r"),
        out(4.0, "\r\n"),
    ];
    let expected = vec![
        out(0.0, "$ "),
        input(1.0, "h"),
        out(1.0, "h"),
        input(1.5, "e"),
        out(1.5, "e"),
        input(3.0, "\r"),
        out(3.0, "\r\n"),
    ];
    let rendered = render_in(80, 24, events, range(1.0, 3.0)).unwrap();
    assert_eq!(rendered.events, expected);
}

#[test]
fn a_range_that_ends_between_a_backspace_and_its_echo_keeps_the_backspace() {
    // The end of the range cuts the echo of the Backspace from its input
    // event. Inside the range the Backspace changes nothing, but its echo
    // after the range erases the `r`. Thus the filter must keep it.
    //
    // The bell gives the unit output inside the range, so the rule of a unit
    // with no output does not keep the Backspace. Only the cut rule does.
    // Mutant that was run: `mark_removed` never sets `cut`. Then the
    // Backspace and the bell go and the test fails.
    let events = vec![
        out(0.0, "$ "),
        input(1.0, "h"),
        out(1.0, "h"),
        input(1.5, "r"),
        out(1.5, "r"),
        input(1.99, "\x7f"),
        out(1.995, "\x07"),
        out(2.0, "\x08 \x08"),
        input(2.5, "e"),
        out(2.5, "e"),
    ];
    let rendered = render_in(80, 24, events.clone(), range(1.0, 2.0)).unwrap();
    assert_eq!(rendered.events, events);
}

#[test]
fn a_range_that_ends_after_the_echo_of_a_backspace_removes_the_typo() {
    // Mutant that was run: the filter keeps the last key of the range when
    // any event comes after the range. Then the `r` and the Backspace stay and
    // the test fails. The first event after this range is an input event, so
    // the end of the range cuts no event of the unit of the Backspace.
    let rendered = render_in(80, 24, typo_then_backspace("\x7f"), range(1.0, 2.25)).unwrap();
    assert_eq!(
        rendered.events,
        vec![
            out(0.0, "$ "),
            input(1.0, "h"),
            out(1.0, "h"),
            input(1.75, "e"),
            out(1.75, "e"),
        ]
    );
}

#[test]
fn a_black_cell_is_not_a_blank_cell() {
    // With `TerminalTheme::auto()`, black is the background, so the space on
    // `CSI 40 m` equals a blank cell. In another theme the viewer sees a black
    // cell, so the pair must stay.
    let events = vec![
        out(0.0, "$ "),
        input(1.0, "x"),
        out(1.0, "x"),
        input(2.0, "\x7f"),
        out(2.0, "\x08\x1b[40m \x1b[0m\x08"),
    ];
    assert_eq!(run(events.clone()), events);
}

/// Run the filter on the region `All` of `events` with `options`.
fn run_with(events: Vec<Event>, options: FilterOptions) -> Result<Vec<Event>, NleError> {
    let mut registry = FilterRegistry::new();
    register(&mut registry);
    let applied = AppliedFilter {
        options,
        ..applied_to_all()
    };
    registry.run(events, &[applied])
}

#[test]
fn an_option_fails_the_render_and_the_message_names_it() {
    let options = FilterOptions::new().with("speed", 2);
    let error = run_with(typo_then_backspace("\x7f"), options).unwrap_err();
    let NleError::RenderError(message) = error else {
        panic!("expected a render error, got {error:?}");
    };
    assert!(message.contains("speed"), "{message}");
    assert!(message.contains("backspace-removal"), "{message}");
}

/// The message of the render error that `result` holds.
fn render_error_message(result: Result<Recording, NleError>) -> String {
    match result {
        Err(NleError::RenderError(message)) => message,
        other => panic!("expected a render error, got {other:?}"),
    }
}

#[test]
fn a_terminal_of_no_columns_fails_the_render() {
    let message = render_error_message(render(0, 24, vec![out(0.0, "x")]));
    assert!(message.contains("0 by 24"), "{message}");
}

#[test]
fn a_terminal_of_no_rows_fails_the_render() {
    let message = render_error_message(render(80, 0, vec![out(0.0, "x")]));
    assert!(message.contains("80 by 0"), "{message}");
}

#[test]
fn a_terminal_of_more_than_the_cell_limit_fails_the_render() {
    let message = render_error_message(render(2048, 1024, vec![out(0.0, "x")]));
    assert!(message.contains("2048 by 1024"), "{message}");
    assert!(message.contains(&MAX_CELLS.to_string()), "{message}");
}

#[test]
fn a_line_of_more_typed_keys_than_the_depth_keeps_its_oldest_key() {
    // The first typed key falls off the stack. The last backspace then has no
    // typed key to erase, so it stays with the key before it.
    let mut events = vec![out(0.0, "$ ")];
    for i in 0..=MAX_DEPTH {
        events.extend(typed(&[("a", 1.0 + 0.25 * i as f64)]));
    }
    for i in 0..=MAX_DEPTH {
        let time = 1.0 + 0.25 * (MAX_DEPTH + 1 + i) as f64;
        events.push(input(time, "\x7f"));
        events.push(out(time, "\x08 \x08"));
    }
    let expected = vec![
        out(0.0, "$ "),
        input(1.0, "a"),
        out(1.0, "a"),
        input(1.25, "\x7f"),
        out(1.25, "\x08 \x08"),
    ];
    let rendered = render(300, 3, events).unwrap();
    assert_eq!(rendered.events, expected);
}

#[test]
fn the_budget_lowers_the_depth_on_a_large_terminal() {
    // Mutant that was run: the depth is always `MAX_DEPTH`. Then all five
    // typed keys and backspaces go and the test fails.
    // A terminal of 2048 by 512 cells has 1 << 20 cells, so the depth is 4.
    let mut events = vec![out(0.0, "$ ")];
    events.extend(typed(&[
        ("a", 1.0),
        ("b", 1.25),
        ("c", 1.5),
        ("d", 1.75),
        ("e", 2.0),
    ]));
    for time in [2.25, 2.5, 2.75, 3.0, 3.25] {
        events.push(input(time, "\x7f"));
        events.push(out(time, "\x08 \x08"));
    }
    let expected = vec![
        out(0.0, "$ "),
        input(1.0, "a"),
        out(1.0, "a"),
        input(1.25, "\x7f"),
        out(1.25, "\x08 \x08"),
    ];
    let rendered = render(2048, 512, events).unwrap();
    assert_eq!(rendered.events, expected);
}
