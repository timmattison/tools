//! The report: the text that a person reads, the JSON that a program reads,
//! and the list of the commands that did not parse.

use chrono::NaiveDate;
use comfy_table::{presets, CellAlignment, ContentArrangement, Table};
use num_format::{Locale, ToFormattedString};
use serde::Serialize;

use crate::catalog::PathState;
use crate::tally::{Classification, Summary, Tally, ToolUsage, Window};
use crate::tool::ToolName;

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

/// Gives the tools that have `classification`, sorted by name.
fn tools_with(tally: &Tally, classification: Classification) -> Vec<&ToolUsage> {
    let mut found: Vec<&ToolUsage> = tally
        .tools
        .iter()
        .filter(|usage| usage.classification == classification)
        .collect();
    found.sort_by(|left, right| left.tool.name.cmp(&right.tool.name));
    found
}

/// Gives the idle tools, the tool with the most uses first. Tools with equal
/// uses stay in name order.
fn idle_tools(tally: &Tally) -> Vec<&ToolUsage> {
    let mut idle = tools_with(tally, Classification::Idle);
    idle.sort_by_key(|usage| std::cmp::Reverse(usage.uses));
    idle
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
    let old: Vec<String> = tools_with(tally, Classification::NeverUsedOld)
        .iter()
        .map(|usage| usage.tool.name.to_string())
        .collect();
    let new: Vec<String> = tools_with(tally, Classification::NeverUsedNew)
        .iter()
        .map(|usage| {
            format!(
                "{} (added {})",
                usage.tool.name,
                date_cell(usage.tool.added)
            )
        })
        .collect();
    let idle: Vec<String> = idle_tools(tally)
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

/// The `history` block of the JSON document.
#[derive(Serialize)]
struct HistoryDocument<'a> {
    rows: u64,
    before_since: u64,
    not_found: u64,
    unparsed_rows: u64,
    unparsed: Vec<UnparsedDocument<'a>>,
}

/// One command that did not parse, in the JSON document.
#[derive(Serialize)]
struct UnparsedDocument<'a> {
    command: &'a str,
    rows: u64,
}

/// The `shell` block of the JSON document.
#[derive(Serialize)]
struct ShellDocument<'a> {
    aliases: usize,
    functions: usize,
    unparsed_functions: &'a [String],
    unparsed_aliases: usize,
}

/// One use through an alias or a function, in the JSON document.
#[derive(Serialize)]
struct ThroughDocument<'a> {
    name: &'a str,
    uses: u64,
}

/// The state of the PATH for one tool, in the JSON document.
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum PathDocument {
    Cargo,
    Shadowed { by: String },
    NotOnPath,
}

/// One tool in the JSON document.
#[derive(Serialize)]
struct ToolDocument<'a> {
    name: &'a ToolName,
    crate_dir: String,
    uses: u64,
    typed: u64,
    through: Vec<ThroughDocument<'a>>,
    development_runs: u64,
    first_use: Option<NaiveDate>,
    last_use: Option<NaiveDate>,
    added: Option<NaiveDate>,
    installed: bool,
    path: Option<PathDocument>,
    classification: Classification,
}

/// The whole JSON document.
#[derive(Serialize)]
struct Document<'a> {
    today: NaiveDate,
    since: Option<NaiveDate>,
    new_days: u32,
    idle_days: u32,
    history: HistoryDocument<'a>,
    shell: ShellDocument<'a>,
    tools: Vec<ToolDocument<'a>>,
    never_used_old: Vec<&'a ToolName>,
    never_used_new: Vec<&'a ToolName>,
    idle: Vec<&'a ToolName>,
}

/// Gives the JSON form of the state of the PATH.
fn path_document(state: &PathState) -> PathDocument {
    match state {
        PathState::Cargo => PathDocument::Cargo,
        PathState::Shadowed(path) => PathDocument::Shadowed {
            by: path.to_string_lossy().into_owned(),
        },
        PathState::NotOnPath => PathDocument::NotOnPath,
    }
}

/// Gives the JSON form of one tool.
fn tool_document(usage: &ToolUsage) -> ToolDocument<'_> {
    ToolDocument {
        name: &usage.tool.name,
        crate_dir: usage.tool.crate_dir.to_string_lossy().into_owned(),
        uses: usage.uses,
        typed: usage.typed,
        through: usage
            .through
            .iter()
            .map(|(name, uses)| ThroughDocument { name, uses: *uses })
            .collect(),
        development_runs: usage.development_runs,
        first_use: usage.first_use,
        last_use: usage.last_use,
        added: usage.tool.added,
        installed: usage.tool.installed,
        path: usage.tool.path.as_ref().map(path_document),
        classification: usage.classification,
    }
}

/// Gives the names of `tools`.
fn names_of<'a>(tools: &[&'a ToolUsage]) -> Vec<&'a ToolName> {
    tools.iter().map(|usage| &usage.tool.name).collect()
}

/// Gives the JSON document, pretty-printed, with a final newline.
///
/// The crate directory is a text with `/` separators. The three lists at the
/// end follow the order of the text report.
///
/// # Panics
///
/// Never in practice. The document holds only numbers, dates, and text, and
/// `serde_json` serializes each of them without an error.
#[must_use]
pub fn json(tally: &Tally, window: &Window) -> String {
    let summary = &tally.summary;
    let document = Document {
        today: window.today,
        since: window.since,
        new_days: window.new_days,
        idle_days: window.idle_days,
        history: HistoryDocument {
            rows: summary.rows,
            before_since: summary.before_since,
            not_found: summary.not_found,
            unparsed_rows: summary.unparsed_rows,
            unparsed: summary
                .unparsed
                .iter()
                .map(|group| UnparsedDocument {
                    command: &group.command,
                    rows: group.rows,
                })
                .collect(),
        },
        shell: ShellDocument {
            aliases: summary.aliases,
            functions: summary.functions,
            unparsed_functions: &summary.unparsed_functions,
            unparsed_aliases: summary.unparsed_aliases,
        },
        tools: tally.tools.iter().map(tool_document).collect(),
        never_used_old: names_of(&tools_with(tally, Classification::NeverUsedOld)),
        never_used_new: names_of(&tools_with(tally, Classification::NeverUsedNew)),
        idle: names_of(&idle_tools(tally)),
    };
    let mut text = serde_json::to_string_pretty(&document)
        .expect("a document of numbers, dates, and text always serializes");
    text.push('\n');
    text
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
