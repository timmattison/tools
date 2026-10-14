//! Tests of the report: the text, the JSON, and the list of unparsed commands.

use std::path::PathBuf;

use chrono::NaiveDate;
use wdid::catalog::{PathState, Tool};
use wdid::render::{table_width, text};
use wdid::tally::{Classification, Summary, Tally, ToolUsage, Unparsed, Window};
use wdid::tool::ToolName;

/// Makes a date.
fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// Makes the window of every test: the date of the run is 2026-10-14.
fn window() -> Window {
    Window {
        today: date(2026, 10, 14),
        since: None,
        new_days: 90,
        idle_days: 180,
    }
}

/// The facts of one tool that a test states. The rest comes from the name.
struct Row<'a> {
    name: &'a str,
    typed: u64,
    through: &'a [(&'a str, u64)],
    development_runs: u64,
    first_use: Option<NaiveDate>,
    last_use: Option<NaiveDate>,
    added: Option<NaiveDate>,
    path: Option<PathState>,
    classification: Classification,
}

/// Makes the usage of one tool from the facts in `row`.
fn usage(row: Row<'_>) -> ToolUsage {
    let through: Vec<(String, u64)> = row
        .through
        .iter()
        .map(|(name, count)| ((*name).to_string(), *count))
        .collect();
    let through_total: u64 = through.iter().map(|(_, count)| count).sum();
    ToolUsage {
        tool: Tool {
            name: ToolName::new(row.name),
            crate_dir: PathBuf::from("src").join(row.name),
            added: row.added,
            installed: row.path.is_some(),
            path: row.path,
        },
        uses: row.typed + through_total,
        typed: row.typed,
        through,
        development_runs: row.development_runs,
        first_use: row.first_use,
        last_use: row.last_use,
        classification: row.classification,
    }
}

/// Makes the summary of the fixture: no `since` date, one function that did
/// not parse, and two commands that did not parse.
fn summary() -> Summary {
    Summary {
        rows: 1_234,
        before_since: 0,
        not_found: 12,
        unparsed_rows: 3,
        unparsed: vec![
            Unparsed {
                command: "echo $(".to_string(),
                rows: 2,
            },
            Unparsed {
                command: "a |".to_string(),
                rows: 1,
            },
        ],
        aliases: 5,
        functions: 4,
        unparsed_functions: vec!["broken".to_string()],
        unparsed_aliases: 0,
    }
}

/// Makes the tally of the fixture, in tally order (most uses first, then name):
/// `cwt` is used through two names and has a development run, `aa` is idle and
/// another program runs first, `faulte` is new and not on the PATH, and `goup`
/// is old and not installed.
fn fixture() -> Tally {
    Tally {
        tools: vec![
            usage(Row {
                name: "cwt",
                typed: 31,
                through: &[("wtm", 3_061), ("wt", 3_029)],
                development_runs: 2,
                first_use: Some(date(2025, 2, 27)),
                last_use: Some(date(2026, 10, 13)),
                added: Some(date(2025, 6, 1)),
                path: Some(PathState::Cargo),
                classification: Classification::Used,
            }),
            usage(Row {
                name: "aa",
                typed: 218,
                through: &[],
                development_runs: 0,
                first_use: Some(date(2024, 1, 5)),
                last_use: Some(date(2025, 11, 13)),
                added: Some(date(2024, 1, 1)),
                path: Some(PathState::Shadowed(PathBuf::from("/usr/bin/aa"))),
                classification: Classification::Idle,
            }),
            usage(Row {
                name: "faulte",
                typed: 0,
                through: &[],
                development_runs: 0,
                first_use: None,
                last_use: None,
                added: Some(date(2026, 9, 20)),
                path: Some(PathState::NotOnPath),
                classification: Classification::NeverUsedNew,
            }),
            usage(Row {
                name: "goup",
                typed: 0,
                through: &[],
                development_runs: 0,
                first_use: None,
                last_use: None,
                added: Some(date(2024, 3, 2)),
                path: None,
                classification: Classification::NeverUsedOld,
            }),
        ],
        summary: summary(),
    }
}

/// The table of the fixture at a width of 200 columns.
const TABLE_200: &str = r#"┌────────┬───────┬───────┬────────────────────────────────┬──────────┬────────────┬────────────┬────────────┬────────────────────────┐
│ Tool   ┆  Uses ┆ Typed ┆ Through an alias or a function ┆ Dev runs ┆ First use  ┆ Last use   ┆ Added      ┆ Notes                  │
╞════════╪═══════╪═══════╪════════════════════════════════╪══════════╪════════════╪════════════╪════════════╪════════════════════════╡
│ cwt    ┆ 6,121 ┆    31 ┆ wtm 3,061, wt 3,029            ┆        2 ┆ 2025-02-27 ┆ 2026-10-13 ┆ 2025-06-01 ┆                        │
│ aa     ┆   218 ┆   218 ┆                                ┆        0 ┆ 2024-01-05 ┆ 2025-11-13 ┆ 2024-01-01 ┆ /usr/bin/aa runs first │
│ faulte ┆     0 ┆     0 ┆                                ┆        0 ┆ -          ┆ -          ┆ 2026-09-20 ┆ not on PATH            │
│ goup   ┆     0 ┆     0 ┆                                ┆        0 ┆ -          ┆ -          ┆ 2024-03-02 ┆ not installed          │
└────────┴───────┴───────┴────────────────────────────────┴──────────┴────────────┴────────────┴────────────┴────────────────────────┘"#;

/// The text before the first blank line: the table.
fn table_of(report: &str) -> &str {
    report.split_once("\n\n").map_or(report, |(table, _)| table)
}

/// The table has the nine columns, one row for each tool in tally order, a
/// number with thousands separators, a right-aligned number, `-` for an absent
/// date, and a note for each state of the PATH.
#[test]
fn the_table_shows_one_row_for_each_tool() {
    let report = text(&fixture(), &window(), Some(200));

    assert_eq!(table_of(&report), TABLE_200);
}

/// A narrow width wraps the cells, and no line is longer than the width.
#[test]
fn a_narrow_table_wraps_inside_its_width() {
    let report = text(&fixture(), &window(), Some(60));
    let table = table_of(&report);

    assert!(table.lines().count() > 6, "the cells wrap:\n{table}");
    for line in table.lines() {
        assert!(
            line.chars().count() <= 60,
            "the line is longer than 60 columns: {line:?}"
        );
    }
}

/// `COLUMNS` wins, then the terminal, else there is no width. A value that is
/// not a positive integer is ignored.
#[test]
fn the_table_width_takes_the_stated_width_then_the_terminal() {
    let cases: [(Option<&str>, Option<u16>, Option<u16>); 10] = [
        (Some("150"), Some(80), Some(150)),
        (Some("150"), None, Some(150)),
        (Some(" 150\n"), Some(80), Some(150)),
        (None, Some(80), Some(80)),
        (None, None, None),
        (Some("0"), Some(80), Some(80)),
        (Some("wide"), Some(80), Some(80)),
        (Some("-5"), None, None),
        (Some("70000"), Some(80), Some(80)),
        (None, Some(0), None),
    ];
    for (stated, terminal, expected) in cases {
        assert_eq!(
            table_width(stated, terminal),
            expected,
            "stated {stated:?}, terminal {terminal:?}"
        );
    }
}
