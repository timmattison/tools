//! The report of the definitions: the text that a person reads and the JSON
//! that a program reads.
//!
//! The text report shows only my definitions, unless the window has `all`.
//! Then it shows mine first, and the others in one table for each file of
//! origin. The JSON document always holds each entry of the tally, with its
//! origin and a field that says if it is mine.
//!
//! The text report ends with the group `Duplicates and shadows`. It lists each
//! finding of the tally that holds a definition of mine or a place of mine, or
//! each finding with `all`. The JSON document holds each finding.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::NaiveDate;
use comfy_table::{presets, CellAlignment, ColumnConstraint, ContentArrangement, Table, Width};
use serde::Serialize;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{Classification, Finding, FindingKind, Kind, Member, Summary, Tally, Usage, Window};
use crate::origin::{Origin, Places};
use crate::render::{
    counted, date_cell, group, number, number_of, BETWEEN_BLOCKS, THROUGH_SEPARATOR,
};

/// The name of each column of the table, in order.
const COLUMNS: [&str; 9] = [
    "Name",
    "Kind",
    "Uses",
    "Typed",
    "Through",
    "First use",
    "Last use",
    "Origin",
    "Definition",
];

/// The place of the `Through` column.
const THROUGH_COLUMN: usize = 4;

/// The place of the `Definition` column.
const DEFINITION_COLUMN: usize = 8;

/// The number of the columns other than the definition.
const FIXED_COLUMNS: usize = DEFINITION_COLUMN;

/// The text of the `Origin` cell when the origin is unknown, and the heading
/// of the group of the others whose origin is unknown.
const UNKNOWN_ORIGIN: &str = "-";

/// The heading of the table of my definitions in the report of `--all`.
const HEADING_MINE: &str = "Mine";

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

/// The heading of the group of definitions that the shell runs without a
/// typed name.
const HEADING_RUNS: &str = "Runs without typing";

/// The text that divides two reasons of one definition in a group.
const REASON_SEPARATOR: &str = "; ";

/// What starts the shell line of the header.
const SHELL_LABEL: &str = "Shell";

/// What starts the line of the header that counts mine and the others.
const DEFINITIONS_LABEL: &str = "Definitions";

/// The note after the counts of mine and the others, when the text report
/// does not show the others.
const ALL_HINT: &str = " (--all shows the others)";

/// The heading of the group of the duplicates and the shadows.
const HEADING_FINDINGS: &str = "Duplicates and shadows";

/// What starts the entry of a finding of the kind
/// [`FindingKind::SameExpansion`].
const LABEL_SAME_EXPANSION: &str = "Same expansion";

/// What starts the entry of a finding of the kind
/// [`FindingKind::AliasAndFunction`].
const LABEL_ALIAS_AND_FUNCTION: &str = "Alias and function";

/// What starts the entry of a finding of the kind
/// [`FindingKind::AliasHidesFunction`].
const LABEL_ALIAS_HIDES_FUNCTION: &str = "Alias hides function";

/// What starts the entry of a finding of the kind
/// [`FindingKind::HidesProgram`].
const LABEL_HIDES_PROGRAM: &str = "Hides a program";

/// The text that divides the members of a finding, and the places of a
/// finding.
const MEMBER_SEPARATOR: &str = ", ";

/// The text between the members of a finding of the kind
/// [`FindingKind::DefinedMoreThanOnce`] and its places.
const AT: &str = " at ";

/// The note after the last place of a finding of the kind
/// [`FindingKind::DefinedMoreThanOnce`].
const IN_EFFECT: &str = " (in effect)";

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

/// Gives the text of `origin` with `~` for the home directory, or
/// [`UNKNOWN_ORIGIN`].
fn shown_origin(origin: Option<&Origin>, places: &Places) -> String {
    origin.map_or_else(|| UNKNOWN_ORIGIN.to_string(), |origin| places.show(origin))
}

/// Gives the `Origin` cell: the origin with `~` for the home directory, or
/// [`UNKNOWN_ORIGIN`].
fn origin_cell(usage: &Usage, places: &Places) -> String {
    shown_origin(usage.origin.as_ref(), places)
}

/// Gives the cells of `usage` for the columns other than the definition, in
/// column order.
fn fixed_cells(usage: &Usage, places: &Places) -> [String; FIXED_COLUMNS] {
    [
        usage.name.clone(),
        usage.kind.label().to_string(),
        number(usage.uses),
        number(usage.typed),
        through_cell(usage),
        date_cell(usage.first_use),
        date_cell(usage.last_use),
        origin_cell(usage, places),
    ]
}

/// Gives the display width that the table needs for the text of the columns
/// other than the definition. The `Through` column counts at most
/// [`THROUGH_MAX`].
fn fixed_text_width(rows: &[[String; FIXED_COLUMNS]]) -> usize {
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

/// Gives the table of `usages`, one row for each entry in the order of
/// `usages`. `places` gives the text of each origin.
///
/// `width` limits the table to that many display columns. The definition is
/// one line, cut to the room that the other columns leave. The `Through` cell
/// wraps inside [`THROUGH_MAX`] columns. `None` sets no limit, no cut, and no
/// wrap. The table never reads the terminal itself, because a read of the
/// terminal makes the text depend on where the tool runs.
fn table(usages: &[&Usage], places: &Places, width: Option<u16>) -> String {
    let rows: Vec<[String; FIXED_COLUMNS]> = usages
        .iter()
        .map(|usage| fixed_cells(usage, places))
        .collect();
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
    for (usage, fixed) in usages.iter().zip(rows) {
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

/// Gives the entries of `usages` that have `classification`, sorted by name
/// and kind.
fn with<'a>(usages: &[&'a Usage], classification: Classification) -> Vec<&'a Usage> {
    let mut found: Vec<&Usage> = usages
        .iter()
        .copied()
        .filter(|usage| usage.classification == classification)
        .collect();
    found.sort_by(|left, right| (&left.name, left.kind).cmp(&(&right.name, right.kind)));
    found
}

/// Gives the idle entries of `usages`, the entry with the most uses first.
/// Entries with equal uses stay in name order.
fn idle_definitions<'a>(usages: &[&'a Usage]) -> Vec<&'a Usage> {
    let mut idle = with(usages, Classification::Idle);
    idle.sort_by_key(|usage| std::cmp::Reverse(usage.uses));
    idle
}

/// Gives the text after the facts of `usage` in a group: a comma and its
/// reasons, divided by [`REASON_SEPARATOR`], or nothing when it has none.
fn reasons_suffix(usage: &Usage) -> String {
    if usage.reasons.is_empty() {
        return String::new();
    }
    let reasons: Vec<String> = usage.reasons.iter().map(ToString::to_string).collect();
    format!(", {}", reasons.join(REASON_SEPARATOR))
}

/// Gives the three groups of `usages`: the definitions never used, the
/// definitions idle, and the definitions that the shell runs without a typed
/// name. Each entry ends with its reasons.
fn group_lines(usages: &[&Usage], window: &Window) -> String {
    let never: Vec<String> = with(usages, Classification::NeverUsed)
        .iter()
        .map(|usage| {
            format!(
                "{} ({}{})",
                usage.name,
                usage.kind.label(),
                reasons_suffix(usage)
            )
        })
        .collect();
    let idle: Vec<String> = idle_definitions(usages)
        .iter()
        .map(|usage| {
            format!(
                "{} ({}, {}, last use {}{})",
                usage.name,
                usage.kind.label(),
                counted(usage.uses, "use", "uses"),
                date_cell(usage.last_use),
                reasons_suffix(usage),
            )
        })
        .collect();
    let runs: Vec<String> = with(usages, Classification::RunsWithoutTyping)
        .iter()
        .map(|usage| {
            format!(
                "{} ({}{})",
                usage.name,
                usage.kind.label(),
                reasons_suffix(usage)
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
        group(HEADING_RUNS, &runs),
    ]
    .join("\n")
}

/// Gives the three lines that describe the history, the shell, and the counts
/// of mine and the others.
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
    let mut definitions_line = format!(
        "{DEFINITIONS_LABEL}: {} mine, {} others",
        number_of(summary.mine),
        number_of(summary.others),
    );
    if !window.all && summary.others > 0 {
        definitions_line.push_str(ALL_HINT);
    }
    definitions_line.push('.');
    format!("{history}\n{shell_line}\n{definitions_line}")
}

/// Gives a heading with the count of `usages`, and their table under it.
fn headed_table(heading: &str, usages: &[&Usage], places: &Places, width: Option<u16>) -> String {
    format!(
        "{heading}: {}\n{}",
        counted(usages.len(), "definition", "definitions"),
        table(usages, places, width),
    )
}

/// Gives the entries that are not mine, grouped by the file of their origin,
/// in path order. Each group keeps the tally order. The tally makes each
/// entry with an unknown origin mine, thus the group of `None` is empty in a
/// report of [`super::tally`].
fn others_by_file(tally: &Tally) -> BTreeMap<Option<&Path>, Vec<&Usage>> {
    let mut groups: BTreeMap<Option<&Path>, Vec<&Usage>> = BTreeMap::new();
    for usage in tally.definitions.iter().filter(|usage| !usage.mine) {
        let file = usage.origin.as_ref().map(|origin| origin.file.as_path());
        groups.entry(file).or_default().push(usage);
    }
    groups
}

/// Gives the heading of the group of the others from `file`: the file with
/// `~` for the home directory, or [`UNKNOWN_ORIGIN`].
fn file_heading(file: Option<&Path>, places: &Places) -> String {
    file.map_or_else(
        || UNKNOWN_ORIGIN.to_string(),
        |file| {
            places.show(&Origin {
                file: file.to_path_buf(),
                line: None,
            })
        },
    )
}

/// Gives the text of `member`: its name, then its kind, its origin, and its
/// uses in parentheses. With `with_origin` false, the origin is left out.
fn member_text(member: &Member, places: &Places, with_origin: bool) -> String {
    let uses = counted(member.uses, "use", "uses");
    if with_origin {
        let origin = shown_origin(member.origin.as_ref(), places);
        format!(
            "{} ({}, {origin}, {uses})",
            member.name,
            member.kind.label()
        )
    } else {
        format!("{} ({}, {uses})", member.name, member.kind.label())
    }
}

/// Gives the members of `finding`, divided by [`MEMBER_SEPARATOR`].
fn members_text(finding: &Finding, places: &Places, with_origin: bool) -> String {
    finding
        .definitions
        .iter()
        .map(|member| member_text(member, places, with_origin))
        .collect::<Vec<_>>()
        .join(MEMBER_SEPARATOR)
}

/// Joins `words` with one space. Each control character shows as the escape
/// of [`char::escape_default`] (`\n`, `\t`, `\u{1b}`), so that the entry stays
/// on one line. A backslash stays as it is, because the words are shell text.
fn words_text(words: &[String]) -> String {
    let mut text = String::new();
    for character in words.join(" ").chars() {
        if character.is_control() {
            text.extend(character.escape_default());
        } else {
            text.push(character);
        }
    }
    text
}

/// Gives the entry of `finding` in the group of the duplicates and the
/// shadows. The text of each kind has its own shape.
fn finding_entry(finding: &Finding, places: &Places) -> String {
    match finding.kind {
        FindingKind::SameExpansion => format!(
            "{LABEL_SAME_EXPANSION} ({}): {}",
            words_text(&finding.words),
            members_text(finding, places, true),
        ),
        FindingKind::AliasAndFunction => format!(
            "{LABEL_ALIAS_AND_FUNCTION} ({}): {}",
            words_text(&finding.words),
            members_text(finding, places, true),
        ),
        FindingKind::DefinedMoreThanOnce => {
            let spots = finding
                .places
                .iter()
                .map(|origin| shown_origin(origin.as_ref(), places))
                .collect::<Vec<_>>()
                .join(MEMBER_SEPARATOR);
            format!(
                "Defined {}: {}{AT}{spots}{IN_EFFECT}",
                counted(finding.places.len(), "time", "times"),
                members_text(finding, places, false),
            )
        }
        FindingKind::AliasHidesFunction => format!(
            "{LABEL_ALIAS_HIDES_FUNCTION}: {}",
            members_text(finding, places, true),
        ),
        FindingKind::HidesProgram => {
            let program = finding.program.as_ref().map(|file| Origin {
                file: file.clone(),
                line: None,
            });
            format!(
                "{LABEL_HIDES_PROGRAM} ({}): {}",
                shown_origin(program.as_ref(), places),
                members_text(finding, places, true),
            )
        }
    }
}

/// Gives the group of the duplicates and the shadows: each finding of the
/// tally with a definition of mine or a place of mine, or each finding when
/// `all` is true.
fn findings_group(tally: &Tally, all: bool) -> String {
    let entries: Vec<String> = tally
        .findings
        .iter()
        .filter(|finding| all || finding.mine(&tally.places))
        .map(|finding| finding_entry(finding, &tally.places))
        .collect();
    group(HEADING_FINDINGS, &entries)
}

/// Gives the text report. `shell` is the path of the shell that gave the
/// definitions. `width` is the table width. `None` sets no limit.
///
/// The report starts with the header. Then come the three groups, and last the
/// group `Duplicates and shadows`. Between the header and the groups, the
/// report has the table of my definitions. With `window.all`, that table has
/// the heading `Mine` and a count, and one more table follows for each
/// file that defined the others, with that file and a count as its heading.
/// The three groups then hold each entry, else only mine. The last group
/// holds each finding with `window.all`, else only the findings that hold one
/// of my definitions or one of my places. The definition in a table is one line. It is cut to the
/// room that the width leaves.
#[must_use]
pub fn text(tally: &Tally, window: &Window, shell: &str, width: Option<u16>) -> String {
    let places = &tally.places;
    let mine: Vec<&Usage> = tally
        .definitions
        .iter()
        .filter(|usage| usage.mine)
        .collect();
    let mut blocks = vec![header_lines(&tally.summary, window, shell)];
    let shown: Vec<&Usage> = if window.all {
        blocks.push(headed_table(HEADING_MINE, &mine, places, width));
        for (file, usages) in others_by_file(tally) {
            let heading = file_heading(file, places);
            blocks.push(headed_table(&heading, &usages, places, width));
        }
        tally.definitions.iter().collect()
    } else {
        blocks.push(table(&mine, places, width));
        mine
    };
    blocks.push(group_lines(&shown, window));
    blocks.push(findings_group(tally, window.all));
    blocks.join(BETWEEN_BLOCKS) + "\n"
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
    mine: usize,
    others: usize,
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
    origin: Option<&'a Origin>,
    mine: bool,
    reasons: Vec<String>,
}

/// The name and the kind of one definition, in a list of the JSON document.
#[derive(Serialize)]
struct NamedDocument<'a> {
    name: &'a str,
    kind: Kind,
}

/// One definition of a finding, in the JSON document.
#[derive(Serialize)]
struct MemberDocument<'a> {
    name: &'a str,
    kind: Kind,
    origin: Option<&'a Origin>,
    uses: u64,
    mine: bool,
}

/// One duplicate or one shadow, in the JSON document.
#[derive(Serialize)]
struct FindingDocument<'a> {
    kind: FindingKind,
    mine: bool,
    definitions: Vec<MemberDocument<'a>>,
    words: &'a [String],
    places: Vec<Option<&'a Origin>>,
    program: Option<&'a Path>,
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
    runs_without_typing: Vec<NamedDocument<'a>>,
    findings: Vec<FindingDocument<'a>>,
}

/// Gives the JSON form of one finding. `directories` decides which places are
/// mine.
fn finding_document<'a>(finding: &'a Finding, directories: &Places) -> FindingDocument<'a> {
    FindingDocument {
        kind: finding.kind,
        mine: finding.mine(directories),
        definitions: finding
            .definitions
            .iter()
            .map(|member| MemberDocument {
                name: &member.name,
                kind: member.kind,
                origin: member.origin.as_ref(),
                uses: member.uses,
                mine: member.mine,
            })
            .collect(),
        words: &finding.words,
        places: finding.places.iter().map(Option::as_ref).collect(),
        program: finding.program.as_deref(),
    }
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
        origin: usage.origin.as_ref(),
        mine: usage.mine,
        reasons: usage.reasons.iter().map(ToString::to_string).collect(),
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
/// `definitions` holds every entry in tally order, also an entry with no use
/// and an entry that is not mine. Each entry has its `origin` (`file` and
/// `line`, or `null` when it is unknown) and `mine`. The definition text is
/// never cut. Each entry has its `reasons`, as the text report writes them, in
/// a list that is empty for most entries. The lists `never_used`, `idle`, and
/// `runs_without_typing` hold every entry too, in the order of the text
/// report.
///
/// `findings` holds each finding of the tally in order, also a finding with no
/// definition of mine. A finding is `mine` when one of its definitions or one
/// of its places is mine. Each finding has the fields `kind`, `mine`,
/// `definitions`, `words`, `places`, and `program`, always present. Each
/// definition has `name`, `kind`, `origin`, `uses`, and `mine`. `words` and
/// `places` are empty lists, and `program` is `null`, when the kind of the
/// finding does not use them. A place is an origin or `null`.
///
/// # Panics
///
/// Never in practice. The document holds only numbers, dates, and text, and
/// `serde_json` serializes each of them without an error. The file of an
/// origin came from text, thus it is valid UTF-8.
#[must_use]
pub fn json(tally: &Tally, window: &Window, shell: &str) -> String {
    let summary = &tally.summary;
    let every: Vec<&Usage> = tally.definitions.iter().collect();
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
            mine: summary.mine,
            others: summary.others,
        },
        definitions: tally.definitions.iter().map(definition_document).collect(),
        never_used: named(&with(&every, Classification::NeverUsed)),
        idle: named(&idle_definitions(&every)),
        runs_without_typing: named(&with(&every, Classification::RunsWithoutTyping)),
        findings: tally
            .findings
            .iter()
            .map(|finding| finding_document(finding, &tally.places))
            .collect(),
    };
    let mut text = serde_json::to_string_pretty(&document)
        .expect("a document of numbers, dates, and text always serializes");
    text.push('\n');
    text
}
