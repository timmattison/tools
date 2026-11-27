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
