//! The report: the text that a person reads, the JSON that a program reads,
//! and the list of the commands that did not parse.

use chrono::NaiveDate;
use comfy_table::{presets, CellAlignment, ContentArrangement, Table};
use num_format::{Locale, ToFormattedString};

use crate::catalog::PathState;
use crate::tally::{Tally, ToolUsage, Window};

/// The name of each column of the table, in order.
const COLUMNS: [&str; 9] = [
    "Tool",
    "Uses",
    "Typed",
    "Through an alias or a function",
    "Dev runs",
    "First use",
    "Last use",
    "Added",
    "Notes",
];

/// The places of the columns that hold a count. The table aligns them to the
/// right, so the digits of two rows line up.
const COUNT_COLUMNS: [usize; 3] = [1, 2, 4];

/// The text in place of a date that the tally does not know.
const NO_DATE: &str = "-";

/// The format of a date in the report.
const DATE_FORMAT: &str = "%Y-%m-%d";

/// What holds the parts of the `Through` cell apart.
const THROUGH_SEPARATOR: &str = ", ";

/// The note of a tool that `cargo install --list` does not name.
const NOTE_NOT_INSTALLED: &str = "not installed";

/// The note of a tool that no directory of the PATH holds.
const NOTE_NOT_ON_PATH: &str = "not on PATH";

/// The words after the path in the note of a tool that another program shadows.
const NOTE_RUNS_FIRST: &str = "runs first";

/// Gives `count` with a comma between each group of three digits.
fn number(count: u64) -> String {
    count.to_formatted_string(&Locale::en)
}

/// Gives `date` as `YYYY-MM-DD`, or `-` when the date is not known.
fn date_cell(date: Option<NaiveDate>) -> String {
    date.map_or_else(
        || NO_DATE.to_string(),
        |date| date.format(DATE_FORMAT).to_string(),
    )
}

/// Gives the `Through` cell: each name with its count, most used first.
fn through_cell(usage: &ToolUsage) -> String {
    usage
        .through
        .iter()
        .map(|(name, count)| format!("{name} {}", number(*count)))
        .collect::<Vec<_>>()
        .join(THROUGH_SEPARATOR)
}

/// Gives the `Notes` cell: where the PATH finds the tool.
fn note_cell(usage: &ToolUsage) -> String {
    match &usage.tool.path {
        None => NOTE_NOT_INSTALLED.to_string(),
        Some(PathState::Cargo) => String::new(),
        Some(PathState::Shadowed(path)) => {
            format!("{} {NOTE_RUNS_FIRST}", path.to_string_lossy())
        }
        Some(PathState::NotOnPath) => NOTE_NOT_ON_PATH.to_string(),
    }
}

/// Gives the table of the tools, one row for each tool in tally order.
///
/// `width` wraps the table at that many columns. `None` makes each column as
/// wide as its widest cell. The table never reads the terminal itself, because
/// a read of the terminal makes the text depend on where the tool runs.
fn table(tally: &Tally, width: Option<u16>) -> String {
    let mut table = Table::new();
    table
        .load_preset(presets::UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .force_no_tty()
        .set_header(COLUMNS);
    if let Some(width) = width {
        table.set_width(width);
    }
    for usage in &tally.tools {
        table.add_row([
            usage.tool.name.to_string(),
            number(usage.uses),
            number(usage.typed),
            through_cell(usage),
            number(usage.development_runs),
            date_cell(usage.first_use),
            date_cell(usage.last_use),
            date_cell(usage.tool.added),
            note_cell(usage),
        ]);
    }
    for index in COUNT_COLUMNS {
        if let Some(column) = table.column_mut(index) {
            column.set_cell_alignment(CellAlignment::Right);
        }
    }
    table.to_string()
}

/// Gives the text report. `width` is the table width. `None` sets no limit.
#[must_use]
pub fn text(tally: &Tally, window: &Window, width: Option<u16>) -> String {
    let _ = window;
    table(tally, width)
}

/// Gives the JSON document, pretty-printed, with a final newline.
#[must_use]
pub fn json(tally: &Tally, window: &Window) -> String {
    let _ = (tally, window);
    String::new()
}

/// Gives the unparsed commands, then the unparsed functions.
#[must_use]
pub fn unparsed(tally: &Tally) -> String {
    let _ = tally;
    String::new()
}

/// Gives the table width.
///
/// `stated` is the value of `COLUMNS`. It wins when it is a positive integer
/// that fits in a `u16`, because POSIX says that this value overrides the width
/// that the system selects. A value of 0 and a text that is not a number are
/// ignored. `terminal` is the width that the terminal reports. It answers when
/// `stated` does not, and only when it is positive. When neither source
/// answers, the result is `None`, and the table has no width limit.
#[must_use]
pub fn table_width(stated: Option<&str>, terminal: Option<u16>) -> Option<u16> {
    stated
        .and_then(|text| text.trim().parse::<u16>().ok())
        .filter(|width| *width > 0)
        .or_else(|| terminal.filter(|width| *width > 0))
}
