use termstate::{TerminalState, TerminalTheme};

use super::*;

const WHITE: Rgb = (255, 255, 255);
const BLACK: Rgb = (0, 0, 0);
const RED: Rgb = (255, 85, 85);

fn terminal(columns: usize, rows: usize, output: &str) -> TerminalState {
    let mut state = TerminalState::new(columns, rows, TerminalTheme::auto());
    state.process_output(output);
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
