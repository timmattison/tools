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
