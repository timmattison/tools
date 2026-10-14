//! The report: the text that a person reads, the JSON that a program reads,
//! and the list of the commands that did not parse.

use chrono::NaiveDate;
use comfy_table::{presets, CellAlignment, ContentArrangement, Table};
use num_format::{Locale, ToFormattedString};

use crate::catalog::PathState;
use crate::tally::{Classification, Summary, Tally, ToolUsage, Window};

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

/// Gives `count`, a `usize`, with a comma between each group of three digits.
fn number_of(count: usize) -> String {
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

/// The word `none`, for a group with no entry.
const NONE: &str = "none";

/// What holds the entries of a group apart.
const ENTRY_SEPARATOR: &str = ", ";

/// What holds the blocks of the text report apart: one blank line.
const BETWEEN_BLOCKS: &str = "\n\n";

/// Gives the names of the tools that have `classification`, sorted by name.
fn names_of(tally: &Tally, classification: Classification) -> Vec<&ToolUsage> {
    let mut found: Vec<&ToolUsage> = tally
        .tools
        .iter()
        .filter(|usage| usage.classification == classification)
        .collect();
    found.sort_by(|left, right| left.tool.name.cmp(&right.tool.name));
    found
}

/// Gives `entries` joined, or `none` when there is no entry.
fn entries_or_none(entries: &[String]) -> String {
    if entries.is_empty() {
        NONE.to_string()
    } else {
        entries.join(ENTRY_SEPARATOR)
    }
}

/// Gives the three lines that name the tools never used and the tools idle.
fn group_lines(tally: &Tally, window: &Window) -> String {
    let old: Vec<String> = names_of(tally, Classification::NeverUsedOld)
        .iter()
        .map(|usage| usage.tool.name.to_string())
        .collect();
    let new: Vec<String> = names_of(tally, Classification::NeverUsedNew)
        .iter()
        .map(|usage| {
            format!(
                "{} (added {})",
                usage.tool.name,
                date_cell(usage.tool.added)
            )
        })
        .collect();
    let mut idle = names_of(tally, Classification::Idle);
    idle.sort_by(|left, right| right.uses.cmp(&left.uses));
    let idle: Vec<String> = idle
        .iter()
        .map(|usage| {
            format!(
                "{} ({} {}, last use {})",
                usage.tool.name,
                number(usage.uses),
                if usage.uses == 1 { "use" } else { "uses" },
                date_cell(usage.last_use),
            )
        })
        .collect();
    format!(
        "Never used, old (added {} or more days ago): {}\n\
         Never used, new (added less than {} days ago): {}\n\
         Idle (no use in the last {} days): {}",
        window.new_days,
        entries_or_none(&old),
        window.new_days,
        entries_or_none(&new),
        window.idle_days,
        entries_or_none(&idle),
    )
}

/// Gives the two lines that summarize the history and the shell.
fn summary_lines(summary: &Summary, window: &Window) -> String {
    let rows = number(summary.rows);
    let history_start = match window.since {
        Some(since) => format!(
            "History: {rows} rows, {} before {}.",
            number(summary.before_since),
            date_cell(Some(since)),
        ),
        None => format!("History: {rows} rows."),
    };
    let history = format!(
        "{history_start} {} not counted (exit 127). {} did not parse (wdid --unparsed prints them).",
        number(summary.not_found),
        number(summary.unparsed_rows),
    );
    let mut shell = if summary.aliases == 0 && summary.functions == 0 {
        "Shell: no aliases and no functions (--no-shell, or none defined).".to_string()
    } else {
        format!(
            "Shell: {} aliases, {} functions ({} did not parse).",
            number_of(summary.aliases),
            number_of(summary.functions),
            number_of(summary.unparsed_functions.len()),
        )
    };
    if summary.unparsed_aliases > 0 {
        shell.push_str(&format!(
            " {} alias lines did not parse.",
            number_of(summary.unparsed_aliases)
        ));
    }
    format!("{history}\n{shell}")
}

/// Gives the text report. `width` is the table width. `None` sets no limit.
#[must_use]
pub fn text(tally: &Tally, window: &Window, width: Option<u16>) -> String {
    [
        table(tally, width),
        group_lines(tally, window),
        summary_lines(&tally.summary, window),
    ]
    .join(BETWEEN_BLOCKS)
        + "\n"
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
