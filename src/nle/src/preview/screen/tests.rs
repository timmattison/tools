use termstate::{TerminalState, TerminalTheme};

use super::*;

const WHITE: Rgb = (255, 255, 255);
const BLACK: Rgb = (0, 0, 0);
const RED: Rgb = (255, 85, 85);

fn terminal(columns: usize, rows: usize, output: &str) -> TerminalState {
    let mut state = TerminalState::new(columns, rows, TerminalTheme::auto());
    state.process_output(output).expect("the output parses");
    state
}

fn plain() -> Style {
    Style {
        foreground: WHITE,
        background: BLACK,
        bold: false,
        italic: false,
        underline: false,
    }
}

fn texts(row: &Row) -> Vec<&str> {
    row.runs.iter().map(|run| run.text.as_str()).collect()
}

#[test]
fn plain_text_is_one_run_in_each_row() {
    let screen = Screen::of(&terminal(4, 2, "\x1b[?25lab"));
    assert_eq!(screen.columns, 4);
    assert_eq!(screen.background, BLACK);
    assert_eq!(screen.row_texts(), ["ab  ", "    "]);
    assert_eq!(screen.rows.len(), 2);
    for (row, text) in screen.rows.iter().zip(["ab  ", "    "]) {
        assert_eq!(
            row.runs,
            [Run {
                text: text.to_string(),
                style: plain(),
                single_cell: false
            }]
        );
    }
}

#[test]
fn a_change_of_style_splits_the_run() {
    let screen = Screen::of(&terminal(5, 1, "\x1b[?25la\x1b[31;1mb\x1b[0mc"));
    let row = &screen.rows[0];
    assert_eq!(texts(row), ["a", "b", "c  "]);
    assert_eq!(row.runs[0].style, plain());
    assert_eq!(
        row.runs[1].style,
        Style {
            foreground: RED,
            bold: true,
            ..plain()
        }
    );
    assert_eq!(row.runs[2].style, plain());
}

#[test]
fn underline_and_italic_show_in_the_style() {
    let screen = Screen::of(&terminal(3, 1, "\x1b[?25l\x1b[4mu\x1b[0m\x1b[3mi"));
    let row = &screen.rows[0];
    assert_eq!(texts(row), ["u", "i", " "]);
    assert!(row.runs[0].style.underline && !row.runs[0].style.italic);
    assert!(row.runs[1].style.italic && !row.runs[1].style.underline);
}

#[test]
fn a_visible_cursor_swaps_the_colors_of_its_cell() {
    let screen = Screen::of(&terminal(4, 1, "ab"));
    let row = &screen.rows[0];
    assert_eq!(texts(row), ["ab", " ", " "]);
    assert_eq!(
        row.runs[1].style,
        Style {
            foreground: BLACK,
            background: WHITE,
            ..plain()
        }
    );
    assert_eq!(row.runs[0].style, plain());
    assert_eq!(row.runs[2].style, plain());
}

#[test]
fn the_cursor_swaps_the_colors_only_in_its_own_row() {
    let screen = Screen::of(&terminal(3, 2, "ab\r\nc"));
    let swapped = Style {
        foreground: BLACK,
        background: WHITE,
        ..plain()
    };
    assert_eq!(texts(&screen.rows[0]), ["ab "]);
    assert_eq!(screen.rows[0].runs[0].style, plain());
    assert_eq!(texts(&screen.rows[1]), ["c", " ", " "]);
    assert_eq!(screen.rows[1].runs[0].style, plain());
    assert_eq!(screen.rows[1].runs[1].style, swapped);
    assert_eq!(screen.rows[1].runs[2].style, plain());
}

#[test]
fn a_hidden_cursor_changes_no_cell() {
    let screen = Screen::of(&terminal(4, 1, "\x1b[?25lab"));
    assert_eq!(texts(&screen.rows[0]), ["ab  "]);
}

#[test]
fn a_wide_character_is_a_run_of_its_own() {
    let screen = Screen::of(&terminal(6, 1, "\x1b[?25l日本ab"));
    let row = &screen.rows[0];
    assert_eq!(texts(row), ["日", "本", "ab"]);
    assert_eq!(
        row.runs
            .iter()
            .map(|run| run.single_cell)
            .collect::<Vec<_>>(),
        [true, true, false]
    );
}

#[test]
fn a_control_character_shows_as_a_space() {
    let mut state = terminal(3, 1, "\x1b[?25lab");
    state.grid[0][0].ch = '\0';
    let screen = Screen::of(&state);
    assert_eq!(screen.row_texts(), [" b "]);
}

#[test]
fn hex_is_lowercase_with_two_digits_each() {
    assert_eq!(hex((40, 42, 54)), "#282a36");
    assert_eq!(hex((0, 0, 0)), "#000000");
    assert_eq!(hex((255, 171, 1)), "#ffab01");
}

#[test]
fn css_of_a_plain_style_gives_the_colors() {
    assert_eq!(plain().css(), "color:#ffffff;background-color:#000000");
}

#[test]
fn css_adds_a_declaration_for_each_flag() {
    let style = Style {
        bold: true,
        italic: true,
        underline: true,
        ..plain()
    };
    assert_eq!(
        style.css(),
        "color:#ffffff;background-color:#000000;font-weight:bold;font-style:italic;text-decoration:underline"
    );
}

#[test]
fn the_background_of_the_screen_follows_the_theme() {
    let state = TerminalState::new(2, 1, TerminalTheme::dracula());
    assert_eq!(Screen::of(&state).background, (40, 42, 54));
}

#[test]
fn the_background_of_the_screen_follows_an_osc_11_change() {
    let state = terminal(2, 1, "\x1b]11;rgb:10/20/30\x07");
    assert_eq!(Screen::of(&state).background, (0x10, 0x20, 0x30));
}

#[test]
fn each_pair_of_cell_colors_resolves_one_time() {
    // The cursor is hidden, because the cell under it swaps its colors.
    let state = terminal(
        200,
        60,
        "\x1b[?25l\x1b[31mred\x1b[0m plain \x1b[42mgreen\x1b[0m\r\n\x1b[1;34mblue\x1b[0m",
    );
    let pairs: std::collections::HashSet<_> = state
        .get_grid()
        .iter()
        .flatten()
        .map(|cell| (cell.fg_color, cell.bg_color))
        .collect();
    let foregrounds: std::collections::HashSet<_> = pairs.iter().map(|&(fg, _)| fg).collect();
    assert!(
        foregrounds.len() < pairs.len(),
        "the grid holds one foreground on two backgrounds"
    );
    let mut colors = Colors::new(&state);
    let screen = Screen::of_with(&mut colors);
    // The `+ 1` is the blank cell that gives the background of the screen.
    assert!(
        colors.resolutions() <= pairs.len() + 1,
        "{} resolutions for {} pairs of colors",
        colors.resolutions(),
        pairs.len()
    );
    assert_eq!(screen.rows.len(), state.get_grid().len());
    for (row_index, (cells, row)) in state.get_grid().iter().zip(&screen.rows).enumerate() {
        let drawn: Vec<ColorPair> = row
            .runs
            .iter()
            .flat_map(|run| {
                run.text
                    .chars()
                    .map(move |_| (run.style.foreground, run.style.background))
            })
            .collect();
        let resolved: Vec<ColorPair> = cells
            .iter()
            .map(|cell| state.resolve_cell_colors(cell))
            .collect();
        assert_eq!(drawn, resolved, "the colors of row {row_index}");
    }
}
