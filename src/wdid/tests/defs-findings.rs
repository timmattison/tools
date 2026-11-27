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

/// `"${@}"` is the same word as `"$@"`: both shells keep it as written, and it
/// passes on each argument. The function is thus the same command as the alias.
#[test]
fn an_alias_and_a_function_that_passes_on_its_arguments_with_braces_are_one_finding() {
    for body in ["\tgit log \"${@}\"", "{ \n    git log \"${@}\"\n}"] {
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

/// The second startup file of the tests.
const MORE: &str = "/home/me/more.zsh";

/// Gives the finding of one kind, which the test expects to be the only one.
fn only(found: &[Finding], kind: FindingKind) -> Option<&Finding> {
    let mut of_kind = found.iter().filter(|finding| finding.kind == kind);
    let first = of_kind.next();
    assert!(of_kind.next().is_none(), "more than one {kind:?}");
    first
}

/// Sets the places that defined the alias `name`.
fn set_places(defs: &mut Definitions, name: &str, places: &[Option<Origin>]) {
    defs.alias_places.insert(name.to_string(), places.to_vec());
}

/// An alias that two startup files define is one finding with each place.
#[test]
fn an_alias_defined_twice_is_one_finding_with_both_places() {
    let mut defs = empty();
    alias(&mut defs, "ll", "ls -l", MORE, 1);
    let places = [Some(origin(ZSHRC, Some(1))), Some(origin(MORE, Some(1)))];
    set_places(&mut defs, "ll", &places);
    let found = findings(&[row("ll")], &defs);
    assert_eq!(
        found,
        vec![Finding {
            kind: FindingKind::DefinedMoreThanOnce,
            definitions: vec![member(
                "ll",
                Kind::Alias,
                Some(origin(MORE, Some(1))),
                1,
                true
            )],
            words: Vec::new(),
            places: places.to_vec(),
            program: None,
        }]
    );
}

/// A place that a later place repeats is dropped, and the last one stays.
#[test]
fn a_repeated_place_keeps_its_last_occurrence() {
    let a = Some(origin(ZSHRC, Some(1)));
    let b = Some(origin(MORE, Some(1)));
    let mut defs = empty();
    alias(&mut defs, "ll", "ls -l", ZSHRC, 1);
    set_places(&mut defs, "ll", &[a.clone(), b.clone(), a.clone()]);
    let found = findings(&[], &defs);
    let finding = only(&found, FindingKind::DefinedMoreThanOnce).unwrap();
    assert_eq!(finding.places, vec![b, a]);
}

/// One place, and the same place twice, are not findings.
#[test]
fn one_place_is_not_a_finding() {
    let a = Some(origin(ZSHRC, Some(1)));
    for places in [vec![a.clone()], vec![a.clone(), a], Vec::new()] {
        let mut defs = empty();
        alias(&mut defs, "ll", "ls -l", ZSHRC, 1);
        set_places(&mut defs, "ll", &places);
        assert_eq!(findings(&[], &defs), Vec::new(), "places {places:?}");
    }
}

/// Two unknown places are not known to be one place, thus each one stays.
#[test]
fn each_unknown_place_stays() {
    let a = Some(origin(ZSHRC, Some(1)));
    for places in [vec![None, None], vec![None, a.clone(), None], vec![None, a]] {
        let mut defs = empty();
        alias(&mut defs, "ll", "ls -l", ZSHRC, 1);
        set_places(&mut defs, "ll", &places);
        let found = findings(&[], &defs);
        let finding = only(&found, FindingKind::DefinedMoreThanOnce);
        assert_eq!(
            finding.map(|f| &f.places),
            Some(&places),
            "places {places:?}"
        );
    }
}

/// A suffix alias with two places is a finding, and its name is its marker.
#[test]
fn a_suffix_alias_defined_twice_is_a_finding() {
    let mut defs = empty();
    defs.aliases.insert_suffix("md", "glow");
    defs.suffix_places.insert(
        "md".to_string(),
        vec![Some(origin(ZSHRC, Some(1))), Some(origin(MORE, Some(2)))],
    );
    let found = findings(&[], &defs);
    let finding = only(&found, FindingKind::DefinedMoreThanOnce).unwrap();
    assert_eq!(finding.definitions[0].name, "*.md");
    assert_eq!(finding.definitions[0].kind, Kind::SuffixAlias);
    assert_eq!(finding.places.len(), 2);
}

/// An alias and a function with the same name are one finding.
#[test]
fn an_alias_that_hides_a_function_is_one_finding() {
    let mut defs = empty();
    alias(&mut defs, "wdx", "echo x", ZSHRC, 7);
    function(&mut defs, "wdx", "\techo y", GIT_PLUGIN);
    let found = findings(&[row("wdx")], &defs);
    assert_eq!(
        found,
        vec![Finding {
            kind: FindingKind::AliasHidesFunction,
            definitions: vec![
                member("wdx", Kind::Alias, Some(origin(ZSHRC, Some(7))), 1, true),
                member(
                    "wdx",
                    Kind::Function,
                    Some(origin(GIT_PLUGIN, None)),
                    0,
                    false
                ),
            ],
            words: Vec::new(),
            places: Vec::new(),
            program: None,
        }]
    );
}

/// A global alias that hides a function is a finding too.
#[test]
fn a_global_alias_that_hides_a_function_is_a_finding() {
    let mut defs = empty();
    defs.aliases.insert_global("wdg", "echo x");
    function(&mut defs, "wdg", "\techo y", ZSHRC);
    let found = findings(&[], &defs);
    let finding = only(&found, FindingKind::AliasHidesFunction).unwrap();
    assert_eq!(finding.definitions[0].kind, Kind::GlobalAlias);
}

/// An alias that starts with its own name runs the function, thus it hides
/// nothing.
#[test]
fn an_alias_that_wraps_its_own_function_is_not_a_finding() {
    let mut defs = empty();
    alias(&mut defs, "wdy", "wdy --color", ZSHRC, 8);
    function(&mut defs, "wdy", "\techo y", ZSHRC);
    assert_eq!(findings(&[], &defs), Vec::new());
}

/// Says that the shell finds the program `path` for `name`.
fn program(defs: &mut Definitions, name: &str, path: &str) {
    defs.programs.insert(name.to_string(), PathBuf::from(path));
}

/// An alias that has the name of a program is one finding with the program.
#[test]
fn an_alias_that_hides_a_program_is_one_finding() {
    let mut defs = empty();
    alias(&mut defs, "hidden", "echo hi", ZSHRC, 9);
    program(&mut defs, "hidden", "/usr/bin/hidden");
    let found = findings(&[row("hidden")], &defs);
    assert_eq!(
        found,
        vec![Finding {
            kind: FindingKind::HidesProgram,
            definitions: vec![member(
                "hidden",
                Kind::Alias,
                Some(origin(ZSHRC, Some(9))),
                1,
                true
            )],
            words: Vec::new(),
            places: Vec::new(),
            program: Some(PathBuf::from("/usr/bin/hidden")),
        }]
    );
}

/// A function that has the name of a program is one finding.
#[test]
fn a_function_that_hides_a_program_is_one_finding() {
    let mut defs = empty();
    function(&mut defs, "wdcat", "\tbat \"$@\"", ZSHRC);
    program(&mut defs, "wdcat", "/bin/wdcat");
    let found = findings(&[], &defs);
    let finding = only(&found, FindingKind::HidesProgram).unwrap();
    assert_eq!(finding.definitions[0].kind, Kind::Function);
    assert_eq!(finding.program, Some(PathBuf::from("/bin/wdcat")));
}

/// A global alias that has the name of a program is a finding.
#[test]
fn a_global_alias_that_hides_a_program_is_a_finding() {
    let mut defs = empty();
    defs.aliases.insert_global("wdglobal", "echo x");
    program(&mut defs, "wdglobal", "/bin/wdglobal");
    let found = findings(&[], &defs);
    let finding = only(&found, FindingKind::HidesProgram).unwrap();
    assert_eq!(finding.definitions[0].kind, Kind::GlobalAlias);
}

/// An alias that starts with its own name, and an alias or a function that
/// runs the program through `command`, are wrappers and not findings.
#[test]
fn a_wrapper_of_a_program_is_not_a_finding() {
    let mut defs = empty();
    alias(&mut defs, "ls", "ls -G", ZSHRC, 1);
    program(&mut defs, "ls", "/bin/ls");
    function(
        &mut defs,
        "grep",
        "\tcommand grep --color=auto \"$@\"",
        ZSHRC,
    );
    program(&mut defs, "grep", "/usr/bin/grep");
    alias(&mut defs, "rg", "command rg --smart-case", ZSHRC, 2);
    program(&mut defs, "rg", "/usr/bin/rg");
    assert_eq!(findings(&[], &defs), Vec::new());
}

/// An alias that runs its own name through `command` skips a function of
/// that name, thus it still hides the function.
#[test]
fn an_alias_that_runs_its_name_through_command_hides_the_function() {
    let mut defs = empty();
    alias(&mut defs, "wdz", "command wdz", ZSHRC, 1);
    function(&mut defs, "wdz", "\techo z", ZSHRC);
    assert_eq!(
        kinds(&findings(&[], &defs)),
        [FindingKind::AliasHidesFunction]
    );
}

/// A function that runs a different program through `command` still hides
/// the program of its name.
#[test]
fn a_function_that_runs_another_program_through_command_is_a_finding() {
    let mut defs = empty();
    function(&mut defs, "wdcat", "\tcommand bat \"$@\"", ZSHRC);
    program(&mut defs, "wdcat", "/bin/wdcat");
    assert_eq!(kinds(&findings(&[], &defs)), [FindingKind::HidesProgram]);
}

/// A private function takes part only when the report lists it.
#[test]
fn a_private_function_takes_part_only_with_all() {
    let mut defs = empty();
    function(&mut defs, "_wdpriv", "\techo y", ZSHRC);
    program(&mut defs, "_wdpriv", "/bin/_wdpriv");
    assert_eq!(findings(&[], &defs), Vec::new());
    let with_all = tally(&[], &defs, &window(true)).findings;
    assert_eq!(kinds(&with_all), [FindingKind::HidesProgram]);
}

/// A finding is mine when one of its definitions is mine.
#[test]
fn a_finding_is_mine_when_one_definition_is_mine() {
    let mut defs = empty();
    alias(&mut defs, "gs", "git status", ZSHRC, 1);
    alias(&mut defs, "gst", "git status", GIT_PLUGIN, 1);
    alias(&mut defs, "ga", "git add", GIT_PLUGIN, 2);
    alias(&mut defs, "gad", "git add", GIT_PLUGIN, 3);
    let found = findings(&[], &defs);
    assert_eq!(found.len(), 2);
    let mine: Vec<(&str, bool)> = found
        .iter()
        .map(|finding| {
            (
                finding.definitions[0].name.as_str(),
                finding.mine(&places()),
            )
        })
        .collect();
    assert_eq!(mine, [("ga", false), ("gs", true)]);
}

/// An alias of my startup file that a plugin defines again is a mine finding,
/// although the member of the definition in effect is not mine.
#[test]
fn a_finding_is_mine_when_a_place_before_the_last_is_mine() {
    let mut defs = empty();
    alias(&mut defs, "ll", "ls -l", GIT_PLUGIN, 30);
    set_places(
        &mut defs,
        "ll",
        &[
            Some(origin(ZSHRC, Some(12))),
            Some(origin(GIT_PLUGIN, Some(30))),
        ],
    );
    let result = tally(&[], &defs, &window(false));
    assert_eq!(result.findings.len(), 1, "{:?}", result.findings);
    let found = &result.findings[0];
    assert_eq!(found.kind, FindingKind::DefinedMoreThanOnce);
    assert!(!found.definitions[0].mine);
    let document: serde_json::Value = serde_json::from_str(&wdid::defs::render::json(
        &result,
        &window(false),
        "/bin/zsh",
    ))
    .unwrap();
    assert_eq!(document["findings"][0]["mine"], true, "{document}");
}

/// The findings are sorted by kind, then by the name of the first
/// definition, then by its kind.
#[test]
fn the_findings_are_sorted_by_kind_then_name() {
    let mut defs = empty();
    alias(&mut defs, "hp", "echo hi", ZSHRC, 1);
    program(&mut defs, "hp", "/bin/hp");
    function(&mut defs, "fz", "\techo y", ZSHRC);
    program(&mut defs, "fz", "/bin/fz");
    alias(&mut defs, "fz", "echo z", ZSHRC, 2);
    alias(&mut defs, "dd", "ls -l", ZSHRC, 3);
    set_places(
        &mut defs,
        "dd",
        &[Some(origin(ZSHRC, Some(3))), Some(origin(MORE, Some(1)))],
    );
    alias(&mut defs, "s2", "wc -l", ZSHRC, 4);
    alias(&mut defs, "s1", "wc -l", ZSHRC, 5);
    alias(&mut defs, "gl", "git log", ZSHRC, 6);
    function(&mut defs, "glog", "\tgit log \"$@\"", ZSHRC);
    let found = findings(&[], &defs);
    let order: Vec<(FindingKind, &str, Kind)> = found
        .iter()
        .map(|finding| {
            let first = &finding.definitions[0];
            (finding.kind, first.name.as_str(), first.kind)
        })
        .collect();
    assert_eq!(
        order,
        [
            (FindingKind::SameExpansion, "s1", Kind::Alias),
            (FindingKind::AliasAndFunction, "gl", Kind::Alias),
            (FindingKind::DefinedMoreThanOnce, "dd", Kind::Alias),
            (FindingKind::AliasHidesFunction, "fz", Kind::Alias),
            (FindingKind::HidesProgram, "fz", Kind::Alias),
            (FindingKind::HidesProgram, "fz", Kind::Function),
            (FindingKind::HidesProgram, "hp", Kind::Alias),
        ]
    );
}

/// A quoted literal and an expansion with the same text are not the same
/// word, and neither are a quoted literal and an operator.
#[test]
fn a_quoted_literal_and_an_expansion_are_different_words() {
    for (a, b) in [
        ("echo $HOME", "echo '$HOME'"),
        ("ls ~", "ls '~'"),
        ("ls | less", "ls '|' less"),
    ] {
        let mut defs = empty();
        alias(&mut defs, "a", a, ZSHRC, 1);
        alias(&mut defs, "b", b, ZSHRC, 2);
        assert_eq!(findings(&[], &defs), Vec::new(), "values {a:?} and {b:?}");
    }
}

/// A function whose last word is the quoted text "$@" passes on no
/// arguments, thus it does not do the same thing as the alias.
#[test]
fn a_function_that_ends_with_the_quoted_text_of_all_arguments_is_not_a_finding() {
    let mut defs = empty();
    alias(&mut defs, "gl", "git log", ZSHRC, 2);
    function(&mut defs, "glog", "\tgit log '\"$@\"'", ZSHRC);
    assert_eq!(kinds(&findings(&[], &defs)), Vec::new());
}

/// A word that holds a blank shows in quotes, so the words `echo`, `x y` do
/// not read as the words `echo`, `x`, `y`.
#[test]
fn a_word_that_holds_a_blank_shows_in_quotes() {
    let mut defs = empty();
    alias(&mut defs, "a", "echo \"x y\"", ZSHRC, 1);
    alias(&mut defs, "b", "echo 'x y'", ZSHRC, 2);
    alias(&mut defs, "c", "echo x y", ZSHRC, 3);
    let found = findings(&[], &defs);
    let shared = only(&found, FindingKind::SameExpansion).expect("a same-expansion finding");
    let names: Vec<&str> = shared
        .definitions
        .iter()
        .map(|member| member.name.as_str())
        .collect();
    assert_eq!(names, ["a", "b"]);
    assert_eq!(shared.words, ["echo", "'x y'"]);
    let report = wdid::defs::render::text(
        &tally(&[], &defs, &window(false)),
        &window(false),
        "/bin/zsh",
        None,
    );
    assert!(
        report.contains("Same expansion (echo 'x y'): a (alias"),
        "{report}"
    );
}
