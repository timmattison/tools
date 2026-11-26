//! Tests of the count of the uses of each alias and each function.

use chrono::{Days, NaiveDate, NaiveDateTime};
use wdid::command_words::{command_words, Aliases, Parsed};
use wdid::defs::{tally, Classification, Kind, Tally, Usage, Window};
use wdid::history::{Exit, HistoryEntry};
use wdid::shell::Definitions;

/// The idle limit of the default window.
const IDLE_DAYS: u32 = 30;

/// The date of the run in every test.
fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 14).unwrap()
}

/// Makes the date `days` days before today.
fn days_ago(days: u64) -> NaiveDate {
    today().checked_sub_days(Days::new(days)).unwrap()
}

/// Makes a window with the default limit and no `since` date.
fn window() -> Window {
    Window {
        today: today(),
        since: None,
        idle_days: IDLE_DAYS,
    }
}

/// Makes a row `days` days before today, with an exit code and a command.
fn entry(days: u64, exit: i64, command: &str) -> HistoryEntry {
    HistoryEntry {
        time: NaiveDateTime::new(
            days_ago(days),
            chrono::NaiveTime::from_hms_opt(12, 30, 0).unwrap(),
        ),
        exit: Exit::new(exit),
        host: "host".to_string(),
        user: "user".to_string(),
        command: command.to_string(),
    }
}

/// Makes a row that succeeded, one day ago.
fn row(command: &str) -> HistoryEntry {
    entry(1, 0, command)
}

/// Makes definitions from regular aliases, global aliases, suffix aliases,
/// and functions (name, body). A function body parses with an empty alias
/// table.
fn definitions(
    aliases: &[(&str, &str)],
    globals: &[(&str, &str)],
    suffixes: &[(&str, &str)],
    functions: &[(&str, &str)],
) -> Definitions {
    let mut table = Aliases::new();
    for (name, value) in aliases {
        table.insert(*name, *value);
    }
    for (name, value) in globals {
        table.insert_global(*name, *value);
    }
    for (suffix, value) in suffixes {
        table.insert_suffix(*suffix, *value);
    }
    let mut result = Definitions {
        aliases: table,
        ..Definitions::default()
    };
    for (name, body) in functions {
        let line = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        result
            .function_lines
            .insert((*name).to_string(), line.trim().to_string());
        match command_words(body, &Aliases::new()) {
            Parsed::Words(words) => {
                result.functions.insert((*name).to_string(), words);
            }
            Parsed::Unparsed => result.unparsed_functions.push((*name).to_string()),
        }
    }
    result
}

/// Runs the count with the default window.
fn run(entries: &[HistoryEntry], definitions: &Definitions) -> Tally {
    tally(entries, definitions, &window())
}

/// Finds the entry of one name and kind.
fn usage<'a>(tally: &'a Tally, name: &str, kind: Kind) -> &'a Usage {
    tally
        .definitions
        .iter()
        .find(|usage| usage.name == name && usage.kind == kind)
        .unwrap_or_else(|| panic!("no entry {name}"))
}

/// Makes the expected `through` list.
fn through(pairs: &[(&str, u64)]) -> Vec<(String, u64)> {
    pairs
        .iter()
        .map(|(name, count)| ((*name).to_string(), *count))
        .collect()
}

/// A typed alias counts as typed.
#[test]
fn a_typed_alias_counts_as_typed() {
    let defs = definitions(&[("wt", "cwt")], &[], &[], &[]);
    let result = run(&[row("wt a"), row("wt b")], &defs);
    let wt = usage(&result, "wt", Kind::Alias);
    assert_eq!(wt.uses, 2);
    assert_eq!(wt.typed, 2);
    assert!(wt.through.is_empty());
}

/// A typed function counts as typed.
#[test]
fn a_typed_function_counts_as_typed() {
    let defs = definitions(&[], &[], &[], &[("myf", "\techo hi")]);
    let result = run(&[row("myf"), row("myf x")], &defs);
    let myf = usage(&result, "myf", Kind::Function);
    assert_eq!(myf.uses, 2);
    assert_eq!(myf.typed, 2);
}

/// An alias in the value of another alias counts through the typed alias.
#[test]
fn an_alias_of_an_alias_counts_through_the_typed_one() {
    let defs = definitions(&[("wtf", "wt -f"), ("wt", "cwt")], &[], &[], &[]);
    let result = run(&[row("wtf")], &defs);
    let wtf = usage(&result, "wtf", Kind::Alias);
    assert_eq!((wtf.uses, wtf.typed), (1, 1));
    let wt = usage(&result, "wt", Kind::Alias);
    assert_eq!((wt.uses, wt.typed), (1, 0));
    assert_eq!(wt.through, through(&[("wtf", 1)]));
}

/// A function counts every function that its body calls, to any depth.
#[test]
fn a_function_counts_the_functions_it_calls_to_depth_two() {
    let defs = definitions(
        &[],
        &[],
        &[],
        &[("a", "\tb"), ("b", "\tc"), ("c", "\techo")],
    );
    let result = run(&[row("a")], &defs);
    let a = usage(&result, "a", Kind::Function);
    assert_eq!((a.uses, a.typed), (1, 1));
    assert_eq!(
        usage(&result, "b", Kind::Function).through,
        through(&[("a", 1)])
    );
    assert_eq!(
        usage(&result, "c", Kind::Function).through,
        through(&[("a", 1)])
    );
}

/// A cycle and a self-call add no use to the function that starts the walk.
#[test]
fn a_cycle_and_a_self_call_add_nothing_to_the_start() {
    let defs = definitions(&[], &[], &[], &[("f", "\tg"), ("g", "\tf"), ("h", "\th x")]);
    let result = run(&[row("f"), row("h")], &defs);
    let f = usage(&result, "f", Kind::Function);
    assert_eq!((f.uses, f.typed), (1, 1));
    assert!(f.through.is_empty());
    assert_eq!(
        usage(&result, "g", Kind::Function).through,
        through(&[("f", 1)])
    );
    assert_eq!(usage(&result, "h", Kind::Function).uses, 1);
}

/// A global alias counts in every word position.
#[test]
fn a_global_alias_counts_in_each_position() {
    let defs = definitions(&[], &[("G", "| cat")], &[], &[]);
    let result = run(&[row("ls G"), row("ls G; echo G")], &defs);
    let global = usage(&result, "G", Kind::GlobalAlias);
    assert_eq!((global.uses, global.typed), (3, 3));
}

/// A suffix alias has its marker as its name.
#[test]
fn a_suffix_alias_counts_by_its_marker() {
    let defs = definitions(&[], &[], &[("md", "glow")], &[]);
    let result = run(&[row("README.md")], &defs);
    let suffix = usage(&result, "*.md", Kind::SuffixAlias);
    assert_eq!((suffix.uses, suffix.typed), (1, 1));
    assert_eq!(suffix.definition, "glow");
}

/// A row with exit code 127 does not count.
#[test]
fn a_row_with_exit_127_does_not_count() {
    let defs = definitions(&[("wt", "cwt")], &[], &[], &[]);
    let result = run(&[entry(1, 127, "wt a"), entry(1, 0, "wt b")], &defs);
    assert_eq!(usage(&result, "wt", Kind::Alias).uses, 1);
    assert_eq!(result.summary.not_found, 1);
}

/// A name in quoted text does not count.
#[test]
fn a_name_in_quoted_text_does_not_count() {
    let defs = definitions(&[("wt", "cwt")], &[], &[], &[("myf", "\techo hi")]);
    let result = run(
        &[
            row("echo \"wt\""),
            row("git commit -m 'myf wt'"),
            row("'wt'"),
        ],
        &defs,
    );
    assert_eq!(usage(&result, "wt", Kind::Alias).uses, 0);
    assert_eq!(usage(&result, "myf", Kind::Function).uses, 0);
}

/// An alias whose value calls a function counts the function through it.
#[test]
fn an_alias_that_calls_a_function_counts_the_function() {
    let defs = definitions(&[("g", "myf --x")], &[], &[], &[("myf", "\techo hi")]);
    let result = run(&[row("g")], &defs);
    let g = usage(&result, "g", Kind::Alias);
    assert_eq!((g.uses, g.typed), (1, 1));
    let myf = usage(&result, "myf", Kind::Function);
    assert_eq!((myf.uses, myf.typed), (1, 0));
    assert_eq!(myf.through, through(&[("g", 1)]));
}

/// An alias that starts a function walk is the name of each use through it,
/// also at depth two.
#[test]
fn an_alias_that_starts_a_function_walk_names_the_alias() {
    let defs = definitions(&[("g", "a")], &[], &[], &[("a", "\tb"), ("b", "\techo")]);
    let result = run(&[row("g")], &defs);
    let a = usage(&result, "a", Kind::Function);
    assert_eq!((a.uses, a.typed), (1, 0));
    assert_eq!(a.through, through(&[("g", 1)]));
    let b = usage(&result, "b", Kind::Function);
    assert_eq!((b.uses, b.typed), (1, 0));
    assert_eq!(b.through, through(&[("g", 1)]));
}

/// An alias whose value calls the function of the same name is a typed use of
/// the alias and a use of the function through the alias.
#[test]
fn an_alias_that_calls_the_function_of_its_name_counts_both() {
    let defs = definitions(&[("x", "x --flag")], &[], &[], &[("x", "\techo")]);
    let result = run(&[row("x")], &defs);
    let alias = usage(&result, "x", Kind::Alias);
    assert_eq!((alias.uses, alias.typed), (1, 1));
    assert!(alias.through.is_empty());
    let function = usage(&result, "x", Kind::Function);
    assert_eq!((function.uses, function.typed), (1, 0));
    assert_eq!(function.through, through(&[("x", 1)]));
}

/// An alias in a function body does not count.
#[test]
fn an_alias_in_a_function_body_does_not_count() {
    let defs = definitions(&[("ll", "ls -l")], &[], &[], &[("f", "\tll x")]);
    let result = run(&[row("f")], &defs);
    assert_eq!(usage(&result, "ll", Kind::Alias).uses, 0);
}

/// A private function does not show, but a call through it counts.
#[test]
fn a_private_function_does_not_show_but_passes_the_call() {
    let defs = definitions(
        &[],
        &[],
        &[],
        &[("f", "\t_helper"), ("_helper", "\tg"), ("g", "\techo")],
    );
    let result = run(&[row("f")], &defs);
    assert!(result.definitions.iter().all(|u| u.name != "_helper"));
    assert_eq!(
        usage(&result, "g", Kind::Function).through,
        through(&[("f", 1)])
    );
    assert_eq!(result.summary.functions, 2);
}

/// The classification follows the last use and the idle limit.
#[test]
fn the_classification_follows_the_last_use() {
    let defs = definitions(
        &[("never", "a"), ("idle", "b"), ("fresh", "c")],
        &[],
        &[],
        &[],
    );
    let result = run(&[entry(40, 0, "idle"), entry(3, 0, "fresh")], &defs);
    assert_eq!(
        usage(&result, "never", Kind::Alias).classification,
        Classification::NeverUsed
    );
    assert_eq!(
        usage(&result, "idle", Kind::Alias).classification,
        Classification::Idle
    );
    assert_eq!(
        usage(&result, "fresh", Kind::Alias).classification,
        Classification::Used
    );
}

/// The `since` date drops the earlier rows.
#[test]
fn since_drops_the_earlier_rows() {
    let defs = definitions(&[("wt", "cwt")], &[], &[], &[]);
    let window = Window {
        since: Some(days_ago(5)),
        ..window()
    };
    let result = tally(&[entry(10, 0, "wt"), entry(2, 0, "wt")], &defs, &window);
    let wt = usage(&result, "wt", Kind::Alias);
    assert_eq!(wt.uses, 1);
    assert_eq!(wt.first_use, Some(days_ago(2)));
    assert_eq!(result.summary.before_since, 1);
}

/// The first and the last use cover the typed uses and the uses through.
#[test]
fn the_first_and_last_use_cover_typed_and_through_uses() {
    let defs = definitions(&[("wtf", "wt -f"), ("wt", "cwt")], &[], &[], &[]);
    let result = run(
        &[entry(9, 0, "wtf"), entry(4, 0, "wt"), entry(2, 0, "wtf")],
        &defs,
    );
    let wt = usage(&result, "wt", Kind::Alias);
    assert_eq!(wt.first_use, Some(days_ago(9)));
    assert_eq!(wt.last_use, Some(days_ago(2)));
    assert_eq!((wt.typed, wt.uses), (1, 3));
}

/// An alias and a function with one name are two entries.
#[test]
fn an_alias_and_a_function_with_one_name_are_two_entries() {
    let defs = definitions(&[("x", "echo")], &[], &[], &[("x", "\techo")]);
    let result = run(&[], &defs);
    let kinds: Vec<Kind> = result
        .definitions
        .iter()
        .filter(|u| u.name == "x")
        .map(|u| u.kind)
        .collect();
    assert_eq!(kinds, vec![Kind::Alias, Kind::Function]);
}

/// The most used entry is first. Equal uses sort by name.
#[test]
fn the_entries_sort_by_uses_then_name() {
    let defs = definitions(&[("b", "x"), ("a", "x"), ("c", "x")], &[], &[], &[]);
    let result = run(&[row("c"), row("c"), row("b"), row("a")], &defs);
    let names: Vec<&str> = result.definitions.iter().map(|u| u.name.as_str()).collect();
    assert_eq!(names, vec!["c", "a", "b"]);
}

/// The summary counts the rows and the definitions.
#[test]
fn the_summary_counts_rows_and_definitions() {
    let mut defs = definitions(
        &[("wt", "cwt")],
        &[("G", "| cat")],
        &[("md", "glow")],
        &[("ok", "\techo"), ("bad", "\techo $(")],
    );
    defs.unparsed_aliases.push("broken".to_string());
    let result = run(&[row("echo $("), row("bad"), row("wt")], &defs);
    let summary = &result.summary;
    assert_eq!(summary.rows, 3);
    assert_eq!(summary.unparsed_rows, 1);
    assert_eq!(summary.aliases, 3);
    assert_eq!(summary.functions, 2);
    assert_eq!(summary.unparsed_functions, 1);
    assert_eq!(summary.unparsed_aliases, 1);
    let bad = usage(&result, "bad", Kind::Function);
    assert_eq!((bad.uses, bad.typed), (1, 1));
}

/// The uses through other definitions sort by count, then by name.
#[test]
fn the_through_list_sorts_by_count_then_name() {
    let defs = definitions(
        &[("zz", "wt"), ("aa", "wt"), ("mm", "wt"), ("wt", "cwt")],
        &[],
        &[],
        &[],
    );
    let result = run(&[row("zz"), row("zz"), row("mm"), row("aa")], &defs);
    assert_eq!(
        usage(&result, "wt", Kind::Alias).through,
        through(&[("zz", 2), ("aa", 1), ("mm", 1)])
    );
}

/// A name and a value with multibyte characters stay whole.
#[test]
fn multibyte_names_and_values_stay_whole() {
    let defs = definitions(&[("日本", "echo 日本語 🎉")], &[], &[], &[]);
    let result = run(&[row("日本")], &defs);
    let alias = usage(&result, "日本", Kind::Alias);
    assert_eq!(alias.typed, 1);
    assert_eq!(alias.definition, "echo 日本語 🎉");
}
