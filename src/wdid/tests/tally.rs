//! Tests of the tally of the tool uses.

use chrono::{Days, NaiveDate, NaiveDateTime};
use wdid::catalog::{Catalog, Tool};
use wdid::command_words::{command_words, Aliases, Parsed};
use wdid::history::{Exit, HistoryEntry};
use wdid::shell::{Definitions, Resolver};
use wdid::tally::{tally, Classification, Tally, ToolUsage, Unparsed, Window};
use wdid::tool::ToolName;

/// The date of the run in every test.
fn today() -> NaiveDate {
    date(2026, 10, 14)
}

/// Makes a date.
fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// Makes a window with the default limits and no `since` date.
fn window() -> Window {
    Window {
        today: today(),
        since: None,
        new_days: 90,
        idle_days: 180,
    }
}

/// Makes a row from a date, an exit code, and a command.
fn row_on(day: NaiveDate, exit: i64, command: &str) -> HistoryEntry {
    HistoryEntry {
        time: NaiveDateTime::new(day, chrono::NaiveTime::from_hms_opt(12, 30, 0).unwrap()),
        exit: Exit::new(exit),
        host: "host".to_string(),
        user: "user".to_string(),
        command: command.to_string(),
    }
}

/// Makes a row on a fixed recent day.
fn row(exit: i64, command: &str) -> HistoryEntry {
    row_on(date(2026, 10, 1), exit, command)
}

/// Makes a tool with a known added date.
fn tool(name: &str, added: Option<NaiveDate>) -> Tool {
    Tool {
        name: ToolName::new(name),
        crate_dir: format!("src/{name}").into(),
        added,
        installed: true,
        path: None,
    }
}

/// Makes a catalog of tools that were added long ago, sorted by name.
fn catalog(names: &[&str]) -> Catalog {
    Catalog {
        root: "/work".into(),
        tools: names
            .iter()
            .map(|name| tool(name, Some(date(2020, 1, 1))))
            .collect(),
    }
}

/// Makes definitions from alias pairs and function pairs (name, body).
fn definitions(aliases: &[(&str, &str)], functions: &[(&str, &str)]) -> Definitions {
    let mut result = Definitions {
        aliases: aliases
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect(),
        ..Definitions::default()
    };
    for (name, body) in functions {
        match command_words(body, &Aliases::new()) {
            Parsed::Words(words) => {
                result.functions.insert((*name).to_string(), words);
            }
            Parsed::Unparsed => result.unparsed_functions.push((*name).to_string()),
        }
    }
    result
}

/// Runs the tally with a window.
fn run_in(
    entries: &[HistoryEntry],
    definitions: &Definitions,
    catalog: &Catalog,
    window: &Window,
) -> Tally {
    let resolver = Resolver::new(definitions, &catalog.names());
    tally(entries, definitions, &resolver, catalog, window)
}

/// Runs the tally with the default window.
fn run(entries: &[HistoryEntry], definitions: &Definitions, catalog: &Catalog) -> Tally {
    run_in(entries, definitions, catalog, &window())
}

/// Finds the usage of one tool.
fn usage<'a>(tally: &'a Tally, name: &str) -> &'a ToolUsage {
    tally
        .tools
        .iter()
        .find(|usage| usage.tool.name.as_str() == name)
        .unwrap_or_else(|| panic!("no tool {name}"))
}

/// Makes the expected `through` list.
fn through(pairs: &[(&str, u64)]) -> Vec<(String, u64)> {
    pairs
        .iter()
        .map(|(name, count)| ((*name).to_string(), *count))
        .collect()
}

#[test]
fn a_typed_use_counts_as_typed() {
    let result = run(
        &[row(0, "prcp a b"), row(0, "ls | prcp"), row(0, "ls")],
        &Definitions::default(),
        &catalog(&["cwt", "prcp"]),
    );
    let prcp = usage(&result, "prcp");
    assert_eq!(prcp.typed, 2);
    assert_eq!(prcp.uses, 2);
    assert!(prcp.through.is_empty());
    assert_eq!(usage(&result, "cwt").uses, 0);
}

#[test]
fn a_use_through_an_alias_counts_for_the_alias() {
    let defs = definitions(&[("pc", "prcp --fast")], &[]);
    let result = run(
        &[row(0, "pc x"), row(0, "pc y"), row(0, "prcp z")],
        &defs,
        &catalog(&["prcp"]),
    );
    let prcp = usage(&result, "prcp");
    assert_eq!(prcp.typed, 1);
    assert_eq!(prcp.through, through(&[("pc", 2)]));
    assert_eq!(prcp.uses, 3);
}

#[test]
fn a_use_through_a_function_counts_for_the_function() {
    let defs = definitions(&[], &[("wt", "cwt \"$@\"")]);
    let result = run(
        &[row(0, "wt a"), row(0, "wt b")],
        &defs,
        &catalog(&["cwt"]),
    );
    let cwt = usage(&result, "cwt");
    assert_eq!(cwt.typed, 0);
    assert_eq!(cwt.through, through(&[("wt", 2)]));
    assert_eq!(cwt.uses, 2);
}

#[test]
fn a_use_through_an_alias_of_a_function_counts_for_the_alias() {
    let defs = definitions(&[("wtf", "wt -f")], &[("wt", "cwt \"$@\"")]);
    let result = run(&[row(0, "wtf")], &defs, &catalog(&["cwt"]));
    let cwt = usage(&result, "cwt");
    assert_eq!(cwt.typed, 0);
    assert_eq!(cwt.through, through(&[("wtf", 1)]));
    assert_eq!(cwt.uses, 1);
}

#[test]
fn a_function_named_like_a_tool_counts_as_typed_once() {
    let defs = definitions(&[], &[("crap", "command crap \"$@\"")]);
    let result = run(&[row(0, "crap x")], &defs, &catalog(&["crap"]));
    let crap = usage(&result, "crap");
    assert_eq!(crap.typed, 1);
    assert!(crap.through.is_empty());
    assert_eq!(crap.uses, 1);
}

#[test]
fn a_function_that_calls_two_tools_counts_both() {
    let defs = definitions(&[], &[("both", "prcp a; prhash b")]);
    let result = run(&[row(0, "both")], &defs, &catalog(&["prcp", "prhash"]));
    assert_eq!(usage(&result, "prcp").through, through(&[("both", 1)]));
    assert_eq!(usage(&result, "prhash").through, through(&[("both", 1)]));
}

#[test]
fn through_names_are_sorted_by_count_then_by_name() {
    let defs = definitions(&[("b", "prcp"), ("a", "prcp"), ("c", "prcp")], &[]);
    let result = run(
        &[
            row(0, "c"),
            row(0, "c"),
            row(0, "c"),
            row(0, "b"),
            row(0, "a"),
        ],
        &defs,
        &catalog(&["prcp"]),
    );
    assert_eq!(
        usage(&result, "prcp").through,
        through(&[("c", 3), ("a", 1), ("b", 1)])
    );
}

#[test]
fn first_use_and_last_use_come_from_the_dates_of_the_counted_rows() {
    let defs = definitions(&[("pc", "prcp")], &[]);
    let result = run(
        &[
            row_on(date(2026, 3, 5), 0, "prcp"),
            row_on(date(2026, 1, 2), 0, "pc"),
            row_on(date(2026, 8, 9), 0, "prcp"),
        ],
        &defs,
        &catalog(&["prcp"]),
    );
    let prcp = usage(&result, "prcp");
    assert_eq!(prcp.first_use, Some(date(2026, 1, 2)));
    assert_eq!(prcp.last_use, Some(date(2026, 8, 9)));
}

#[test]
fn the_same_command_in_two_rows_counts_twice() {
    let result = run(
        &[row(0, "prcp a"), row(0, "prcp a")],
        &Definitions::default(),
        &catalog(&["prcp"]),
    );
    assert_eq!(usage(&result, "prcp").uses, 2);
}

#[test]
fn a_multibyte_command_counts() {
    let result = run(
        &[row(0, "prcp 日本語.txt 🎉/")],
        &Definitions::default(),
        &catalog(&["prcp"]),
    );
    assert_eq!(usage(&result, "prcp").typed, 1);
}

/// Keeps the imports of later groups in use.
#[allow(dead_code, reason = "later test groups use these items")]
fn later(_: Days, _: Classification, _: Unparsed) {}
