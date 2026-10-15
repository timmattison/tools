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
    let result = run(&[row(0, "wt a"), row(0, "wt b")], &defs, &catalog(&["cwt"]));
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

#[test]
fn a_development_run_counts_apart_from_the_uses() {
    let result = run(
        &[
            row_on(date(2026, 9, 1), 0, "./target/release/prcp a"),
            row_on(date(2026, 9, 2), 0, "cargo run --bin prcp -- x"),
            row_on(date(2026, 9, 3), 0, "~/.cargo/bin/prcp"),
        ],
        &Definitions::default(),
        &catalog(&["prcp"]),
    );
    let prcp = usage(&result, "prcp");
    assert_eq!(prcp.development_runs, 2);
    assert_eq!(prcp.typed, 1);
    assert_eq!(prcp.uses, 1);
    assert_eq!(prcp.first_use, Some(date(2026, 9, 3)));
    assert_eq!(prcp.last_use, Some(date(2026, 9, 3)));
}

#[test]
fn a_row_with_exit_127_is_not_counted() {
    let result = run(
        &[row(127, "prcp a"), row(0, "prcp b")],
        &Definitions::default(),
        &catalog(&["prcp"]),
    );
    assert_eq!(usage(&result, "prcp").uses, 1);
    assert_eq!(result.summary.not_found, 1);
}

#[test]
fn a_row_with_exit_minus_one_is_counted() {
    let result = run(
        &[row(-1, "prcp a"), row(1, "prcp b")],
        &Definitions::default(),
        &catalog(&["prcp"]),
    );
    assert_eq!(usage(&result, "prcp").uses, 2);
    assert_eq!(result.summary.not_found, 0);
}

#[test]
fn the_since_date_drops_the_days_before_it_and_keeps_the_day_itself() {
    let since = date(2026, 6, 15);
    let entries = [
        row_on(since.pred_opt().unwrap(), 0, "prcp"),
        row_on(since, 0, "prcp"),
        row_on(since.succ_opt().unwrap(), 0, "prcp"),
    ];
    let result = run_in(
        &entries,
        &Definitions::default(),
        &catalog(&["prcp"]),
        &Window {
            since: Some(since),
            ..window()
        },
    );
    let prcp = usage(&result, "prcp");
    assert_eq!(prcp.uses, 2);
    assert_eq!(prcp.first_use, Some(since));
    assert_eq!(result.summary.before_since, 1);
}

#[test]
fn a_row_before_since_is_not_a_not_found_row() {
    let since = date(2026, 6, 15);
    let entries = [
        row_on(date(2026, 6, 1), 127, "prcp"),
        row_on(date(2026, 6, 20), 127, "prcp"),
    ];
    let result = run_in(
        &entries,
        &Definitions::default(),
        &catalog(&["prcp"]),
        &Window {
            since: Some(since),
            ..window()
        },
    );
    assert_eq!(result.summary.before_since, 1);
    assert_eq!(result.summary.not_found, 1);
}

#[test]
fn without_since_every_row_counts() {
    let result = run(
        &[row_on(date(1999, 1, 1), 0, "prcp")],
        &Definitions::default(),
        &catalog(&["prcp"]),
    );
    assert_eq!(usage(&result, "prcp").uses, 1);
    assert_eq!(result.summary.before_since, 0);
}

/// Gives the date that is `days` days before today.
fn days_ago(days: u64) -> NaiveDate {
    today().checked_sub_days(Days::new(days)).unwrap()
}

/// Classifies a tool that was added on `added` and that ran last on `last`.
fn classify(added: Option<NaiveDate>, last: Option<NaiveDate>) -> Classification {
    let catalog = Catalog {
        root: "/work".into(),
        tools: vec![tool("prcp", added)],
    };
    let entries: Vec<HistoryEntry> = last.map(|day| row_on(day, 0, "prcp")).into_iter().collect();
    let result = run(&entries, &Definitions::default(), &catalog);
    usage(&result, "prcp").classification
}

#[test]
fn a_tool_that_never_ran_is_new_below_the_new_days_limit() {
    assert_eq!(
        classify(Some(days_ago(89)), None),
        Classification::NeverUsedNew
    );
    assert_eq!(
        classify(Some(days_ago(90)), None),
        Classification::NeverUsedOld
    );
    assert_eq!(
        classify(Some(days_ago(91)), None),
        Classification::NeverUsedOld
    );
    assert_eq!(classify(Some(today()), None), Classification::NeverUsedNew);
}

#[test]
fn a_tool_with_an_unknown_added_date_that_never_ran_is_old() {
    assert_eq!(classify(None, None), Classification::NeverUsedOld);
}

#[test]
fn a_tool_that_ran_is_idle_above_the_idle_days_limit() {
    let added = Some(days_ago(1000));
    assert_eq!(classify(added, Some(days_ago(179))), Classification::Used);
    assert_eq!(classify(added, Some(days_ago(180))), Classification::Used);
    assert_eq!(classify(added, Some(days_ago(181))), Classification::Idle);
    assert_eq!(classify(added, Some(today())), Classification::Used);
}

#[test]
fn the_limits_come_from_the_window() {
    let catalog = Catalog {
        root: "/work".into(),
        tools: vec![
            tool("old", Some(days_ago(10))),
            tool("ran", Some(days_ago(500))),
        ],
    };
    let entries = [row_on(days_ago(20), 0, "ran")];
    let result = run_in(
        &entries,
        &Definitions::default(),
        &catalog,
        &Window {
            new_days: 5,
            idle_days: 19,
            ..window()
        },
    );
    assert_eq!(
        usage(&result, "old").classification,
        Classification::NeverUsedOld
    );
    assert_eq!(usage(&result, "ran").classification, Classification::Idle);
}

#[test]
fn a_use_before_the_added_date_counts() {
    let added = date(2026, 9, 1);
    let catalog = Catalog {
        root: "/work".into(),
        tools: vec![tool("prcp", Some(added))],
    };
    let result = run(
        &[row_on(date(2026, 8, 1), 0, "prcp")],
        &Definitions::default(),
        &catalog,
    );
    let prcp = usage(&result, "prcp");
    assert_eq!(prcp.uses, 1);
    assert_eq!(prcp.first_use, Some(date(2026, 8, 1)));
    assert!(prcp.first_use < prcp.tool.added);
    assert_eq!(prcp.classification, Classification::Used);
}

#[test]
fn a_development_run_alone_does_not_make_a_tool_used() {
    let result = run(
        &[row_on(days_ago(1), 0, "./target/debug/prcp")],
        &Definitions::default(),
        &catalog(&["prcp"]),
    );
    let prcp = usage(&result, "prcp");
    assert_eq!(prcp.development_runs, 1);
    assert_eq!(prcp.classification, Classification::NeverUsedOld);
}

/// Makes the expected `Unparsed` value.
fn unparsed(command: &str, rows: u64) -> Unparsed {
    Unparsed {
        command: command.to_string(),
        rows,
    }
}

#[test]
fn unparsed_rows_are_counted_and_grouped_by_command() {
    let result = run(
        &[
            row(0, "echo $("),
            row(0, "a |"),
            row(0, "echo $("),
            row(0, "prcp x"),
            row(0, "echo $("),
            row(0, "a |"),
            row(0, ";;"),
        ],
        &Definitions::default(),
        &catalog(&["prcp"]),
    );
    assert_eq!(result.summary.unparsed_rows, 6);
    assert_eq!(
        result.summary.unparsed,
        vec![
            unparsed("echo $(", 3),
            unparsed("a |", 2),
            unparsed(";;", 1)
        ]
    );
}

#[test]
fn unparsed_commands_with_equal_rows_sort_by_command() {
    let result = run(
        &[row(0, "b |"), row(0, "a |"), row(0, "c |")],
        &Definitions::default(),
        &catalog(&["prcp"]),
    );
    assert_eq!(
        result.summary.unparsed,
        vec![unparsed("a |", 1), unparsed("b |", 1), unparsed("c |", 1)]
    );
}

#[test]
fn unparsed_rows_skip_the_rows_that_the_filters_drop() {
    let since = date(2026, 6, 15);
    let result = run_in(
        &[
            row_on(date(2026, 6, 1), 0, "a |"),
            row_on(date(2026, 6, 20), 127, "a |"),
            row_on(date(2026, 6, 20), 0, "a |"),
        ],
        &Definitions::default(),
        &catalog(&["prcp"]),
        &Window {
            since: Some(since),
            ..window()
        },
    );
    assert_eq!(result.summary.unparsed_rows, 1);
    assert_eq!(result.summary.unparsed, vec![unparsed("a |", 1)]);
}

#[test]
fn the_summary_counts_rows_aliases_and_functions() {
    let mut defs = definitions(
        &[("pc", "prcp"), ("ll", "ls -l"), ("bad", "echo $(")],
        &[("wt", "cwt"), ("broken", "echo $("), ("worse", "a |")],
    );
    defs.unparsed_aliases = vec!["alias x=".to_string()];
    let result = run(
        &[
            row_on(date(2020, 1, 1), 0, "prcp"),
            row(127, "nothing"),
            row(0, "ls"),
        ],
        &defs,
        &catalog(&["cwt", "prcp"]),
    );
    let summary = &result.summary;
    assert_eq!(summary.rows, 3);
    assert_eq!(summary.not_found, 1);
    assert_eq!(summary.aliases, 3);
    assert_eq!(summary.functions, 3);
    assert_eq!(
        summary.unparsed_functions,
        vec!["broken".to_string(), "worse".to_string()]
    );
    assert_eq!(summary.unparsed_aliases, vec!["alias x=".to_string()]);
}

#[test]
fn the_rows_of_the_summary_include_the_rows_before_since() {
    let result = run_in(
        &[row_on(date(2026, 1, 1), 0, "prcp"), row(0, "prcp")],
        &Definitions::default(),
        &catalog(&["prcp"]),
        &Window {
            since: Some(date(2026, 6, 15)),
            ..window()
        },
    );
    assert_eq!(result.summary.rows, 2);
    assert_eq!(result.summary.before_since, 1);
}

#[test]
fn every_tool_of_the_catalog_has_an_entry_even_without_a_use() {
    let result = run(
        &[row(0, "ls")],
        &Definitions::default(),
        &catalog(&["a", "b", "c"]),
    );
    assert_eq!(result.tools.len(), 3);
    assert!(result.tools.iter().all(|usage| usage.uses == 0));
}

#[test]
fn tools_sort_by_uses_then_by_name() {
    let result = run(
        &[
            row(0, "mid"),
            row(0, "most"),
            row(0, "most"),
            row(0, "most"),
            row(0, "mid"),
            row(0, "also"),
            row(0, "also"),
        ],
        &Definitions::default(),
        &catalog(&["also", "mid", "most", "none", "zero"]),
    );
    let names: Vec<&str> = result
        .tools
        .iter()
        .map(|usage| usage.tool.name.as_str())
        .collect();
    assert_eq!(names, ["most", "also", "mid", "none", "zero"]);
}
