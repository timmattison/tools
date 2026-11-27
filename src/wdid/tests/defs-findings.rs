//! Tests of the duplicates and the shadows among the definitions.

use std::path::PathBuf;

use chrono::{NaiveDate, NaiveDateTime};
use wdid::command_words::{command_words, Aliases, Parsed};
use wdid::defs::{tally, Finding, FindingKind, Kind, Member, Tally, Window};
use wdid::history::{Exit, HistoryEntry};
use wdid::origin::{Origin, Places};
use wdid::shell::Definitions;

/// The file of the git plugin of the oh-my-zsh of [`places`].
const GIT_PLUGIN: &str = "/home/me/omz/plugins/git/git.plugin.zsh";
/// The file of my startup file.
const ZSHRC: &str = "/home/me/.zshrc";

/// Makes the directories of the tests.
fn places() -> Places {
    Places {
        home: Some(PathBuf::from("/home/me")),
        zsh: Some(PathBuf::from("/home/me/omz")),
        zsh_custom: None,
    }
}

/// Makes an origin in `file`, at `line` when it is given.
fn origin(file: &str, line: Option<u32>) -> Origin {
    Origin {
        file: PathBuf::from(file),
        line,
    }
}

/// Makes a window for the date of the tests.
fn window(all: bool) -> Window {
    Window {
        today: NaiveDate::from_ymd_opt(2026, 10, 14).unwrap(),
        since: None,
        idle_days: 30,
        all,
    }
}

/// Makes a row that succeeded on the day before the date of the tests.
fn row(command: &str) -> HistoryEntry {
    HistoryEntry {
        time: NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2026, 10, 13).unwrap(),
            chrono::NaiveTime::from_hms_opt(12, 30, 0).unwrap(),
        ),
        exit: Exit::new(0),
        host: "host".to_string(),
        user: "user".to_string(),
        command: command.to_string(),
    }
}

/// Makes empty definitions with the directories of the tests.
fn empty() -> Definitions {
    Definitions {
        places: places(),
        ..Definitions::default()
    }
}

/// Adds the regular alias `name` with `value`, defined at `file:line`.
fn alias(defs: &mut Definitions, name: &str, value: &str, file: &str, line: u32) {
    defs.aliases.insert(name, value);
    defs.alias_origins
        .insert(name.to_string(), origin(file, Some(line)));
}

/// Adds the function `name` with `body`, defined at `file`.
fn function(defs: &mut Definitions, name: &str, body: &str, file: &str) {
    defs.function_bodies
        .insert(name.to_string(), body.to_string());
    let line = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    defs.function_lines
        .insert(name.to_string(), line.trim().to_string());
    match command_words(body, &Aliases::new()) {
        Parsed::Words(found) => {
            defs.functions.insert(name.to_string(), found);
        }
        Parsed::Unparsed => defs.unparsed_functions.push(name.to_string()),
    }
    defs.function_origins
        .insert(name.to_string(), origin(file, None));
}

/// Runs the count and gives the findings.
fn findings(rows: &[HistoryEntry], defs: &Definitions) -> Vec<Finding> {
    let result: Tally = tally(rows, defs, &window(false));
    result.findings
}

/// Makes the expected member.
fn member(name: &str, kind: Kind, origin: Option<Origin>, uses: u64, mine: bool) -> Member {
    Member {
        name: name.to_string(),
        kind,
        origin,
        uses,
        mine,
    }
}

/// Makes a list of strings.
fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

/// Two aliases with the same words are one finding, whatever their blanks.
#[test]
fn two_aliases_with_the_same_words_are_one_finding() {
    let mut defs = empty();
    alias(&mut defs, "gs", "git status", ZSHRC, 3);
    alias(&mut defs, "gst", "git  status", GIT_PLUGIN, 1);
    let found = findings(&[row("gs"), row("gs"), row("gst")], &defs);
    assert_eq!(
        found,
        vec![Finding {
            kind: FindingKind::SameExpansion,
            definitions: vec![
                member("gs", Kind::Alias, Some(origin(ZSHRC, Some(3))), 2, true),
                member(
                    "gst",
                    Kind::Alias,
                    Some(origin(GIT_PLUGIN, Some(1))),
                    1,
                    false
                ),
            ],
            words: words(&["git", "status"]),
            places: Vec::new(),
            program: None,
        }]
    );
}

/// Different words, a suffix alias, and a global alias are not duplicates.
#[test]
fn different_words_suffix_and_global_aliases_are_not_duplicates() {
    let mut defs = empty();
    alias(&mut defs, "a", "ls | less", ZSHRC, 1);
    alias(&mut defs, "b", "ls less", ZSHRC, 2);
    alias(&mut defs, "g", "glow", ZSHRC, 3);
    defs.aliases.insert_suffix("md", "glow");
    defs.aliases.insert_global("GG", "glow");
    assert_eq!(findings(&[], &defs), Vec::new());
}

/// A regular alias and a global alias with the same value are not duplicates.
#[test]
fn a_regular_and_a_global_alias_are_not_duplicates() {
    let mut defs = empty();
    alias(&mut defs, "x", "wc -l", ZSHRC, 1);
    defs.aliases.insert_global("L", "wc -l");
    assert_eq!(findings(&[], &defs), Vec::new());
}

/// Two global aliases with the same value are one finding, sorted by name.
#[test]
fn two_global_aliases_with_one_value_are_one_finding() {
    let mut defs = empty();
    defs.aliases.insert_global("W2", "wc -l");
    defs.aliases.insert_global("W1", "wc -l");
    let found = findings(&[], &defs);
    assert_eq!(found.len(), 1);
    let names: Vec<&str> = found[0]
        .definitions
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(names, ["W1", "W2"]);
    assert!(found[0]
        .definitions
        .iter()
        .all(|m| m.kind == Kind::GlobalAlias));
}

/// Gives the kinds of `found`.
fn kinds(found: &[Finding]) -> Vec<FindingKind> {
    found.iter().map(|finding| finding.kind).collect()
}

/// An alias and a function that does the same thing are one finding, for the
/// zsh body and for the bash body.
#[test]
fn an_alias_and_a_function_that_do_the_same_thing_are_one_finding() {
    for body in ["\tgit log \"$@\"", "{ \n    git log \"$@\"\n}"] {
        let mut defs = empty();
        alias(&mut defs, "gl", "git log", ZSHRC, 2);
        function(&mut defs, "glog", body, GIT_PLUGIN);
        let found = findings(&[row("gl"), row("glog"), row("glog")], &defs);
        assert_eq!(
            found,
            vec![Finding {
                kind: FindingKind::AliasAndFunction,
                definitions: vec![
                    member("gl", Kind::Alias, Some(origin(ZSHRC, Some(2))), 1, true),
                    member(
                        "glog",
                        Kind::Function,
                        Some(origin(GIT_PLUGIN, None)),
                        2,
                        false
                    ),
                ],
                words: words(&["git", "log"]),
                places: Vec::new(),
                program: None,
            }],
            "body {body:?}"
        );
    }
}

/// A function that does something else than the alias is not a finding.
#[test]
fn a_function_that_does_something_else_is_not_a_finding() {
    for body in [
        "\tgit log",
        "\tgit log \"$@\" | cat",
        "\tgit log --oneline \"$@\"",
    ] {
        let mut defs = empty();
        alias(&mut defs, "gl", "git log", ZSHRC, 2);
        function(&mut defs, "glog", body, ZSHRC);
        assert_eq!(kinds(&findings(&[], &defs)), Vec::new(), "body {body:?}");
    }
}

/// Each alias that matches the function is in the finding, sorted by name,
/// and a global alias matches too.
#[test]
fn each_matching_alias_is_in_the_finding() {
    let mut defs = empty();
    alias(&mut defs, "z", "git log", ZSHRC, 1);
    defs.aliases.insert_global("A", "git log");
    alias(&mut defs, "other", "git show", ZSHRC, 2);
    function(&mut defs, "glog", "\tgit log \"$@\"", ZSHRC);
    let found = findings(&[], &defs);
    let alias_and_function: Vec<&Finding> = found
        .iter()
        .filter(|finding| finding.kind == FindingKind::AliasAndFunction)
        .collect();
    assert_eq!(alias_and_function.len(), 1);
    let names: Vec<&str> = alias_and_function[0]
        .definitions
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(names, ["A", "z", "glog"]);
}

/// A function with no body, and a suffix alias, take no part.
#[test]
fn a_function_with_no_body_takes_no_part() {
    let mut defs = empty();
    alias(&mut defs, "gl", "git log", ZSHRC, 2);
    defs.functions.insert("glog".to_string(), Vec::new());
    assert_eq!(findings(&[], &defs), Vec::new());
}
