//! Tests of the viewport filter.

use std::sync::Arc;

use serde_json::json;
use termstate::{ColorSource, TerminalState, TerminalTheme};

use super::writer::{Glyph, Look, Writer};
use super::*;
use crate::error::NleError;
use crate::pipeline::{RegionView, ScreenSize};
use crate::preview::{MAX_COLUMNS, MAX_ROWS};
use crate::types::{AppliedFilter, EditProject, EventType, FilterRegion, Recording, Seconds};

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

/// The answer of the filter for `events` on the region `All` of a screen of
/// `width` by `height`.
fn call(
    width: u16,
    height: u16,
    events: &[Event],
    value: serde_json::Value,
) -> Result<Screen, String> {
    let view = ScreenView {
        region: RegionView {
            length: 100.0,
            open: true,
        },
        size: ScreenSize { width, height },
        before: &[],
        whole_timeline: true,
    };
    filter(events, view, &options(value))
}

/// Render `events` through the app registry, with the filter on `region`.
fn render_on(
    width: u16,
    height: u16,
    events: Vec<Event>,
    value: serde_json::Value,
    region: FilterRegion,
) -> Result<Recording, NleError> {
    let mut source = Recording::test_fixture();
    source.width = width;
    source.height = height;
    source.duration = 100.0;
    source.events = events;
    let mut project = EditProject::from_recording(Arc::new(source)).unwrap();
    project.filters.push(AppliedFilter {
        kind: FilterKind::new(VIEWPORT),
        region,
        options: options(value),
    });
    crate::filters::registry().render(&project)
}

/// Render `events` on a screen of 80 by 24, with the filter on `All`.
fn render(events: Vec<Event>, value: serde_json::Value) -> Recording {
    render_on(80, 24, events, value, FilterRegion::All).unwrap()
}

/// The text of the error of `call`, or a panic.
fn refusal(width: u16, height: u16, value: serde_json::Value) -> String {
    match call(width, height, &[out(0.0, "hi")], value.clone()) {
        Err(message) => message,
        Ok(screen) => panic!("{value}: expected an error, got {screen:?}"),
    }
}

/// A `(time, data)` pair for each event.
fn pairs(events: &[Event]) -> Vec<(f64, &str)> {
    events
        .iter()
        .map(|event| (event.time, event.data.as_str()))
        .collect()
}

// Tests of the options and the checks.

#[test]
fn no_option_gives_the_events_and_the_size_back() {
    let events = vec![
        out(0.0, "\x1b]2;t\x07hi\r\n"),
        input(0.5, "q"),
        out(1.0, "\x1b[2J"),
    ];

    let screen = call(80, 24, &events, json!({})).unwrap();

    assert_eq!(screen.events, events);
    assert_eq!(
        screen.size,
        ScreenSize {
            width: 80,
            height: 24
        }
    );
}

#[test]
fn a_crop_of_the_full_screen_gives_the_events_and_the_size_back() {
    let events = vec![out(0.0, "\x1b]2;t\x07hi\r\n"), out(1.0, "\x1b[2J")];

    for value in [
        json!({ "rows": { "start": 0, "end": 24 } }),
        json!({ "columns": { "start": 0, "end": 80 } }),
        json!({ "hide_rows": [], "hide_columns": [] }),
        json!({ "rows": { "start": 0, "end": 24 }, "columns": { "start": 0, "end": 80 } }),
    ] {
        let screen = call(80, 24, &events, value.clone()).unwrap();

        assert_eq!(screen.events, events, "{value}");
        assert_eq!(
            screen.size,
            ScreenSize {
                width: 80,
                height: 24
            },
            "{value}"
        );
    }
}

#[test]
fn an_unknown_option_is_refused_and_named() {
    let message = refusal(80, 24, json!({ "colour": 1 }));

    assert!(message.contains("unknown field"), "{message}");
    assert!(message.contains("colour"), "{message}");
}

#[test]
fn an_unknown_key_of_a_crop_is_refused_and_named() {
    let message = refusal(
        80,
        24,
        json!({ "rows": { "start": 0, "end": 2, "step": 1 } }),
    );

    assert!(message.contains("step"), "{message}");
}

#[test]
fn a_value_of_the_wrong_type_is_refused() {
    for value in [
        json!({ "rows": "all" }),
        json!({ "rows": { "start": -1, "end": 2 } }),
        json!({ "rows": { "start": 0 } }),
        json!({ "hide_rows": 3 }),
        json!({ "hide_columns": ["a"] }),
    ] {
        let message = refusal(80, 24, value.clone());

        assert!(!message.is_empty(), "{value}");
    }
}

#[test]
fn a_screen_with_no_cell_is_refused() {
    assert_eq!(
        refusal(0, 24, json!({ "hide_rows": [0] })),
        "the screen of 0x24 has no cells"
    );
    assert_eq!(
        refusal(80, 0, json!({ "hide_rows": [0] })),
        "the screen of 80x0 has no cells"
    );
}

#[test]
fn a_screen_above_the_limits_of_the_preview_is_refused() {
    let too_wide = u16::try_from(MAX_COLUMNS + 1).unwrap();
    let too_high = u16::try_from(MAX_ROWS + 1).unwrap();

    assert_eq!(
        refusal(too_wide, 10, json!({ "hide_rows": [0] })),
        format!("the screen of {too_wide}x10 is larger than {MAX_COLUMNS}x{MAX_ROWS}, the limit of the preview")
    );
    assert_eq!(
        refusal(10, too_high, json!({ "hide_rows": [0] })),
        format!("the screen of 10x{too_high} is larger than {MAX_COLUMNS}x{MAX_ROWS}, the limit of the preview")
    );
}

#[test]
fn a_screen_at_the_limits_of_the_preview_is_accepted() {
    let width = u16::try_from(MAX_COLUMNS).unwrap();
    let height = u16::try_from(MAX_ROWS).unwrap();

    let screen = call(width, height, &[], json!({ "hide_rows": [0] })).unwrap();

    assert_eq!(
        screen.size,
        ScreenSize {
            width,
            height: height - 1
        }
    );
}

#[test]
fn a_crop_that_starts_at_or_after_its_end_is_refused() {
    assert_eq!(
        refusal(80, 24, json!({ "rows": { "start": 3, "end": 3 } })),
        "rows: the start 3 is not before the end 3"
    );
    assert_eq!(
        refusal(80, 24, json!({ "columns": { "start": 5, "end": 2 } })),
        "columns: the start 5 is not before the end 2"
    );
}

#[test]
fn a_crop_that_ends_after_the_screen_is_refused() {
    assert_eq!(
        refusal(80, 24, json!({ "rows": { "start": 0, "end": 25 } })),
        "rows: the end 25 is after the last row of the screen (24 rows)"
    );
    assert_eq!(
        refusal(80, 24, json!({ "columns": { "start": 0, "end": 81 } })),
        "columns: the end 81 is after the last column of the screen (80 columns)"
    );
}

#[test]
fn a_hidden_index_that_is_not_on_the_screen_is_refused() {
    assert_eq!(
        refusal(80, 24, json!({ "hide_rows": [0, 24] })),
        "hide_rows: the row 24 is not on the screen (24 rows)"
    );
    assert_eq!(
        refusal(80, 24, json!({ "hide_columns": [80] })),
        "hide_columns: the column 80 is not on the screen (80 columns)"
    );
}

#[test]
fn a_screen_with_no_visible_row_or_column_is_refused() {
    assert_eq!(
        refusal(3, 2, json!({ "hide_rows": [0, 1] })),
        "no row is visible"
    );
    assert_eq!(
        refusal(
            3,
            2,
            json!({ "rows": { "start": 1, "end": 2 }, "hide_rows": [1] })
        ),
        "no row is visible"
    );
    assert_eq!(
        refusal(3, 2, json!({ "hide_columns": [0, 1, 2] })),
        "no column is visible"
    );
}

#[test]
fn the_checks_run_in_the_documented_order() {
    // The crop of the rows comes before the crop of the columns, and the
    // crops come before the hidden indexes.
    let bad = json!({
        "rows": { "start": 5, "end": 5 },
        "columns": { "start": 5, "end": 5 },
        "hide_rows": [99],
        "hide_columns": [99],
    });
    assert!(refusal(80, 24, bad).starts_with("rows:"));

    let bad = json!({
        "columns": { "start": 5, "end": 5 },
        "hide_rows": [99],
        "hide_columns": [99],
    });
    assert!(refusal(80, 24, bad).starts_with("columns:"));

    let bad = json!({ "hide_rows": [99], "hide_columns": [99] });
    assert!(refusal(80, 24, bad).starts_with("hide_rows:"));

    let bad = json!({ "hide_columns": [99], "hide_rows": [0, 1] });
    assert!(refusal(80, 2, bad).starts_with("hide_columns:"));
}

#[test]
fn the_render_error_names_the_filter() {
    let result = render_on(
        80,
        24,
        vec![out(0.0, "hi")],
        json!({ "hide_rows": [24] }),
        FilterRegion::All,
    );

    match result {
        Err(NleError::RenderError(message)) => assert!(
            message.contains(
                "filter \"viewport\" failed: hide_rows: the row 24 is not on the screen (24 rows)"
            ),
            "{message}"
        ),
        other => panic!("expected a RenderError, got {other:?}"),
    }
}

// Tests of the size.

#[test]
fn the_rendered_recording_is_the_visible_columns_by_the_visible_rows() {
    let rendered = render(
        vec![out(1.0, "hi")],
        json!({
            "rows": { "start": 2, "end": 20 },
            "columns": { "start": 10, "end": 70 },
            "hide_rows": [0, 5, 6],
            "hide_columns": [11, 69, 75],
        }),
    );

    assert_eq!((rendered.width, rendered.height), (58, 16));
}

// Tests of the bytes.

#[test]
fn one_red_bold_character_gives_one_sgr_and_the_character() {
    let rendered = render(vec![out(1.0, "\x1b[1;31mA")], json!({ "hide_rows": [23] }));

    assert_eq!(pairs(&rendered.events), vec![(1.0, "\x1b[0;1;31mA")]);
}

#[test]
fn a_cell_that_the_cursor_is_not_on_gets_a_cup() {
    let rendered = render(
        vec![out(1.0, "\r\n\x1b[1;31mA")],
        json!({ "hide_rows": [23] }),
    );

    assert_eq!(
        pairs(&rendered.events),
        vec![(1.0, "\x1b[2;1H\x1b[0;1;31mA")]
    );
}

#[test]
fn two_cells_next_to_each_other_give_one_cup() {
    let rendered = render(vec![out(1.0, "\r\nAB")], json!({ "hide_rows": [23] }));

    assert_eq!(pairs(&rendered.events), vec![(1.0, "\x1b[2;1HAB")]);
}

#[test]
fn a_cell_after_a_wide_character_gets_its_own_cup() {
    let rendered = render(vec![out(1.0, "日本")], json!({ "hide_rows": [23] }));

    assert_eq!(
        pairs(&rendered.events),
        vec![(1.0, "日\x1b[1;2H本\x1b[1;3H")]
    );
}

#[test]
fn a_cursor_on_a_hidden_row_is_hidden_at_the_start() {
    let rendered = render(vec![out(1.0, "x")], json!({ "hide_rows": [0] }));

    assert_eq!(rendered.events.first().map(|e| e.time), Some(0.0));
    assert_eq!(
        rendered.events.first().map(|e| e.data.as_str()),
        Some("\x1b[?25l")
    );
}

#[test]
fn a_cursor_that_comes_back_to_a_visible_row_is_shown_again() {
    let rendered = render(
        vec![out(1.0, "\x1b[24;1H"), out(2.0, "\x1b[1;1H")],
        json!({ "hide_rows": [23] }),
    );

    assert_eq!(
        pairs(&rendered.events),
        vec![(1.0, "\x1b[?25l"), (2.0, "\x1b[?25h")]
    );
}

// Tests of the sequences that pass through.

#[test]
fn the_sequences_that_change_no_cell_pass_through() {
    for data in [
        "\x1b]2;a title\x07",
        "\x1b]4;1;#ff0000\x1b\\",
        "\x1b]12;#00ff00\x07",
        "\x1b[5 q",
        "\x1b[?12h",
        "\x1b[?12l",
        "\x07",
    ] {
        let rendered = render(vec![out(1.0, data)], json!({ "hide_rows": [23] }));

        assert_eq!(pairs(&rendered.events), vec![(1.0, data)], "{data:?}");
    }
}

#[test]
fn a_sequence_that_is_near_a_pass_through_form_does_not_pass_through() {
    // Mode 12 that is not private, a longer private mode that ends in 12, and
    // a final q with no space (DECLL).
    for data in ["\x1b[12h", "\x1b[?1012h", "\x1b[5q"] {
        let rendered = render(vec![out(1.0, data)], json!({ "hide_rows": [23] }));

        assert_eq!(pairs(&rendered.events), vec![], "{data:?}");
    }
}

#[test]
fn mode_12_passes_through_once_for_each_mention_in_a_list() {
    let rendered = render(
        vec![out(1.0, "\x1b[?12;25h"), out(2.0, "\x1b[?25;12l")],
        json!({ "hide_rows": [23] }),
    );

    assert_eq!(
        pairs(&rendered.events),
        vec![(1.0, "\x1b[?12h"), (2.0, "\x1b[?12l\x1b[?25l")]
    );
}

#[test]
fn the_pass_through_comes_before_the_paint() {
    let rendered = render(vec![out(1.0, "\x07A")], json!({ "hide_rows": [23] }));

    assert_eq!(pairs(&rendered.events), vec![(1.0, "\x07A")]);
}

#[test]
fn the_sequences_that_the_paint_replaces_do_not_pass_through() {
    for data in ["\x1b[?1049h", "\x1b(0", "\x1b[2J", "\x1b[H", "\x1b[1;31m"] {
        let rendered = render(vec![out(1.0, data)], json!({ "hide_rows": [23] }));

        assert!(
            rendered
                .events
                .iter()
                .all(|event| !event.data.contains(data)),
            "{data:?}: {:?}",
            rendered.events
        );
    }
}

#[test]
fn an_osc_that_two_events_split_passes_in_the_event_where_it_ends() {
    let rendered = render(
        vec![out(1.0, "\x1b]2;ti"), out(2.0, "tle\x07")],
        json!({ "hide_rows": [23] }),
    );

    assert_eq!(pairs(&rendered.events), vec![(2.0, "\x1b]2;title\x07")]);
}

// Tests of the events.

#[test]
fn an_input_event_stays_with_its_time() {
    let rendered = render(
        vec![input(0.5, "q"), out(1.0, "A"), input(2.0, "w")],
        json!({ "hide_rows": [23] }),
    );

    assert_eq!(
        pairs(&rendered.events),
        vec![(0.5, "q"), (1.0, "A"), (2.0, "w")]
    );
    assert_eq!(rendered.events[0].event_type, EventType::Input);
    assert_eq!(rendered.events[2].event_type, EventType::Input);
}

#[test]
fn an_output_event_that_changes_nothing_is_removed() {
    let rendered = render(
        vec![out(1.0, "\x1b[2J"), out(2.0, "\x1b[?1049h"), out(3.0, "A")],
        json!({ "hide_rows": [23] }),
    );

    assert_eq!(pairs(&rendered.events), vec![(3.0, "A")]);
}

// Tests of the writer.

/// A glyph with the default look.
fn glyph(ch: char) -> Glyph {
    Glyph {
        ch,
        look: Look::default(),
    }
}

/// A target of `width` by `height` that is blank except for `cells`, which
/// are `(column, row, glyph)`.
fn target(width: usize, height: usize, cells: &[(usize, usize, Glyph)]) -> Vec<Glyph> {
    let mut grid = vec![Glyph::BLANK; width * height];
    for (x, y, glyph) in cells {
        grid[y * width + x] = *glyph;
    }
    grid
}

#[test]
fn a_paint_of_the_model_writes_nothing() {
    let mut writer = Writer::blank(3, 2);

    assert_eq!(writer.paint(&target(3, 2, &[]), Some((0, 0))), "");
}

#[test]
fn a_paint_writes_a_cup_for_a_cell_that_the_cursor_is_not_on() {
    let mut writer = Writer::blank(3, 2);
    let cells = target(3, 2, &[(2, 1, glyph('A'))]);

    assert_eq!(writer.paint(&cells, Some((0, 0))), "\x1b[2;3HA\x1b[1;1H");
}

#[test]
fn a_paint_writes_the_cursor_visibility_when_it_changes() {
    let mut writer = Writer::blank(3, 2);
    let blank = target(3, 2, &[]);

    assert_eq!(writer.paint(&blank, None), "\x1b[?25l");
    assert_eq!(writer.paint(&blank, None), "");
    assert_eq!(writer.paint(&blank, Some((0, 0))), "\x1b[?25h");
    assert_eq!(writer.paint(&blank, Some((0, 0))), "");
}

#[test]
fn a_paint_writes_one_sgr_for_cells_with_the_same_look() {
    let mut writer = Writer::blank(4, 1);
    let look = Look {
        fg: ColorSource::Base(12),
        bg: ColorSource::Base(3),
        bold: true,
        italic: true,
        underline: true,
    };
    let styled = |ch| Glyph { ch, look };
    let cells = target(4, 1, &[(0, 0, styled('a')), (1, 0, styled('b'))]);

    assert_eq!(writer.paint(&cells, Some((2, 0))), "\x1b[0;1;3;4;94;43mab");
}

#[test]
fn a_paint_writes_each_form_of_a_color() {
    let cases = [
        (
            ColorSource::Base(1),
            ColorSource::Base(9),
            "\x1b[0;31;101mx",
        ),
        (
            ColorSource::Base(20),
            ColorSource::Base(20),
            "\x1b[0;38;5;20;48;5;20mx",
        ),
        (
            ColorSource::Indexed(200),
            ColorSource::Indexed(7),
            "\x1b[0;38;5;200;48;5;7mx",
        ),
        (
            ColorSource::Rgb(1, 2, 3),
            ColorSource::Rgb(4, 5, 6),
            "\x1b[0;38;2;1;2;3;48;2;4;5;6mx",
        ),
        (
            ColorSource::Default,
            ColorSource::Rgb(4, 5, 6),
            "\x1b[0;48;2;4;5;6mx",
        ),
        (ColorSource::Base(7), ColorSource::Default, "\x1b[0;37mx"),
    ];
    for (fg, bg, expected) in cases {
        let mut writer = Writer::blank(2, 1);
        let look = Look {
            fg,
            bg,
            ..Look::default()
        };
        let cells = target(2, 1, &[(0, 0, Glyph { ch: 'x', look })]);

        assert_eq!(writer.paint(&cells, Some((1, 0))), expected);
    }
}

#[test]
fn a_paint_resets_the_pen_for_a_default_cell_after_a_styled_one() {
    let mut writer = Writer::blank(3, 1);
    let red = Glyph {
        ch: 'r',
        look: Look {
            fg: ColorSource::Base(1),
            ..Look::default()
        },
    };

    let first = writer.paint(&target(3, 1, &[(0, 0, red)]), Some((1, 0)));
    let second = writer.paint(
        &target(3, 1, &[(0, 0, red), (1, 0, glyph('d'))]),
        Some((2, 0)),
    );

    assert_eq!(first, "\x1b[0;31mr");
    assert_eq!(second, "\x1b[0md");
}

#[test]
fn the_cursor_of_the_model_is_unknown_after_the_last_column() {
    let mut writer = Writer::blank(2, 2);
    let cells = target(2, 2, &[(1, 0, glyph('a')), (0, 1, glyph('b'))]);

    assert_eq!(writer.paint(&cells, Some((1, 1))), "\x1b[1;2Ha\x1b[2;1Hb");
}

// The core property: the output shows the crop of the screen.

/// A crop of the screen: what the options say.
#[derive(Clone, Default)]
struct Shape {
    rows: Option<(u16, u16)>,
    columns: Option<(u16, u16)>,
    hide_rows: Vec<u16>,
    hide_columns: Vec<u16>,
}

impl Shape {
    /// The options of the project file.
    fn options(&self) -> serde_json::Value {
        let mut value = json!({
            "hide_rows": self.hide_rows,
            "hide_columns": self.hide_columns,
        });
        if let Some((start, end)) = self.rows {
            value["rows"] = json!({ "start": start, "end": end });
        }
        if let Some((start, end)) = self.columns {
            value["columns"] = json!({ "start": start, "end": end });
        }
        value
    }

    /// The visible indexes of an axis of `len` cells.
    fn visible(len: u16, crop: Option<(u16, u16)>, hidden: &[u16]) -> Vec<usize> {
        let (start, end) = crop.unwrap_or((0, len));
        (start..end)
            .filter(|index| !hidden.contains(index))
            .map(usize::from)
            .collect()
    }
}

/// The shapes that the property runs for each stream.
fn shapes() -> Vec<Shape> {
    vec![
        Shape {
            hide_rows: vec![2],
            ..Shape::default()
        },
        Shape {
            hide_columns: vec![1, 5],
            ..Shape::default()
        },
        Shape {
            rows: Some((1, 5)),
            columns: Some((2, 15)),
            ..Shape::default()
        },
        Shape {
            hide_rows: vec![0, 2, 4],
            hide_columns: vec![3],
            ..Shape::default()
        },
        Shape {
            hide_rows: vec![3],
            columns: Some((0, 18)),
            hide_columns: vec![0, 17],
            ..Shape::default()
        },
    ]
}

/// The shapes that hide no column, for the streams with wide characters.
fn row_shapes() -> Vec<Shape> {
    shapes()
        .into_iter()
        .filter(|shape| shape.hide_columns.is_empty() && shape.columns.is_none())
        .collect()
}

const WIDTH: u16 = 20;
const HEIGHT: u16 = 6;

/// Feed each output event of `rendered` that is at or before `time` to
/// `terminal`, from the event at `next`.
fn feed_until(rendered: &Recording, next: &mut usize, terminal: &mut TerminalState, time: f64) {
    feed_events_until(&rendered.events, next, terminal, time);
}

/// Check that the grid and the cursor of `output` are the crop of `input`.
fn assert_crop(
    input: &TerminalState,
    output: &TerminalState,
    rows: &[usize],
    columns: &[usize],
    label: &str,
) {
    for (row_index, row) in rows.iter().enumerate() {
        for (column_index, column) in columns.iter().enumerate() {
            let want = &input.get_grid()[*row][*column];
            let got = &output.get_grid()[row_index][column_index];
            let at = format!("{label}: cell ({column}, {row})");
            assert_eq!(got.ch, want.ch, "{at}: character");
            assert_eq!(got.fg_source, want.fg_source, "{at}: foreground");
            assert_eq!(got.bg_source, want.bg_source, "{at}: background");
            assert_eq!(got.bold, want.bold, "{at}: bold");
            assert_eq!(got.italic, want.italic, "{at}: italic");
            assert_eq!(got.underline, want.underline, "{at}: underline");
            assert_eq!(got.fg_color, want.fg_color, "{at}: foreground rgb");
            assert_eq!(got.bg_color, want.bg_color, "{at}: background rgb");
        }
    }
    let (cursor_x, cursor_y) = input.get_cursor_position();
    let mapped = columns
        .iter()
        .position(|column| *column == cursor_x)
        .zip(rows.iter().position(|row| *row == cursor_y));
    match mapped {
        Some(position) if input.is_cursor_visible() => {
            assert!(output.is_cursor_visible(), "{label}: the cursor is hidden");
            assert_eq!(
                output.get_cursor_position(),
                position,
                "{label}: cursor position"
            );
        }
        _ => assert!(
            !output.is_cursor_visible(),
            "{label}: the cursor is shown at {:?}, the input cursor is at ({cursor_x}, {cursor_y})",
            output.get_cursor_position()
        ),
    }
}

/// Replay `stream` on a screen of `WIDTH` by `HEIGHT` and check, after each
/// event, that the replay of the rendered output is the crop of the replay of
/// the input. Both replays use `theme`.
fn assert_replays_as_crop(stream: &[&str], shape: &Shape, theme: fn() -> TerminalTheme) {
    let events: Vec<Event> = stream
        .iter()
        .enumerate()
        .map(|(index, data)| out(f64::from(u16::try_from(index + 1).unwrap()), data))
        .collect();
    let rendered = render_on(
        WIDTH,
        HEIGHT,
        events.clone(),
        shape.options(),
        FilterRegion::All,
    )
    .unwrap();
    let rows = Shape::visible(HEIGHT, shape.rows, &shape.hide_rows);
    let columns = Shape::visible(WIDTH, shape.columns, &shape.hide_columns);
    assert_eq!(
        (usize::from(rendered.width), usize::from(rendered.height)),
        (columns.len(), rows.len())
    );
    let mut input = TerminalState::new(usize::from(WIDTH), usize::from(HEIGHT), theme());
    let mut output = TerminalState::new(columns.len(), rows.len(), theme());
    let mut next = 0;

    feed_until(&rendered, &mut next, &mut output, 0.0);
    assert_crop(&input, &output, &rows, &columns, "at the start");
    for (index, event) in events.iter().enumerate() {
        input.process_output(&event.data).unwrap();
        feed_until(&rendered, &mut next, &mut output, event.time);
        let label = format!(
            "after event {index} ({:?}) of {:?}",
            event.data,
            shape.options()
        );
        assert_crop(&input, &output, &rows, &columns, &label);
    }
    assert_eq!(next, rendered.events.len(), "every event is replayed");
}

/// Run the property for `stream` with each of `shapes`.
fn assert_each_shape(stream: &[&str], shapes: &[Shape]) {
    for shape in shapes {
        assert_replays_as_crop(stream, shape, TerminalTheme::auto);
    }
}

#[test]
fn the_output_shows_the_crop_of_plain_text_on_several_lines() {
    assert_each_shape(
        &["one\r\n", "two\r\nthree\r\n", "four\r\n", "five six seven"],
        &shapes(),
    );
}

#[test]
fn the_output_shows_the_crop_of_each_kind_of_color() {
    assert_each_shape(
        &[
            "\x1b[31mred \x1b[95mbright \x1b[38;5;200midx\x1b[0m\r\n",
            "\x1b[38;2;1;2;3mrgb \x1b[44mbg\x1b[1;3;4m styled\x1b[0m\r\n",
            "\x1b[48;5;21mx\x1b[48;2;9;8;7my\x1b[102mz\x1b[0m\r\n",
            "\x1b[4munder\x1b[24m\x1b[3mital\x1b[0m done",
        ],
        &shapes(),
    );
}

#[test]
fn the_output_shows_the_crop_after_a_scroll() {
    let lines: Vec<String> = (1..=12).map(|n| format!("line {n}\r\n")).collect();
    let stream: Vec<&str> = lines.iter().map(String::as_str).collect();

    assert_each_shape(&stream, &shapes());
}

#[test]
fn the_output_shows_the_crop_after_a_clear_and_an_erase() {
    assert_each_shape(
        &[
            "abc\r\ndef\r\nghi",
            "\x1b[2J",
            "\x1b[1;1Hxyz\x1b[K",
            "\x1b[2;3Hqrs\x1b[1;3H\x1b[K",
            "\x1b[41m\x1b[3;1H\x1b[K\x1b[0m",
        ],
        &shapes(),
    );
}

#[test]
fn the_output_shows_the_crop_after_cursor_moves() {
    assert_each_shape(
        &[
            "\x1b[5;10Hx",
            "\x1b[Ay",
            "\x1b[2;1H\x1b[1;4;31mz",
            "\x1b[3;3H",
            "\x1b[B\x1b[C\x1b[Cw",
        ],
        &shapes(),
    );
}

#[test]
fn the_output_shows_the_crop_of_line_drawing() {
    assert_each_shape(
        &[
            "\x1b(0lqqk\x1b(B\r\n",
            "\x1b(0x\x1b(B  \x1b(0x\x1b(B\r\n",
            "\x1b(0mqqj\x1b(B",
        ],
        &shapes(),
    );
}

#[test]
fn the_output_shows_the_crop_of_wide_characters() {
    assert_each_shape(
        &["日本\r\n", "a日b\r\n", "\x1b[31m語\x1b[0mx"],
        &row_shapes(),
    );
}

#[test]
fn the_output_shows_the_crop_when_the_cursor_goes_to_a_hidden_row_and_back() {
    let hidden_third = Shape {
        hide_rows: vec![2],
        ..Shape::default()
    };
    let stream = [
        "a\r\nb\r\nc",
        "\x1b[3;2Hx",
        "\x1b[1;1Hy",
        "\x1b[3;1H",
        "\x1b[4;1Hz",
        "\x1b[?25l",
        "\x1b[?25hq",
    ];

    assert_each_shape(&stream, &[hidden_third]);
    assert_each_shape(&stream, &shapes());
}

#[test]
fn the_output_shows_the_crop_when_hidden_rows_are_not_next_to_each_other() {
    let apart = Shape {
        hide_rows: vec![0, 2, 4],
        hide_columns: vec![1, 3, 5],
        ..Shape::default()
    };

    assert_each_shape(
        &["abcdefgh\r\nijklmnop\r\nqrstuvwx\r\nyz012345\r\n6789abcd"],
        &[apart],
    );
}

#[test]
fn the_theme_still_applies_to_the_output() {
    for shape in shapes() {
        assert_replays_as_crop(
            &[
                "\x1b[31mred \x1b[95mbright \x1b[38;5;200midx\x1b[0m\r\n",
                "\x1b[44mbg\x1b[0m x\r\n",
            ],
            &shape,
            TerminalTheme::dracula,
        );
    }
}

// Tests of the region `Range`.

/// The region from `start` to `end` seconds.
fn span(start: f64, end: f64) -> FilterRegion {
    FilterRegion::Range {
        start: Seconds::new(start).unwrap(),
        end: Seconds::new(end).unwrap(),
    }
}

/// Feed each output event of `events` that is at or before `time` to
/// `terminal`, from the event at `next`.
fn feed_events_until(events: &[Event], next: &mut usize, terminal: &mut TerminalState, time: f64) {
    while let Some(event) = events.get(*next) {
        if event.time > time {
            break;
        }
        if event.event_type == EventType::Output {
            terminal.process_output(&event.data).unwrap();
        }
        *next += 1;
    }
}

/// Check that each cell of `output` is the cell of `reference`, or a blank
/// cell when `mask` gives the visible rows and columns. Check the cursor in
/// the same way.
fn assert_screen(
    reference: &TerminalState,
    output: &TerminalState,
    mask: Option<(&[usize], &[usize])>,
    label: &str,
) {
    let shown = |row: usize, column: usize| {
        mask.is_none_or(|(rows, columns)| rows.contains(&row) && columns.contains(&column))
    };
    for (row, cells) in reference.get_grid().iter().enumerate() {
        for (column, cell) in cells.iter().enumerate() {
            let want = if shown(row, column) {
                glyph_of(cell)
            } else {
                Glyph::BLANK
            };
            let got = glyph_of(&output.get_grid()[row][column]);
            assert_eq!(got, want, "{label}: cell ({column}, {row})");
        }
    }
    let (x, y) = reference.get_cursor_position();
    if reference.is_cursor_visible() && shown(y, x) {
        assert!(output.is_cursor_visible(), "{label}: the cursor is hidden");
        assert_eq!(output.get_cursor_position(), (x, y), "{label}: cursor");
    } else {
        assert!(!output.is_cursor_visible(), "{label}: the cursor is shown");
    }
}

/// Render `stream`, a list of `(time, data)` pairs, with the filter `shape` on
/// the region from `start` to `end`. Replay the whole rendered recording and
/// the original events side by side. At each time of an event, and at the
/// start and the end of the range, check the replay of the rendered recording
/// against the replay of the original. Inside the range it must show the
/// masked screen. At the start of the range and after it, it must show the
/// screen of the original, the cursor included, with no mask.
///
/// Returns the rendered recording.
fn assert_range_property(stream: &[(f64, &str)], start: f64, end: f64, shape: &Shape) -> Recording {
    let events: Vec<Event> = stream.iter().map(|(time, data)| out(*time, data)).collect();
    let rendered = render_on(
        WIDTH,
        HEIGHT,
        events.clone(),
        shape.options(),
        span(start, end),
    )
    .unwrap();
    assert_eq!((rendered.width, rendered.height), (WIDTH, HEIGHT));
    let rows = Shape::visible(HEIGHT, shape.rows, &shape.hide_rows);
    let columns = Shape::visible(WIDTH, shape.columns, &shape.hide_columns);
    let closed = end < rendered.duration;
    let mut times: Vec<f64> = vec![start];
    times.extend(stream.iter().map(|(time, _)| *time));
    if closed {
        times.push(end);
    }
    times.sort_by(f64::total_cmp);
    times.dedup();
    let mut reference = TerminalState::new(
        usize::from(WIDTH),
        usize::from(HEIGHT),
        TerminalTheme::auto(),
    );
    let mut output = TerminalState::new(
        usize::from(WIDTH),
        usize::from(HEIGHT),
        TerminalTheme::auto(),
    );
    let (mut next_original, mut next_rendered) = (0, 0);
    for time in times {
        feed_events_until(&events, &mut next_original, &mut reference, time);
        feed_events_until(&rendered.events, &mut next_rendered, &mut output, time);
        let inside = time >= start && (!closed || time < end);
        let label = format!("at {time} s, {:?}", shape.options());
        let mask: Option<(&[usize], &[usize])> =
            inside.then_some((rows.as_slice(), columns.as_slice()));
        assert_screen(&reference, &output, mask, &label);
    }
    feed_events_until(&rendered.events, &mut next_rendered, &mut output, f64::MAX);
    assert_eq!(next_rendered, rendered.events.len());
    rendered
}

/// Run `assert_range_property` for each of `shapes()`.
fn assert_range_property_each_shape(stream: &[(f64, &str)], start: f64, end: f64) {
    for shape in shapes() {
        assert_range_property(stream, start, end, &shape);
    }
}

#[test]
fn a_range_masks_plain_text_and_gives_it_back_after_the_range() {
    let stream = [
        (1.0, "abcdefghijklmnop\r\nqrstuvwx"),
        (2.0, "\r\nyz0123456789"),
        (4.0, "\x1b[1;1HAAAAAAAAAAAAAAAAAAAA\r\nBBBBBBBBBBBBBBBBBBBB"),
        (5.0, "\r\nCCCCCCCCCCCCCCCCCCCC\r\nDDDDDDDDDDDDDDDDDDDD"),
        (7.0, "\r\nEEEEEEEEEEEEEEEEEEEE\x1b[2;3Hq"),
        (8.0, "\x1b[6;1Hend"),
    ];

    assert_range_property_each_shape(&stream, 3.0, 6.0);
    assert_range_property(
        &stream,
        3.0,
        6.0,
        &Shape {
            hide_rows: vec![0, 2, 4],
            hide_columns: vec![3],
            ..Shape::default()
        },
    );
}

#[test]
fn a_range_gives_the_pen_back_after_the_range() {
    let stream = [
        (1.0, "\x1b[32mgreen\x1b[0m\r\n"),
        (4.0, "\x1b[1;3;4;31;44mred bold\r\n"),
        (5.0, "\x1b[38;2;10;20;30mrgb \x1b[48;5;200mindexed\x1b[95m"),
        (7.0, "after one\r\nafter two"),
        (8.0, "\x1b[0mplain"),
    ];

    assert_range_property_each_shape(&stream, 3.0, 6.0);
}

#[test]
fn a_range_gives_the_scroll_region_back_after_the_range() {
    let stream = [
        (1.0, "a\r\nb\r\nc\r\nd\r\ne\r\nf"),
        (4.0, "\x1b[2;4r\x1b[4;1Hinside"),
        (7.0, "\x1b[4;1H\r\nX\r\nY\r\nZ"),
        (8.0, "\x1b[4;1H\nW\n\n"),
    ];

    assert_range_property_each_shape(&stream, 3.0, 6.0);
}

#[test]
fn a_range_gives_the_saved_cursor_back_after_the_range() {
    let stream = [
        (1.0, "start\r\n"),
        (4.0, "\x1b[3;5H\x1b7\x1b[1;1Hz"),
        (7.0, "\x1b8Q"),
        (8.0, "\x1b[sR\x1b[1;1H\x1b[uS"),
    ];

    assert_range_property_each_shape(&stream, 3.0, 6.0);
}

#[test]
fn a_range_keeps_the_line_drawing_set_before_it_and_gives_it_back() {
    let stream = [
        (1.0, "\x1b(0lqk\r\nx"),
        (4.0, "lqqqk\r\nx   x\r\nmqqqj"),
        (7.0, "\x1b[1;10Hlqk"),
        (8.0, "\x1b(B\x1b[2;10Hlqk"),
    ];

    assert_range_property_each_shape(&stream, 3.0, 6.0);
}

#[test]
fn a_range_gives_the_line_drawing_set_back_when_the_range_turns_it_on() {
    let stream = [(1.0, "ab"), (4.0, "\x1b(0lqk\r\n"), (7.0, "\x1b[3;1Hmqj")];

    assert_range_property_each_shape(&stream, 3.0, 6.0);
}

#[test]
fn a_range_gives_the_cursor_visibility_back_after_the_range() {
    for stream in [
        [(1.0, "abc"), (4.0, "\x1b[?25ldef"), (7.0, "ghi")],
        [(1.0, "abc\x1b[?25l"), (4.0, "\x1b[?25hdef"), (7.0, "ghi")],
    ] {
        assert_range_property_each_shape(&stream, 3.0, 6.0);
    }
}

#[test]
fn a_range_from_the_start_that_holds_each_event_keeps_the_size_and_writes_no_restore() {
    let stream = [
        (1.0, "abcdefghij\r\nklmnopqrst"),
        (2.0, "\x1b[2;4r\x1b[32mgreen\x1b7"),
        (3.0, "\r\nmore\x1b[?25l"),
    ];

    for shape in shapes() {
        let rendered = assert_range_property(&stream, 0.0, 200.0, &shape);

        assert!(
            rendered.events.iter().all(|event| event.time <= 3.0),
            "{:?}",
            rendered.events
        );
        assert!(
            rendered
                .events
                .iter()
                .all(|event| !event.data.contains("\x1b7") && !event.data.contains("r\x1b[")),
            "{:?}",
            rendered.events
        );
    }
}

#[test]
fn a_range_in_the_middle_of_a_recording_writes_the_start_and_the_restore() {
    let rendered = render_on(
        20,
        6,
        vec![out(1.0, "ab")],
        json!({ "hide_rows": [0] }),
        span(3.0, 6.0),
    )
    .unwrap();

    assert_eq!(
        pairs(&rendered.events),
        vec![
            (1.0, "ab"),
            (3.0, "\x1b[1;1H  \x1b[?25l"),
            (
                6.0,
                "\x1b[1;1Hab\x1b[1;6r\x1b[0m\x1b[1;1H\x1b7\x1b[1;3H\x1b[?25h"
            ),
        ]
    );
}

#[test]
fn the_restore_of_a_range_comes_before_an_event_after_it_at_the_same_time() {
    let rendered = render_on(
        20,
        6,
        vec![out(4.0, "x"), out(6.0, "y")],
        json!({ "hide_rows": [0] }),
        span(3.0, 6.0),
    )
    .unwrap();

    let at_end: Vec<&Event> = rendered.events.iter().filter(|e| e.time == 6.0).collect();
    assert_eq!(at_end.len(), 2, "{:?}", rendered.events);
    assert!(at_end[0].data.contains("\x1b7"), "{:?}", at_end[0]);
    assert_eq!(at_end[1].data, "y");
}

#[test]
fn the_rendered_size_of_a_range_is_the_size_of_the_screen() {
    for shape in shapes() {
        let rendered = render_on(
            WIDTH,
            HEIGHT,
            vec![out(1.0, "abc")],
            shape.options(),
            span(0.5, 2.0),
        )
        .unwrap();

        assert_eq!((rendered.width, rendered.height), (WIDTH, HEIGHT));
    }
}

#[test]
fn an_input_event_in_a_range_stays_and_one_before_it_is_not_fed_to_the_emulator() {
    let rendered = render_on(
        20,
        6,
        vec![input(1.0, "ab"), input(4.0, "q")],
        json!({ "hide_rows": [0] }),
        span(3.0, 6.0),
    )
    .unwrap();

    assert!(rendered
        .events
        .iter()
        .any(|event| event.event_type == EventType::Input
            && event.data == "q"
            && event.time == 4.0));
    // The emulator never saw "ab", thus the start has nothing to blank.
    assert_eq!(pairs(&rendered.events)[1], (3.0, "\x1b[?25l"));
}

#[test]
fn an_osc_that_the_start_of_a_range_splits_passes_in_the_first_event_of_the_range() {
    let rendered = render_on(
        20,
        6,
        vec![out(1.0, "\x1b]2;ti"), out(4.0, "tle\x07x")],
        json!({ "hide_rows": [5] }),
        span(3.0, 6.0),
    )
    .unwrap();

    let in_range: Vec<&Event> = rendered.events.iter().filter(|e| e.time == 4.0).collect();
    assert_eq!(in_range.len(), 1, "{:?}", rendered.events);
    assert!(
        in_range[0].data.starts_with("\x1b]2;title\x07"),
        "{:?}",
        in_range[0]
    );
}

#[test]
fn an_escape_sequence_that_the_start_of_a_range_splits_is_read_by_the_emulator() {
    let rendered = render_on(
        20,
        6,
        vec![out(1.0, "ab\x1b[3"), out(4.0, "1mred")],
        json!({ "hide_rows": [5] }),
        span(3.0, 6.0),
    )
    .unwrap();

    let in_range: Vec<&Event> = rendered.events.iter().filter(|e| e.time == 4.0).collect();
    assert_eq!(in_range.len(), 1, "{:?}", rendered.events);
    assert_eq!(in_range[0].data, "\x1b[0;31mred");
}

#[test]
fn a_control_sequence_that_the_end_of_a_range_splits_is_finished_after_the_range() {
    let stream = [
        (1.0, "before\r\n"),
        (4.0, "inside\x1b[3"),
        (7.0, "1mred after"),
        (8.0, "\r\nmore"),
    ];

    assert_range_property_each_shape(&stream, 3.0, 6.0);
}

#[test]
fn an_osc_that_the_end_of_a_range_splits_is_finished_after_the_range() {
    let stream = [
        (1.0, "before\r\n"),
        (4.0, "inside\x1b]2;ti"),
        (7.0, "tle\x07after"),
    ];

    assert_range_property_each_shape(&stream, 3.0, 6.0);
}
