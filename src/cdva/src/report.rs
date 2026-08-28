//! The report: the default table.
//!
//! The table sets the test columns *inside* the code column rather than beside
//! it. `Test code` is a part of `Code`, so a reader who adds the two together
//! double counts, and a bar after `Code` is what says so without a footnote.
//!
//! Every column is as wide as the widest of its header and its values, and a
//! label is measured in the columns a terminal draws it in rather than in the
//! bytes it holds. A language name is ASCII today, but the same renderer prints
//! a path when `--by-file` arrives, and a path holds whatever a file system
//! allows. A width counted in bytes turns one such path into a broken column
//! for every row under it, and a width counted in characters breaks the same
//! column the other way, because one Japanese character draws in two.
//!
//! The output carries no color, so a pipe and a terminal read the same bytes.

use crate::counts::{Row, Summary};
use num_format::{Locale, ToFormattedString};
use unicode_width::UnicodeWidthStr;

/// The number of columns the table prints.
const COLUMNS: usize = 8;

/// The heading of each column, in the order the columns print.
const HEADERS: [&str; COLUMNS] = [
    "Language",
    "Files",
    "Blank",
    "Comment",
    "Code",
    "Test files",
    "Test code",
    "Test %",
];

/// The index of the first column, which is the only one that aligns left.
const LABEL_COLUMN: usize = 0;

/// The index of the column the bar follows.
const CODE_COLUMN: usize = 4;

/// What separates two columns.
const SEPARATOR: &str = "  ";

/// What follows the `Code` column, before the separator: the mark that says the
/// test columns are a part of the code column and not a column beside it.
const BAR: &str = " |";

/// The glyph the rule lines are drawn with.
const RULE_GLYPH: char = '-';

/// Which side of its column a cell sits on.
#[derive(Clone, Copy)]
enum Align {
    /// The cell starts at the left edge of the column, as a name reads.
    Left,
    /// The cell ends at the right edge of the column, as a number reads.
    Right,
}

/// Render the default table. The `Test code` column is a part of `Code`, and
/// not a column beside it.
///
/// A summary of no rows still prints a header, one rule, and a zeroed total,
/// because the shape of the answer is a part of the answer: a reader who counted
/// an empty tree learns more from a table of zeros than from an empty screen.
#[must_use]
pub fn render_table(summary: &Summary) -> String {
    let header = HEADERS.map(str::to_string);
    let body: Vec<[String; COLUMNS]> = summary.rows.iter().map(cells).collect();
    let total = cells(&summary.total);

    let widths = widths(&header, &body, &total);
    let rule: String = std::iter::repeat_n(RULE_GLYPH, table_width(&widths)).collect();

    let mut table = String::new();
    push_line(&mut table, &line(&header, &widths));
    push_line(&mut table, &rule);
    for row in &body {
        push_line(&mut table, &line(row, &widths));
    }
    // With no row between them the two rules would sit on top of one another,
    // and one line of dashes says everything two of them would.
    if !body.is_empty() {
        push_line(&mut table, &rule);
    }
    push_line(&mut table, &line(&total, &widths));
    table
}

/// The cells of one row, as the table prints them.
///
/// The counts of the two buckets are added here rather than by the caller,
/// because `Code` is the whole code of the row and `Test code` is the test share
/// of that same number. A column that read one bucket alone would print two
/// numbers a reader would add together and double count.
fn cells(row: &Row) -> [String; COLUMNS] {
    [
        row.label.clone(),
        thousands(row.files),
        thousands(row.blank()),
        thousands(row.comment()),
        thousands(row.code()),
        thousands(row.test_files),
        thousands(row.test.code),
        format!("{:.1}%", row.test_percent()),
    ]
}

/// A count, with a separator every three digits.
fn thousands(count: u64) -> String {
    count.to_formatted_string(&Locale::en)
}

/// The width of each column: the widest of its header and its values.
fn widths(
    header: &[String; COLUMNS],
    body: &[[String; COLUMNS]],
    total: &[String; COLUMNS],
) -> [usize; COLUMNS] {
    let mut widths = [0_usize; COLUMNS];
    for row in std::iter::once(header)
        .chain(body.iter())
        .chain(std::iter::once(total))
    {
        for (width, cell) in widths.iter_mut().zip(row.iter()) {
            *width = (*width).max(cell.width());
        }
    }
    widths
}

/// The width of the whole table, which is the width of a rule line.
fn table_width(widths: &[usize; COLUMNS]) -> usize {
    let columns: usize = widths.iter().sum();
    columns + (COLUMNS - 1) * SEPARATOR.len() + BAR.len()
}

/// One row of the table, padded column by column.
fn line(row: &[String; COLUMNS], widths: &[usize; COLUMNS]) -> String {
    let mut line = String::new();
    for (index, (cell, width)) in row.iter().zip(widths.iter()).enumerate() {
        if index > 0 {
            if index == CODE_COLUMN + 1 {
                line.push_str(BAR);
            }
            line.push_str(SEPARATOR);
        }
        let align = if index == LABEL_COLUMN {
            Align::Left
        } else {
            Align::Right
        };
        line.push_str(&pad(cell, *width, align));
    }
    line
}

/// `cell`, padded with spaces to `width` columns of a terminal.
///
/// A cell wider than its column is left alone rather than cut. The widths come
/// from the cells themselves, so that cannot happen here; a cut would be a
/// silent lie about the contents, and a wide line is one a reader can see.
fn pad(cell: &str, width: usize, align: Align) -> String {
    let filler: String = std::iter::repeat_n(' ', width.saturating_sub(cell.width())).collect();
    match align {
        Align::Left => format!("{cell}{filler}"),
        Align::Right => format!("{filler}{cell}"),
    }
}

/// Adds one line to the table, and the break that ends it.
fn push_line(table: &mut String, line: &str) {
    table.push_str(line);
    table.push('\n');
}
