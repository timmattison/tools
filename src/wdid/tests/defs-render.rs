//! Tests of the report of `wdid defs`: the text and the JSON.

use chrono::NaiveDate;
use unicode_width::UnicodeWidthStr;
use wdid::defs::render::{json, text};
use wdid::defs::{Classification, Kind, Summary, Tally, Usage, Window};

/// The characters that start a line of the table.
const BOX_CHARS: [char; 5] = ['┌', '│', '├', '╞', '└'];

/// Makes a date.
fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// Makes the window of every test: the date of the run is 2026-11-26.
fn window() -> Window {
    Window {
        today: date(2026, 11, 26),
        since: None,
        idle_days: 180,
    }
}

/// Makes the summary of every test that does not test the summary.
fn summary() -> Summary {
    Summary {
        rows: 1234,
        before_since: 0,
        not_found: 3,
        unparsed_rows: 2,
        aliases: 276,
        functions: 1267,
        unparsed_functions: 24,
        unparsed_aliases: 0,
    }
}

/// Makes a definition that never ran.
fn never(name: &str, kind: Kind, definition: &str) -> Usage {
    Usage {
        name: name.to_string(),
        kind,
        definition: definition.to_string(),
        uses: 0,
        typed: 0,
        through: Vec::new(),
        first_use: None,
        last_use: None,
        classification: Classification::NeverUsed,
    }
}

/// Makes a definition with `uses` uses and the last use on 2025-01-01.
fn idle(name: &str, kind: Kind, uses: u64) -> Usage {
    Usage {
        uses,
        typed: uses,
        first_use: Some(date(2024, 1, 1)),
        last_use: Some(date(2025, 1, 1)),
        classification: Classification::Idle,
        ..never(name, kind, "x")
    }
}

/// Makes the tally of `definitions` with the summary of every test.
fn tally(definitions: Vec<Usage>) -> Tally {
    Tally {
        definitions,
        summary: summary(),
    }
}

/// Makes a tally with one definition that has `definition` as its text.
fn one(definition: &str) -> Tally {
    tally(vec![never("wide", Kind::Alias, definition)])
}

/// Gives the lines of the table in `report`.
fn table_lines(report: &str) -> Vec<&str> {
    report
        .lines()
        .filter(|line| line.starts_with(BOX_CHARS))
        .collect()
}

/// Gives the cells of the first table row whose name cell is `name`.
fn row<'a>(report: &'a str, name: &str) -> Vec<&'a str> {
    let line = table_lines(report)
        .into_iter()
        .find(|line| {
            line.split('┆')
                .next()
                .is_some_and(|c| c.trim_matches(|ch| ch == '│' || ch == ' ') == name)
        })
        .unwrap_or_else(|| panic!("no row for {name} in:\n{report}"));
    line.trim_matches('│')
        .split(['┆', '│'])
        .map(str::trim)
        .collect()
}

/// The header has the counts, the shell line, and the optional `since` text.
#[test]
fn header_lines() {
    let report = text(&tally(Vec::new()), &window(), "/bin/zsh", None);
    assert!(
        report.starts_with(
            "History: 1,234 rows. 3 not counted (exit 127). 2 did not parse.\n\
             Shell: /bin/zsh, 276 aliases, 1,267 functions (24 did not parse).\n\n"
        ),
        "{report}"
    );
    let mut since = window();
    since.since = Some(date(2026, 1, 1));
    let mut counts = tally(Vec::new());
    counts.summary.before_since = 10;
    counts.summary.unparsed_aliases = 2;
    let report = text(&counts, &since, "/bin/zsh", None);
    assert!(
        report.starts_with(
            "History: 1,234 rows, 10 before 2026-01-01. 3 not counted (exit 127). 2 did not parse.\n\
             Shell: /bin/zsh, 276 aliases, 1,267 functions (24 did not parse). 2 alias lines did not parse.\n"
        ),
        "{report}"
    );
}

/// The table has one row for each entry, in tally order, with its cells.
#[test]
fn table_rows() {
    let mut used = never("wt", Kind::Alias, "cwt");
    used.uses = 1234;
    used.typed = 1;
    used.through = vec![("wtf".to_string(), 3), ("wtm".to_string(), 1)];
    used.first_use = Some(date(2026, 1, 2));
    used.last_use = Some(date(2026, 11, 1));
    used.classification = Classification::Used;
    let report = text(
        &tally(vec![
            never("zz", Kind::Function, "echo zz"),
            used,
            never("gx", Kind::GlobalAlias, "| grep"),
        ]),
        &window(),
        "/bin/zsh",
        None,
    );
    assert_eq!(
        row(&report, "wt"),
        [
            "wt",
            "alias",
            "1,234",
            "1",
            "wtf 3, wtm 1",
            "2026-01-02",
            "2026-11-01",
            "cwt"
        ]
    );
    assert_eq!(
        row(&report, "zz"),
        ["zz", "function", "0", "0", "", "-", "-", "echo zz"]
    );
    assert_eq!(row(&report, "gx")[1], "global alias");
    let names: Vec<&str> = table_lines(&report)
        .iter()
        .skip(1)
        .filter_map(|line| line.trim_start_matches('│').split('┆').next())
        .map(str::trim)
        .collect();
    assert_eq!(names, ["Name", "zz", "wt", "gx"]);
}

/// Checks that each table line is at most `width` columns wide.
fn assert_fits(report: &str, width: usize) {
    for line in table_lines(report) {
        assert!(line.width() <= width, "{} > {width}: {line}", line.width());
    }
}

/// A long definition is cut to one line that ends with an ellipsis.
#[test]
fn long_definition_is_cut() {
    let report = text(&one(&"a".repeat(200)), &window(), "/bin/zsh", Some(80));
    assert_fits(&report, 80);
    let cells = row(&report, "wide");
    assert!(cells[7].ends_with('…'), "{report}");
    assert_eq!(
        table_lines(&report).len(),
        5,
        "top, header, rule, one row, bottom: {report}"
    );
}

/// A multibyte definition is cut without a panic and fits.
#[test]
fn multibyte_definition_is_cut() {
    let report = text(
        &one(&"日本語🎉".repeat(60)),
        &window(),
        "/bin/zsh",
        Some(80),
    );
    assert_fits(&report, 80);
    assert!(row(&report, "wide")[7].ends_with('…'), "{report}");
}

/// A short definition is not cut.
#[test]
fn short_definition_is_not_cut() {
    let report = text(&one("echo hi"), &window(), "/bin/zsh", Some(80));
    assert!(!report.contains('…'), "{report}");
    assert_eq!(row(&report, "wide")[7], "echo hi");
}

/// With no width, the whole definition stays.
#[test]
fn no_width_keeps_the_definition() {
    let long = "a".repeat(200);
    let report = text(&one(&long), &window(), "/bin/zsh", None);
    assert!(report.contains(&long), "{report}");
}

/// The two groups list their entries in order, and an empty group says none.
#[test]
fn groups() {
    let mut small = window();
    small.idle_days = 30;
    let report = text(
        &tally(vec![
            never("goup", Kind::Function, "x"),
            never("abc", Kind::Alias, "x"),
            idle("few", Kind::Alias, 5),
            idle("many", Kind::Function, 50),
        ]),
        &small,
        "/bin/zsh",
        None,
    );
    assert!(
        report.contains("Never used:\n  abc (alias)\n  goup (function)"),
        "{report}"
    );
    assert!(
        report.contains(
            "Idle (no use in the last 30 days):\n  many (function, 50 uses, last use 2025-01-01)\n  few (alias, 5 uses, last use 2025-01-01)"
        ),
        "{report}"
    );
    let empty = text(&tally(Vec::new()), &window(), "/bin/zsh", None);
    assert!(empty.contains("Never used: none"), "{empty}");
    assert!(
        empty.contains("Idle (no use in the last 180 days): none"),
        "{empty}"
    );
}

/// The text ends with exactly one newline.
#[test]
fn ends_with_one_newline() {
    let report = text(&tally(Vec::new()), &window(), "/bin/zsh", Some(80));
    assert!(
        report.ends_with('\n') && !report.ends_with("\n\n"),
        "{report:?}"
    );
}

/// The JSON document holds the window, the blocks, and every definition.
#[test]
fn json_document() {
    let mut used = never("wt", Kind::SuffixAlias, &"d".repeat(200));
    used.uses = 3;
    used.typed = 1;
    used.through = vec![("wtf".to_string(), 2)];
    used.first_use = Some(date(2026, 1, 1));
    used.last_use = Some(date(2026, 11, 1));
    used.classification = Classification::Used;
    let tally = tally(vec![
        used,
        never("goup", Kind::Function, "x"),
        never("gx", Kind::GlobalAlias, "x"),
        idle("old", Kind::Alias, 5),
    ]);
    let text = json(&tally, &window(), "/bin/zsh");
    assert!(text.ends_with("}\n"));
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["today"], "2026-11-26");
    assert!(value["since"].is_null());
    assert_eq!(value["idle_days"], 180);
    assert_eq!(value["history"]["rows"], 1234);
    assert_eq!(value["history"]["not_found"], 3);
    assert_eq!(value["history"]["before_since"], 0);
    assert_eq!(value["history"]["unparsed_rows"], 2);
    assert_eq!(value["shell"]["path"], "/bin/zsh");
    assert_eq!(value["shell"]["aliases"], 276);
    assert_eq!(value["shell"]["functions"], 1267);
    assert_eq!(value["shell"]["unparsed_functions"], 24);
    assert_eq!(value["shell"]["unparsed_aliases"], 0);
    let defs = value["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 4);
    assert_eq!(defs[0]["kind"], "suffix_alias");
    assert_eq!(defs[0]["definition"], "d".repeat(200));
    assert_eq!(defs[0]["through"][0]["name"], "wtf");
    assert_eq!(defs[0]["through"][0]["uses"], 2);
    assert_eq!(defs[0]["first_use"], "2026-01-01");
    assert_eq!(defs[0]["classification"], "used");
    assert_eq!(defs[1]["uses"], 0);
    assert_eq!(defs[1]["classification"], "never_used");
    assert_eq!(defs[2]["kind"], "global_alias");
    assert_eq!(
        value["never_used"],
        serde_json::json!([
            {"name": "goup", "kind": "function"},
            {"name": "gx", "kind": "global_alias"}
        ])
    );
    assert_eq!(
        value["idle"],
        serde_json::json!([{"name": "old", "kind": "alias"}])
    );
}
