//! Tests of the report: the text, the JSON, and the list of unparsed commands.

use std::path::PathBuf;

use chrono::NaiveDate;
use wdid::catalog::{PathState, Tool};
use wdid::render::{json, table_width, text, unparsed};
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

/// The values that a cell must show whole: the dates, the counts, and the tool
/// names of the fixture. A date that wraps inside its cell cannot be read.
const WHOLE_VALUES: [&str; 13] = [
    "2025-02-27",
    "2026-10-13",
    "2025-06-01",
    "2024-01-05",
    "2025-11-13",
    "2024-01-01",
    "2026-09-20",
    "2024-03-02",
    "6,121",
    "218",
    "faulte",
    "goup",
    "Dev runs",
];

/// Makes sure that each value of [`WHOLE_VALUES`] is whole on one line.
fn assert_whole_values(table: &str) {
    for value in WHOLE_VALUES {
        assert!(
            table.lines().any(|line| line.contains(value)),
            "{value:?} is not whole on one line:\n{table}"
        );
    }
}

/// A narrow width wraps only the two text columns. Each name, count, and date
/// stays whole, and no line is longer than the width.
#[test]
fn a_narrow_table_wraps_only_the_text_columns() {
    let report = text(&fixture(), &window(), Some(120));
    let table = table_of(&report);

    assert!(table.lines().count() > 8, "the text cells wrap:\n{table}");
    assert_whole_values(table);
    for line in table.lines() {
        assert!(
            line.chars().count() <= 120,
            "the line is longer than 120 columns: {line:?}"
        );
    }
}

/// A width that is too small for the names, the counts, and the dates gives a
/// table that is wider than the width. Each value stays whole.
#[test]
fn a_table_too_narrow_for_its_dates_keeps_each_date_whole() {
    let report = text(&fixture(), &window(), Some(60));
    assert_whole_values(table_of(&report));
}

/// A text column keeps a width that a person can read, also when the width of
/// the table is too small. A real run gave each text column one character, and
/// the header `Through an alias or a function` filled 30 lines.
#[test]
fn a_text_column_keeps_a_width_that_a_person_can_read() {
    let report = text(&fixture(), &window(), Some(60));
    let table = table_of(&report);
    for value in ["wtm 3,061, wt 3,029", "not installed", "Through an alias"] {
        assert!(
            table.lines().any(|line| line.contains(value)),
            "{value:?} is not whole on one line:\n{table}"
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

/// The lines after the table in the report of the fixture at any width.
const TAIL: &str = r#"Never used, old (added 90 or more days ago):
  goup
Never used, new (added less than 90 days ago):
  faulte (added 2026-09-20)
Idle (no use in the last 180 days):
  aa (218 uses, last use 2025-11-13)

History: 1,234 rows. 12 not counted (exit 127). 3 did not parse (wdid --unparsed prints them).
Shell: 5 aliases, 4 functions (1 did not parse).
"#;

/// The whole report: the table, a blank line, the three group lines, a blank
/// line, the two summary lines, and a final newline.
#[test]
fn the_report_holds_the_table_the_groups_and_the_summary() {
    let report = text(&fixture(), &window(), Some(200));

    assert_eq!(report, format!("{TABLE_200}\n\n{TAIL}"));
}

/// Makes a tool that never ran.
fn never_used(name: &str, added: Option<NaiveDate>, classification: Classification) -> ToolUsage {
    usage(Row {
        name,
        typed: 0,
        through: &[],
        development_runs: 0,
        first_use: None,
        last_use: None,
        added,
        path: Some(PathState::Cargo),
        classification,
    })
}

/// Makes a tool that ran `typed` times and then fell idle.
fn idle(name: &str, typed: u64, last_use: NaiveDate) -> ToolUsage {
    usage(Row {
        name,
        typed,
        through: &[],
        development_runs: 0,
        first_use: Some(date(2024, 1, 1)),
        last_use: Some(last_use),
        added: Some(date(2024, 1, 1)),
        path: Some(PathState::Cargo),
        classification: Classification::Idle,
    })
}

/// Gives the three group lines of `tally`.
fn group_lines(tally: &Tally) -> Vec<String> {
    let report = text(tally, &window(), None);
    let groups = report.split("\n\n").nth(1).unwrap_or_default();
    groups.lines().map(str::to_string).collect()
}

/// The old names are sorted by name. The new entries are sorted by name, and
/// each one has its added date. The idle entries are sorted by uses (most
/// first), then by name, and a count of one says `1 use`.
#[test]
fn each_group_is_sorted_and_names_its_facts() {
    let tally = Tally {
        tools: vec![
            idle("zip", 1_500, date(2025, 3, 4)),
            idle("alpha", 2, date(2025, 1, 2)),
            idle("beta", 2, date(2025, 2, 3)),
            idle("single", 1, date(2025, 4, 5)),
            never_used("pear", None, Classification::NeverUsedOld),
            never_used(
                "apple",
                Some(date(2024, 5, 6)),
                Classification::NeverUsedOld,
            ),
            never_used("yak", Some(date(2026, 9, 1)), Classification::NeverUsedNew),
            never_used("bat", Some(date(2026, 10, 1)), Classification::NeverUsedNew),
        ],
        summary: summary(),
    };

    assert_eq!(
        group_lines(&tally),
        [
            "Never used, old (added 90 or more days ago):",
            "  apple",
            "  pear",
            "Never used, new (added less than 90 days ago):",
            "  bat (added 2026-10-01)",
            "  yak (added 2026-09-01)",
            "Idle (no use in the last 180 days):",
            "  zip (1,500 uses, last use 2025-03-04)",
            "  alpha (2 uses, last use 2025-01-02)",
            "  beta (2 uses, last use 2025-02-03)",
            "  single (1 use, last use 2025-04-05)",
        ]
    );
}

/// The limits in the group lines come from the window.
#[test]
fn the_group_lines_name_the_limits_of_the_window() {
    let window = Window {
        new_days: 30,
        idle_days: 365,
        ..window()
    };

    let report = text(&fixture(), &window, None);

    assert!(report.contains("Never used, old (added 30 or more days ago):\n  goup\n"));
    assert!(report.contains("Never used, new (added less than 30 days ago):\n  faulte"));
    assert!(report.contains("Idle (no use in the last 365 days):\n  aa"));
}

/// An empty group says `none`.
#[test]
fn an_empty_group_says_none() {
    let tally = Tally {
        tools: vec![ToolUsage {
            classification: Classification::Used,
            ..idle("only", 5, date(2026, 10, 1))
        }],
        summary: summary(),
    };

    assert_eq!(
        group_lines(&tally),
        [
            "Never used, old (added 90 or more days ago): none",
            "Never used, new (added less than 90 days ago): none",
            "Idle (no use in the last 180 days): none",
        ]
    );
}

/// Gives the summary lines of the report of `summary`.
fn summary_lines(summary: Summary) -> Vec<String> {
    let tally = Tally {
        tools: vec![never_used("a", None, Classification::NeverUsedOld)],
        summary,
    };
    let report = text(&tally, &window(), None);
    let lines = report.split("\n\n").nth(2).unwrap_or_default();
    lines.lines().map(str::to_string).collect()
}

/// With a `since` date, the history line says how many rows came before it.
#[test]
fn the_history_line_names_the_rows_before_the_since_date() {
    let window = Window {
        since: Some(date(2026, 1, 1)),
        ..window()
    };
    let tally = Tally {
        summary: Summary {
            before_since: 1_040,
            ..summary()
        },
        ..fixture()
    };

    let report = text(&tally, &window, None);

    assert!(report.contains(
        "History: 1,234 rows, 1,040 before 2026-01-01. 12 not counted (exit 127). 3 did not parse (wdid --unparsed prints them).\n"
    ));
}

/// The shell line counts the aliases and the functions. It counts the alias
/// lines that did not parse when there are some. It says so when the shell
/// gave nothing.
#[test]
fn the_shell_line_counts_the_definitions() {
    let with_aliases = summary_lines(Summary {
        unparsed_aliases: 2,
        ..summary()
    });
    let one_alias_line = summary_lines(Summary {
        unparsed_aliases: 1,
        ..summary()
    });
    let nothing = summary_lines(Summary {
        aliases: 0,
        functions: 0,
        unparsed_functions: vec![],
        ..summary()
    });

    assert_eq!(
        with_aliases[1],
        "Shell: 5 aliases, 4 functions (1 did not parse). 2 alias lines did not parse."
    );
    assert_eq!(
        one_alias_line[1],
        "Shell: 5 aliases, 4 functions (1 did not parse). 1 alias line did not parse."
    );
    assert_eq!(
        nothing[1],
        "Shell: no aliases and no functions (--no-shell, or none defined)."
    );
}

/// Parses the JSON of `tally`. The test fails when the text is not one pretty
/// JSON document with a final newline.
fn document(tally: &Tally, window: &Window) -> serde_json::Value {
    let raw = json(tally, window);
    assert!(raw.ends_with("}\n"), "no final newline: {raw:?}");
    assert!(raw.contains("\n  \"today\""), "not pretty-printed: {raw:?}");
    serde_json::from_str(&raw).expect("the report is JSON")
}

/// The document has exactly the keys of the design, and the header values come
/// from the window.
#[test]
fn the_json_document_has_the_keys_of_the_design() {
    let value = document(&fixture(), &window());
    let object = value.as_object().unwrap();
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();

    assert_eq!(
        keys,
        [
            "history",
            "idle",
            "idle_days",
            "never_used_new",
            "never_used_old",
            "new_days",
            "shell",
            "since",
            "today",
            "tools"
        ]
    );
    assert_eq!(value["today"], "2026-10-14");
    assert_eq!(value["since"], serde_json::Value::Null);
    assert_eq!(value["new_days"], 90);
    assert_eq!(value["idle_days"], 180);
    assert_eq!(value["never_used_old"], serde_json::json!(["goup"]));
    assert_eq!(value["never_used_new"], serde_json::json!(["faulte"]));
    assert_eq!(value["idle"], serde_json::json!(["aa"]));
}

/// A `since` date is a date text.
#[test]
fn the_json_since_is_a_date_when_the_window_has_one() {
    let window = Window {
        since: Some(date(2026, 1, 1)),
        ..window()
    };

    assert_eq!(document(&fixture(), &window)["since"], "2026-01-01");
}

/// The history and the shell blocks hold the counts of the summary.
#[test]
fn the_json_history_and_shell_blocks_hold_the_summary() {
    let value = document(&fixture(), &window());

    assert_eq!(
        value["history"],
        serde_json::json!({
            "rows": 1234,
            "before_since": 0,
            "not_found": 12,
            "unparsed_rows": 3,
            "unparsed": [
                { "command": "echo $(", "rows": 2 },
                { "command": "a |", "rows": 1 },
            ],
        })
    );
    assert_eq!(
        value["shell"],
        serde_json::json!({
            "aliases": 5,
            "functions": 4,
            "unparsed_functions": ["broken"],
            "unparsed_aliases": 0,
        })
    );
}

/// A tool that was used holds every fact, with the `through` list, the crate
/// directory with `/` separators, and the state of the PATH.
#[test]
fn the_json_tool_holds_every_fact() {
    let value = document(&fixture(), &window());

    assert_eq!(
        value["tools"][0],
        serde_json::json!({
            "name": "cwt",
            "crate_dir": "src/cwt",
            "uses": 6121,
            "typed": 31,
            "through": [
                { "name": "wtm", "uses": 3061 },
                { "name": "wt", "uses": 3029 },
            ],
            "development_runs": 2,
            "first_use": "2025-02-27",
            "last_use": "2026-10-13",
            "added": "2025-06-01",
            "installed": true,
            "path": { "state": "cargo" },
            "classification": "used",
        })
    );
}

/// The `path` of a tool has one form for each state, and `null` when the tool
/// is not installed. An absent date is `null`.
#[test]
fn the_json_path_has_one_form_for_each_state() {
    let value = document(&fixture(), &window());
    let tools = value["tools"].as_array().unwrap();

    assert_eq!(
        tools[1]["path"],
        serde_json::json!({ "state": "shadowed", "by": "/usr/bin/aa" })
    );
    assert_eq!(tools[1]["classification"], "idle");
    assert_eq!(
        tools[2]["path"],
        serde_json::json!({ "state": "not_on_path" })
    );
    assert_eq!(tools[2]["classification"], "never_used_new");
    assert_eq!(tools[3]["path"], serde_json::Value::Null);
    assert_eq!(tools[3]["installed"], false);
    assert_eq!(tools[3]["first_use"], serde_json::Value::Null);
    assert_eq!(tools[3]["last_use"], serde_json::Value::Null);
    assert_eq!(tools[3]["through"], serde_json::json!([]));
    assert_eq!(tools[3]["classification"], "never_used_old");
}

/// The three lists are in the order of the text report: by name, and the idle
/// list by uses (most first), then by name. A tool with no known `added` date
/// still has a name in its list.
#[test]
fn the_json_lists_follow_the_order_of_the_text_report() {
    let tally = Tally {
        tools: vec![
            idle("zip", 1_500, date(2025, 3, 4)),
            idle("alpha", 2, date(2025, 1, 2)),
            idle("beta", 2, date(2025, 2, 3)),
            never_used("pear", None, Classification::NeverUsedOld),
            never_used(
                "apple",
                Some(date(2024, 5, 6)),
                Classification::NeverUsedOld,
            ),
            never_used("yak", Some(date(2026, 9, 1)), Classification::NeverUsedNew),
            never_used("bat", Some(date(2026, 10, 1)), Classification::NeverUsedNew),
        ],
        summary: summary(),
    };

    let value = document(&tally, &window());

    assert_eq!(value["idle"], serde_json::json!(["zip", "alpha", "beta"]));
    assert_eq!(
        value["never_used_old"],
        serde_json::json!(["apple", "pear"])
    );
    assert_eq!(value["never_used_new"], serde_json::json!(["bat", "yak"]));
}

/// Makes a tally that has only the summary lines in the test.
fn tally_with(unparsed: Vec<Unparsed>, unparsed_functions: Vec<&str>) -> Tally {
    Tally {
        tools: vec![],
        summary: Summary {
            unparsed,
            unparsed_functions: unparsed_functions.into_iter().map(str::to_string).collect(),
            ..summary()
        },
    }
}

/// Makes one group of unparsed rows.
fn group(command: &str, rows: u64) -> Unparsed {
    Unparsed {
        command: command.to_string(),
        rows,
    }
}

/// Each command has a line: the rows, a tab, and the escaped command. The
/// escape keeps one command on one line, so a newline, a tab, a CR, and a
/// backslash do not break the line. The rows are plain digits, so that
/// `sort -n` and `awk` read them.
#[test]
fn an_unparsed_command_is_one_escaped_line() {
    let tally = tally_with(
        vec![
            group("echo $(\nls\t-l\r\\", 1_200),
            group("a |", 2),
            group("日本語 🎉 café $(", 1),
        ],
        vec![],
    );

    assert_eq!(
        unparsed(&tally),
        "1200\techo $(\\nls\\t-l\\r\\\\\n2\ta |\n1\t日本語 🎉 café $(\n"
    );
}

/// A function that did not parse comes after the commands, under a blank line.
#[test]
fn the_unparsed_functions_follow_the_commands() {
    let tally = tally_with(vec![group("a |", 2)], vec!["alpha", "beta"]);

    assert_eq!(
        unparsed(&tally),
        "2\ta |\n\nFunctions that did not parse: alpha, beta\n"
    );
}

/// With no command, the functions line stands alone.
#[test]
fn unparsed_functions_alone_have_no_blank_line_before_them() {
    let tally = tally_with(vec![], vec!["alpha"]);

    assert_eq!(unparsed(&tally), "Functions that did not parse: alpha\n");
}

/// A run with nothing that failed says so.
#[test]
fn a_run_with_no_unparsed_command_and_no_unparsed_function_says_so() {
    assert_eq!(
        unparsed(&tally_with(vec![], vec![])),
        "Every command parsed.\n"
    );
}
