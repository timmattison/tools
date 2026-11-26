//! The report of the definitions: the text that a person reads and the JSON
//! that a program reads.

use chrono::NaiveDate;
use comfy_table::{presets, CellAlignment, ColumnConstraint, ContentArrangement, Table, Width};
use serde::Serialize;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{Classification, Kind, Summary, Tally, Usage, Window};
use crate::render::{
    counted, date_cell, group, number, number_of, BETWEEN_BLOCKS, THROUGH_SEPARATOR,
};

/// The name of each column of the table, in order.
const COLUMNS: [&str; 8] = [
    "Name",
    "Kind",
    "Uses",
    "Typed",
    "Through",
    "First use",
    "Last use",
    "Definition",
];

/// The place of the `Through` column.
const THROUGH_COLUMN: usize = 4;

/// The place of the `Definition` column.
const DEFINITION_COLUMN: usize = 7;

/// The places of the columns that hold a count. The table aligns them to the
/// right, so the digits of two rows line up.
const COUNT_COLUMNS: [usize; 2] = [2, 3];

/// The most display columns that the text of the `Through` column takes when
/// the report has a width. A longer cell then wraps inside this width. With no
/// width, the cell is one line.
const THROUGH_MAX: usize = 30;

/// The padding of `comfy_table` in one cell: one space on each side.
const CELL_PADDING: usize = 2;

/// The width of the border that follows a column, and of the padding of the
/// cell that the border belongs to: two spaces and one line.
const COLUMN_OVERHEAD: usize = CELL_PADDING + 1;

/// The width of the border at the left edge of the table.
const LEFT_BORDER: usize = 1;

/// The least display width of the text of the `Definition` column. When the
/// terminal is narrower, the table is wider than the terminal.
const DEFINITION_MIN: usize = 10;

/// The mark that ends a definition that the report cut.
const CUT_MARK: char = '\u{2026}';

/// The heading of the group of definitions that never ran.
const HEADING_NEVER: &str = "Never used";

/// What starts the shell line of the header.
const SHELL_LABEL: &str = "Shell";

/// Gives the `Through` cell: each name with its count, most used first.
fn through_cell(usage: &Usage) -> String {
    usage
        .through
        .iter()
        .map(|(name, count)| format!("{name} {}", number(*count)))
        .collect::<Vec<_>>()
        .join(THROUGH_SEPARATOR)
}

/// Gives `definition` cut to at most `available` display columns. A longer
/// definition becomes its longest prefix that leaves room for the cut mark,
/// then the mark. The function counts characters, never bytes.
fn cut(definition: &str, available: usize) -> String {
    if definition.width() <= available {
        return definition.to_string();
    }
    let room = available.saturating_sub(CUT_MARK.width().unwrap_or(1));
    let mut used = 0_usize;
    let mut kept = String::new();
    for character in definition.chars() {
        let step = character.width().unwrap_or(0);
        if used.saturating_add(step) > room {
            break;
        }
        used = used.saturating_add(step);
        kept.push(character);
    }
    kept.push(CUT_MARK);
    kept
}

/// Gives the cells of `usage` for the seven columns other than the definition,
/// in column order.
fn fixed_cells(usage: &Usage) -> [String; 7] {
    [
        usage.name.clone(),
        usage.kind.label().to_string(),
        number(usage.uses),
        number(usage.typed),
        through_cell(usage),
        date_cell(usage.first_use),
        date_cell(usage.last_use),
    ]
}

/// Gives the display width that the table needs for the text of the seven
/// columns other than the definition. The `Through` column counts at most
/// [`THROUGH_MAX`].
fn fixed_text_width(rows: &[[String; 7]]) -> usize {
    (0..DEFINITION_COLUMN)
        .map(|index| {
            let header = COLUMNS.get(index).map_or(0, |header| header.width());
            let widest = rows
                .iter()
                .filter_map(|row| row.get(index))
                .map(|cell| cell.width())
                .fold(header, usize::max);
            let width = if index == THROUGH_COLUMN {
                widest.min(THROUGH_MAX)
            } else {
                widest
            };
            width.saturating_add(COLUMN_OVERHEAD)
        })
        .fold(LEFT_BORDER, usize::saturating_add)
}

/// Gives the table of the definitions, one row for each entry in tally order.
///
/// `width` limits the table to that many display columns. The definition is
/// one line, cut to the room that the other columns leave. The `Through` cell
/// wraps inside [`THROUGH_MAX`] columns. `None` sets no limit, no cut, and no
/// wrap. The table never reads the terminal itself, because a read of the
/// terminal makes the text depend on where the tool runs.
fn table(tally: &Tally, width: Option<u16>) -> String {
    let rows: Vec<[String; 7]> = tally.definitions.iter().map(fixed_cells).collect();
    let available = width.map(|width| {
        usize::from(width)
            .saturating_sub(fixed_text_width(&rows))
            .saturating_sub(COLUMN_OVERHEAD)
            .max(DEFINITION_MIN)
    });
    let mut table = Table::new();
    table
        .load_preset(presets::UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .force_no_tty()
        .set_header(COLUMNS);
    if let Some(width) = width {
        table.set_width(width);
    }
    for (usage, fixed) in tally.definitions.iter().zip(rows) {
        let definition = available.map_or_else(
            || usage.definition.clone(),
            |available| cut(&usage.definition, available),
        );
        let mut cells = Vec::from(fixed);
        cells.push(definition);
        table.add_row(cells);
    }
    for index in COUNT_COLUMNS {
        if let Some(column) = table.column_mut(index) {
            column.set_cell_alignment(CellAlignment::Right);
        }
    }
    if width.is_some() {
        for index in 0..COLUMNS.len() {
            let constraint = if index == THROUGH_COLUMN {
                ColumnConstraint::UpperBoundary(Width::Fixed(through_limit()))
            } else {
                ColumnConstraint::ContentWidth
            };
            if let Some(column) = table.column_mut(index) {
                column.set_constraint(constraint);
            }
        }
    }
    table.to_string()
}

/// Gives the width of the `Through` column with its padding, as a `u16`.
fn through_limit() -> u16 {
    u16::try_from(THROUGH_MAX + CELL_PADDING).unwrap_or(u16::MAX)
}

/// Gives the definitions that have `classification`, sorted by name and kind.
fn with(tally: &Tally, classification: Classification) -> Vec<&Usage> {
    let mut found: Vec<&Usage> = tally
        .definitions
        .iter()
        .filter(|usage| usage.classification == classification)
        .collect();
    found.sort_by(|left, right| (&left.name, left.kind).cmp(&(&right.name, right.kind)));
    found
}

/// Gives the idle definitions, the definition with the most uses first.
/// Definitions with equal uses stay in name order.
fn idle_definitions(tally: &Tally) -> Vec<&Usage> {
    let mut idle = with(tally, Classification::Idle);
    idle.sort_by_key(|usage| std::cmp::Reverse(usage.uses));
    idle
}

/// Gives the two groups: the definitions never used and the definitions idle.
fn group_lines(tally: &Tally, window: &Window) -> String {
    let never: Vec<String> = with(tally, Classification::NeverUsed)
        .iter()
        .map(|usage| format!("{} ({})", usage.name, usage.kind.label()))
        .collect();
    let idle: Vec<String> = idle_definitions(tally)
        .iter()
        .map(|usage| {
            format!(
                "{} ({}, {}, last use {})",
                usage.name,
                usage.kind.label(),
                counted(usage.uses, "use", "uses"),
                date_cell(usage.last_use),
            )
        })
        .collect();
    [
        group(HEADING_NEVER, &never),
        group(
            &format!(
                "Idle (no use in the last {})",
                counted(window.idle_days, "day", "days")
            ),
            &idle,
        ),
    ]
    .join("\n")
}

/// Gives the two lines that describe the history and the shell.
fn header_lines(summary: &Summary, window: &Window, shell: &str) -> String {
    let rows = counted(summary.rows, "row", "rows");
    let history_start = match window.since {
        Some(since) => format!(
            "History: {rows}, {} before {}.",
            number(summary.before_since),
            date_cell(Some(since)),
        ),
        None => format!("History: {rows}."),
    };
    let history = format!(
        "{history_start} {} not counted (exit 127). {} did not parse.",
        number(summary.not_found),
        number(summary.unparsed_rows),
    );
    let mut shell_line = format!(
        "{SHELL_LABEL}: {shell}, {}, {} ({} did not parse).",
        counted(summary.aliases, "alias", "aliases"),
        counted(summary.functions, "function", "functions"),
        number_of(summary.unparsed_functions),
    );
    if summary.unparsed_aliases > 0 {
        shell_line.push_str(&format!(
            " {} did not parse.",
            counted(summary.unparsed_aliases, "alias line", "alias lines"),
        ));
    }
    format!("{history}\n{shell_line}")
}

/// Gives the text report. `shell` is the path of the shell that gave the
/// definitions. `width` is the table width. `None` sets no limit.
///
/// The report has three blocks: the header, the table, and the two groups.
/// The definition in the table is one line. It is cut to the room that the
/// width leaves.
#[must_use]
pub fn text(tally: &Tally, window: &Window, shell: &str, width: Option<u16>) -> String {
    [
        header_lines(&tally.summary, window, shell),
        table(tally, width),
        group_lines(tally, window),
    ]
    .join(BETWEEN_BLOCKS)
        + "\n"
}

/// The `history` block of the JSON document.
#[derive(Serialize)]
struct HistoryDocument {
    rows: u64,
    before_since: u64,
    not_found: u64,
    unparsed_rows: u64,
}

/// The `shell` block of the JSON document.
#[derive(Serialize)]
struct ShellDocument<'a> {
    path: &'a str,
    aliases: usize,
    functions: usize,
    unparsed_functions: usize,
    unparsed_aliases: usize,
}

/// One use through another definition, in the JSON document.
#[derive(Serialize)]
struct ThroughDocument<'a> {
    name: &'a str,
    uses: u64,
}

/// One definition in the JSON document.
#[derive(Serialize)]
struct DefinitionDocument<'a> {
    name: &'a str,
    kind: Kind,
    definition: &'a str,
    uses: u64,
    typed: u64,
    through: Vec<ThroughDocument<'a>>,
    first_use: Option<NaiveDate>,
    last_use: Option<NaiveDate>,
    classification: Classification,
}

/// The name and the kind of one definition, in a list of the JSON document.
#[derive(Serialize)]
struct NamedDocument<'a> {
    name: &'a str,
    kind: Kind,
}

/// The whole JSON document.
#[derive(Serialize)]
struct Document<'a> {
    today: NaiveDate,
    since: Option<NaiveDate>,
    idle_days: u32,
    history: HistoryDocument,
    shell: ShellDocument<'a>,
    definitions: Vec<DefinitionDocument<'a>>,
    never_used: Vec<NamedDocument<'a>>,
    idle: Vec<NamedDocument<'a>>,
}

/// Gives the JSON form of one definition.
fn definition_document(usage: &Usage) -> DefinitionDocument<'_> {
    DefinitionDocument {
        name: &usage.name,
        kind: usage.kind,
        definition: &usage.definition,
        uses: usage.uses,
        typed: usage.typed,
        through: usage
            .through
            .iter()
            .map(|(name, uses)| ThroughDocument { name, uses: *uses })
            .collect(),
        first_use: usage.first_use,
        last_use: usage.last_use,
        classification: usage.classification,
    }
}

/// Gives the name and the kind of each definition in `definitions`.
fn named<'a>(definitions: &[&'a Usage]) -> Vec<NamedDocument<'a>> {
    definitions
        .iter()
        .map(|usage| NamedDocument {
            name: &usage.name,
            kind: usage.kind,
        })
        .collect()
}

/// Gives the JSON document, pretty-printed, with a final newline.
///
/// `definitions` holds every entry in tally order, also an entry with no use.
/// The definition text is never cut. The lists `never_used` and `idle` follow
/// the order of the text report.
///
/// # Panics
///
/// Never in practice. The document holds only numbers, dates, and text, and
/// `serde_json` serializes each of them without an error.
#[must_use]
pub fn json(tally: &Tally, window: &Window, shell: &str) -> String {
    let summary = &tally.summary;
    let document = Document {
        today: window.today,
        since: window.since,
        idle_days: window.idle_days,
        history: HistoryDocument {
            rows: summary.rows,
            before_since: summary.before_since,
            not_found: summary.not_found,
            unparsed_rows: summary.unparsed_rows,
        },
        shell: ShellDocument {
            path: shell,
            aliases: summary.aliases,
            functions: summary.functions,
            unparsed_functions: summary.unparsed_functions,
            unparsed_aliases: summary.unparsed_aliases,
        },
        definitions: tally.definitions.iter().map(definition_document).collect(),
        never_used: named(&with(tally, Classification::NeverUsed)),
        idle: named(&idle_definitions(tally)),
    };
    let mut text = serde_json::to_string_pretty(&document)
        .expect("a document of numbers, dates, and text always serializes");
    text.push('\n');
    text
}
