//! Tests of the report of `wdid defs`: the text and the JSON.

use chrono::NaiveDate;
use std::path::PathBuf;
use unicode_width::UnicodeWidthStr;
use wdid::defs::render::{json, text};
use wdid::defs::{Classification, Kind, Summary, Tally, Usage, Window};

use wdid::origin::{Origin, Places};

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
        all: false,
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
        mine: 0,
        others: 0,
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
        origin: None,
        mine: true,
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
        places: Places::default(),
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
             Shell: /bin/zsh, 276 aliases, 1,267 functions (24 did not parse).\n\
             Definitions: 0 mine, 0 others.\n\n"
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
            "-",
            "cwt"
        ]
    );
    assert_eq!(
        row(&report, "zz"),
        ["zz", "function", "0", "0", "", "-", "-", "-", "echo zz"]
    );
    assert_eq!(row(&report, "gx")[1], "global alias");
    let names: Vec<&str> = table_lines(&report)
        .iter()
        .filter_map(|line| line.strip_prefix('│'))
        .filter_map(|line| line.split('┆').next())
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
    assert!(cells[8].ends_with('…'), "{report}");
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
    assert!(row(&report, "wide")[8].ends_with('…'), "{report}");
}

/// A short definition is not cut.
#[test]
fn short_definition_is_not_cut() {
    let report = text(&one("echo hi"), &window(), "/bin/zsh", Some(80));
    assert!(!report.contains('…'), "{report}");
    assert_eq!(row(&report, "wide")[8], "echo hi");
}

/// A long `Through` cell wraps inside 30 columns when the report has a width,
/// also when the table has room for the whole cell.
#[test]
fn long_through_cell_wraps_inside_thirty_columns() {
    let mut used = never("wt", Kind::Alias, "cwt");
    used.through = (1..=6_u64).map(|n| (format!("alias{n}"), 7 - n)).collect();
    used.uses = 21;
    used.classification = Classification::Used;
    let report = text(&tally(vec![used]), &window(), "/bin/zsh", Some(200));
    assert_fits(&report, 200);
    let cells: Vec<&str> = table_lines(&report)
        .into_iter()
        .filter(|line| line.starts_with('│'))
        .skip(1)
        .filter_map(|line| line.split('┆').nth(4))
        .map(str::trim)
        .collect();
    assert!(cells.len() > 1, "the cell wraps: {report}");
    for cell in &cells {
        assert!(
            cell.width() <= 30,
            "{} > 30: {cell}\n{report}",
            cell.width()
        );
    }
    assert_eq!(
        cells.join(" "),
        "alias1 6, alias2 5, alias3 4, alias4 3, alias5 2, alias6 1"
    );
}

/// With no width, a long `Through` cell stays on one line.
#[test]
fn no_width_keeps_the_through_cell_on_one_line() {
    let mut used = never("wt", Kind::Alias, "cwt");
    used.through = (1..=6_u64).map(|n| (format!("alias{n}"), 7 - n)).collect();
    used.uses = 21;
    used.classification = Classification::Used;
    let report = text(&tally(vec![used]), &window(), "/bin/zsh", None);
    assert_eq!(
        table_lines(&report).len(),
        5,
        "top, header, rule, one row, bottom: {report}"
    );
    assert_eq!(
        row(&report, "wt")[4],
        "alias1 6, alias2 5, alias3 4, alias4 3, alias5 2, alias6 1"
    );
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

/// The places of the origin tests: the home is `/home/me`, and oh-my-zsh is
/// in `/home/me/omz`.
fn places() -> Places {
    Places {
        home: Some(PathBuf::from("/home/me")),
        zsh: Some(PathBuf::from("/home/me/omz")),
        zsh_custom: None,
    }
}

/// The file of the git plugin of the oh-my-zsh of [`places`].
const GIT_PLUGIN: &str = "/home/me/omz/plugins/git/git.plugin.zsh";

/// Makes a definition that never ran, defined in `file` at `line`, and mine
/// when `mine` is true.
fn from(name: &str, kind: Kind, file: &str, line: Option<u32>, mine: bool) -> Usage {
    Usage {
        origin: Some(Origin {
            file: PathBuf::from(file),
            line,
        }),
        mine,
        ..never(name, kind, "x")
    }
}

/// Makes a tally with two of my definitions and three others, in tally
/// order. `bare` has an unknown origin.
fn mixed() -> Tally {
    let mut counts = tally(vec![
        from("wt", Kind::Alias, "/home/me/.zshrc", Some(3), true),
        from("gst", Kind::Alias, GIT_PLUGIN, Some(1), false),
        never("bare", Kind::Alias, "ls"),
        from("gfun", Kind::Function, GIT_PLUGIN, None, false),
        from(
            "_comp",
            Kind::Function,
            "/usr/share/zsh/functions/_comp",
            None,
            false,
        ),
    ]);
    counts.summary.mine = 2;
    counts.summary.others = 3;
    counts.places = places();
    counts
}

/// Makes the window of the tests of `--all`.
fn all_window() -> Window {
    Window {
        all: true,
        ..window()
    }
}

/// Gives the names in the first cells of the rows of the table lines in
/// `text`, the header included.
fn names(text: &str) -> Vec<&str> {
    table_lines(text)
        .iter()
        .filter_map(|line| line.strip_prefix('│'))
        .filter_map(|line| line.split('┆').next())
        .map(str::trim)
        .collect()
}

#[test]
fn the_header_counts_mine_and_the_others() {
    let report = text(&mixed(), &window(), "/bin/zsh", None);
    assert!(
        report.contains(
            "(24 did not parse).\nDefinitions: 2 mine, 3 others (--all shows the others).\n\n"
        ),
        "{report}"
    );
    let report = text(&mixed(), &all_window(), "/bin/zsh", None);
    assert!(
        report.contains("(24 did not parse).\nDefinitions: 2 mine, 3 others.\n\n"),
        "{report}"
    );
}

#[test]
fn the_origin_column_shows_a_tilde_for_home_and_a_dash_when_unknown() {
    let report = text(&mixed(), &all_window(), "/bin/zsh", None);
    assert_eq!(row(&report, "wt")[7], "~/.zshrc:3");
    assert_eq!(row(&report, "bare")[7], "-");
    assert_eq!(row(&report, "gfun")[7], "~/omz/plugins/git/git.plugin.zsh");
    assert_eq!(row(&report, "_comp")[7], "/usr/share/zsh/functions/_comp");
    assert!(report.contains("│ Name "), "{report}");
    assert!(report.contains("┆ Origin "), "{report}");
}

#[test]
fn by_default_the_text_shows_only_my_definitions() {
    let report = text(&mixed(), &window(), "/bin/zsh", None);
    assert_eq!(names(&report), ["Name", "wt", "bare"]);
    assert!(
        report.contains("Never used:\n  bare (alias)\n  wt (alias)\nIdle"),
        "{report}"
    );
    for other in ["gst", "gfun", "_comp", "git.plugin.zsh"] {
        assert!(!report.contains(other), "{other}: {report}");
    }
}

/// With `--all`, my definitions come first under their own heading. Then
/// each other file has a heading with its count and a table, in path order.
#[test]
fn all_groups_the_others_by_origin_with_a_count() {
    let report = text(&mixed(), &all_window(), "/bin/zsh", None);
    let mine = report.find("\nMine: 2 definitions\n┌").expect(&report);
    let plugin = report
        .find("\n~/omz/plugins/git/git.plugin.zsh: 2 definitions\n┌")
        .expect(&report);
    let comp = report
        .find("\n/usr/share/zsh/functions/_comp: 1 definition\n┌")
        .expect(&report);
    assert!(mine < plugin && plugin < comp, "{report}");
    assert_eq!(names(&report[mine..plugin]), ["Name", "wt", "bare"]);
    assert_eq!(names(&report[plugin..comp]), ["Name", "gst", "gfun"]);
    assert_eq!(names(&report[comp..]), ["Name", "_comp"]);
    assert!(
        report.contains(
            "Never used:\n  _comp (function)\n  bare (alias)\n  gfun (function)\n  gst (alias)\n  wt (alias)\n"
        ),
        "{report}"
    );
}

/// The JSON document holds the others too, also without `--all`, with the
/// origin of each definition and a field that says if it is mine.
#[test]
fn the_json_holds_the_origin_and_mine_of_each_definition() {
    let value: serde_json::Value =
        serde_json::from_str(&json(&mixed(), &window(), "/bin/zsh")).unwrap();
    assert_eq!(value["shell"]["mine"], 2);
    assert_eq!(value["shell"]["others"], 3);
    let defs = value["definitions"].as_array().unwrap();
    assert_eq!(defs.len(), 5);
    assert_eq!(
        defs[0]["origin"],
        serde_json::json!({ "file": "/home/me/.zshrc", "line": 3 })
    );
    assert_eq!(defs[0]["mine"], true);
    assert_eq!(defs[1]["name"], "gst");
    assert_eq!(defs[1]["mine"], false);
    assert!(defs[2]["origin"].is_null(), "{value}");
    assert_eq!(defs[2]["mine"], true);
    assert_eq!(
        defs[3]["origin"],
        serde_json::json!({ "file": GIT_PLUGIN, "line": null })
    );
    let never: Vec<&str> = value["never_used"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert_eq!(never, ["_comp", "bare", "gfun", "gst", "wt"]);
}
