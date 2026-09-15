//! The rows of the worktree list, and the hint under them.
//!
//! [`rows`] draws the rows of the window that the list shows, and [`hint`]
//! draws the keys of the list. The frame of the list (`render_list_frame` in
//! `main.rs`) puts them under the head of the status frame.
//!
//! No row is wider than the pane, because a row that wraps pushes every row
//! under it down one line. Each function measures display width, never bytes,
//! so a multi-byte path loses whole characters and never panics.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the loop of watch mode draws the list from slice 5 of #499 on. The expectation \
                  then fails the build, so slice 5 deletes this attribute"
    )
)]

use colored::Colorize;
use textfit::{pad_right, truncate_left, truncate_to_budget};
use unicode_width::UnicodeWidthStr;

use super::HOME_MARK;
use crate::worktrees::ListRow;

/// The marker before the cursor row.
const CURSOR_MARKER: &str = "  > ";

/// The marker before every other row: blanks as wide as [`CURSOR_MARKER`],
/// so the paths of all rows start in one column.
const ROW_MARKER: &str = "    ";

// The paths line up only when the two markers are equally wide. Both are
// ASCII, so a count of bytes is a count of columns.
const _: () = assert!(CURSOR_MARKER.len() == ROW_MARKER.len());

/// The gap between two columns of a row: between the path and the label, and
/// between the label and the home mark.
const GAP: &str = "  ";

/// The keys of the list, on the bottom row of the pane.
const HINT: &str = "↑↓ move · Enter go · Esc back";

/// Draw the rows of `window`, top to bottom, for a pane of `width` columns.
///
/// A row is `{marker}{path}{pad}  [{label}]`, and the row of the home
/// worktree ends with `  ⌂`. The marker is `  > ` on the cursor row and blank
/// on every other row. The path column is as wide as the widest path in the
/// window, so the labels line up.
///
/// A row never exceeds `width`:
///
/// 1. The path column gets the columns that the marker and the widest end of
///    a row (the label, and the home mark) leave. A path wider than the
///    column loses columns from the left, because the end of a path names the
///    worktree. The column is the same for all rows, so the labels stay in
///    one column.
/// 2. A row that still does not fit, in a pane too narrow for its label,
///    loses columns from the right.
///
/// The cursor row is bold. The paint goes on after the cut, so the escape
/// codes cost no columns.
pub(crate) fn rows(window: &[ListRow<'_>], width: usize) -> Vec<String> {
    let paths: Vec<String> = window
        .iter()
        .map(|row| row.entry.path.as_path().display().to_string())
        .collect();
    let ends: Vec<String> = window.iter().map(row_end).collect();
    let widest_path = widest(&paths);
    let room = width.saturating_sub(UnicodeWidthStr::width(CURSOR_MARKER) + widest(&ends));
    let column = widest_path.min(room);
    window
        .iter()
        .zip(paths.iter().zip(&ends))
        .map(|(row, (path, end))| {
            let marker = if row.cursor {
                CURSOR_MARKER
            } else {
                ROW_MARKER
            };
            let cell = pad_right(&cut_path(path, column), column);
            let text = truncate_to_budget(&format!("{marker}{cell}{end}"), width);
            if row.cursor {
                text.bold().to_string()
            } else {
                text
            }
        })
        .collect()
}

/// Draw the keys of the list, dim, cut to `width` columns, for the bottom row
/// of the pane.
pub(crate) fn hint(width: usize) -> String {
    truncate_to_budget(HINT, width).dimmed().to_string()
}

/// The end of a row, after the path: the gap and the label in brackets, and
/// on the row of the home worktree, the gap and the home mark.
fn row_end(row: &ListRow<'_>) -> String {
    let home = if row.home {
        format!("{GAP}{HOME_MARK}")
    } else {
        String::new()
    };
    format!("{GAP}[{}]{home}", row.entry.label)
}

/// The display width of the widest of `texts`, or 0 for no text.
fn widest(texts: &[String]) -> usize {
    texts
        .iter()
        .map(|text| UnicodeWidthStr::width(text.as_str()))
        .max()
        .unwrap_or(0)
}

/// `path` cut from the left to `column` columns, with `…` where the cut is.
///
/// A column of no columns holds nothing. The `…` alone is one column wide,
/// so it does not fit there.
fn cut_path(path: &str, column: usize) -> String {
    if column == 0 {
        String::new()
    } else {
        truncate_left(path, column)
    }
}
