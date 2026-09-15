//! The rows of the worktree list, and the hint under them.
//!
//! [`rows`] draws the rows of the window that the list shows, and [`hint`]
//! draws the keys of the list. The frame of the list (`render_list_frame` in
//! `main.rs`) puts them under the head of the status frame.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the loop of watch mode draws the list from slice 5 of #499 on. The expectation \
                  then fails the build, so slice 5 deletes this attribute"
    )
)]

use colored::Colorize;
use textfit::pad_right;
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

/// Draw the rows of `window`, top to bottom.
///
/// A row is `{marker}{path}{pad}  [{label}]`, and the row of the home
/// worktree ends with `  ⌂`. The marker is `  > ` on the cursor row and blank
/// on every other row. The path column is as wide as the widest path in the
/// window, so the labels line up. The cursor row is bold.
pub(crate) fn rows(window: &[ListRow<'_>]) -> Vec<String> {
    let paths: Vec<String> = window
        .iter()
        .map(|row| row.entry.path.as_path().display().to_string())
        .collect();
    let column = paths
        .iter()
        .map(|path| UnicodeWidthStr::width(path.as_str()))
        .max()
        .unwrap_or(0);
    window
        .iter()
        .zip(&paths)
        .map(|(row, path)| {
            let marker = if row.cursor {
                CURSOR_MARKER
            } else {
                ROW_MARKER
            };
            let home = if row.home {
                format!("{GAP}{HOME_MARK}")
            } else {
                String::new()
            };
            let text = format!(
                "{marker}{}{GAP}[{}]{home}",
                pad_right(path, column),
                row.entry.label,
            );
            if row.cursor {
                text.bold().to_string()
            } else {
                text
            }
        })
        .collect()
}

/// Draw the keys of the list, dim, for the bottom row of the pane.
pub(crate) fn hint() -> String {
    HINT.dimmed().to_string()
}
