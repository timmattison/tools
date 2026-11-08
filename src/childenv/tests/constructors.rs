//! What each constructor of `childenv` does to the real environment.
//!
//! Each test proves a constructor end to end. A grandchild process prints its
//! environment, and the test reads what survived.
//!
//! No test here sets a variable in its own process. Cargo runs the tests of
//! one binary on many threads, so a `set_var` would change what a sibling test
//! sees. Each test starts the test binary again instead. The parent test sets
//! the hostile variables on that child only. The child test finds the marker
//! variable, starts `/usr/bin/env` through the constructor under test, and
//! checks the lines that `env` prints.
#![cfg(unix)]
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::process::Command;

use childenv::{
    command, fixture_command, inherited_git_variables, InheritedGitEnvironment,
    USER_INTENT_GIT_ENVIRONMENT,
};

/// The variable that tells a test it runs as the child.
const MARKER: &str = "CHILDENV_CONSTRUCTORS_CHILD";

/// The program that prints the environment.
const ENV_PROGRAM: &str = "/usr/bin/env";

/// The hostile variables. The first six are hook exports or names that git
/// invents later. The next two are user intent. The last is not a git name.
const HOSTILE: &[(&str, &str)] = &[
    ("GIT_DIR", "/nonexistent/decoy/.git"),
    ("GIT_INDEX_FILE", "/nonexistent/decoy/.git/index"),
    ("GIT_OBJECT_DIRECTORY", "/nonexistent/decoy/.git/objects"),
    ("GIT_CONFIG_PARAMETERS", "'core.bare=true'"),
    ("GIT_AUTHOR_NAME", "Hook Author"),
    ("GIT_SOMETHING_GIT_INVENTS_LATER", "later"),
    ("GIT_SSH_COMMAND", "ssh -i /nonexistent/key"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GITHUB_TOKEN", "token-value"),
];

/// Names that a hook exports or that git invents later. `command` sheds them.
const SHED_BY_COMMAND: &[&str] = &[
    "GIT_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_CONFIG_PARAMETERS",
    "GIT_AUTHOR_NAME",
    "GIT_SOMETHING_GIT_INVENTS_LATER",
];

/// Returns `true` when this process is the child and the test body must run.
///
/// In the parent, this function starts the test binary again with the hostile
/// variables, asserts that the child passed, and returns `false`.
fn in_hostile_child(test_name: &str) -> bool {
    if std::env::var_os(MARKER).is_some() {
        return true;
    }

    let exe = std::env::current_exe().expect("the path of the test binary");
    let mut child = Command::new(exe);
    child
        .args(["--exact", test_name, "--nocapture"])
        .env(MARKER, "1");
    for (name, value) in HOSTILE {
        child.env(name, value);
    }
    let output = child.output().expect("start the test binary again");

    assert!(
        output.status.success(),
        "the child test {test_name} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    false
}

/// Run `command` and read the `NAME=value` lines that it printed.
fn printed_environment(mut command: Command) -> BTreeMap<String, String> {
    let output = command.output().expect("start the environment printer");
    assert!(output.status.success(), "the environment printer failed");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn command_sheds_hook_names_and_keeps_user_intent() {
    if !in_hostile_child("command_sheds_hook_names_and_keeps_user_intent") {
        return;
    }

    let seen = printed_environment(command(ENV_PROGRAM));

    for name in SHED_BY_COMMAND {
        assert!(!seen.contains_key(*name), "{name} reached the grandchild");
    }
    for name in seen.keys().filter(|name| name.starts_with("GIT_")) {
        assert!(
            USER_INTENT_GIT_ENVIRONMENT.contains(&name.as_str()),
            "{name} is not a user intent name and reached the grandchild"
        );
    }
    assert_eq!(
        seen.get("GIT_SSH_COMMAND").map(String::as_str),
        Some("ssh -i /nonexistent/key")
    );
    assert_eq!(
        seen.get("GIT_TERMINAL_PROMPT").map(String::as_str),
        Some("0")
    );
    assert_eq!(
        seen.get("GITHUB_TOKEN").map(String::as_str),
        Some("token-value")
    );
}

#[test]
fn fixture_command_sheds_every_git_name_and_keeps_the_rest() {
    if !in_hostile_child("fixture_command_sheds_every_git_name_and_keeps_the_rest") {
        return;
    }

    let seen = printed_environment(fixture_command(ENV_PROGRAM));

    let leaked: Vec<&String> = seen
        .keys()
        .filter(|name| name.starts_with("GIT_"))
        .collect();
    assert!(
        leaked.is_empty(),
        "GIT_ names reached the grandchild: {leaked:?}"
    );
    assert_eq!(
        seen.get("GITHUB_TOKEN").map(String::as_str),
        Some("token-value")
    );
}

#[test]
fn inherited_git_variables_lists_what_each_rule_sheds() {
    if !in_hostile_child("inherited_git_variables_lists_what_each_rule_sheds") {
        return;
    }

    let keep = inherited_git_variables(InheritedGitEnvironment::KeepUserIntent);
    let everything = inherited_git_variables(InheritedGitEnvironment::ShedEverything);
    let has = |list: &[OsString], name: &str| list.iter().any(|item| item == name);

    assert!(has(&keep, "GIT_DIR"));
    assert!(has(&keep, "GIT_SOMETHING_GIT_INVENTS_LATER"));
    assert!(!has(&keep, "GIT_SSH_COMMAND"));
    assert!(!has(&keep, "GIT_TERMINAL_PROMPT"));
    assert!(has(&everything, "GIT_DIR"));
    assert!(has(&everything, "GIT_SSH_COMMAND"));
    assert!(has(&everything, "GIT_TERMINAL_PROMPT"));
    assert!(!has(&keep, "GITHUB_TOKEN"));
    assert!(!has(&everything, "GITHUB_TOKEN"));
}

#[test]
fn a_value_pinned_after_the_constructor_wins() {
    if !in_hostile_child("a_value_pinned_after_the_constructor_wins") {
        return;
    }

    let mut pinned = fixture_command(ENV_PROGRAM);
    pinned.env("GIT_DIR", "pinned");
    let seen = printed_environment(pinned);

    assert_eq!(seen.get("GIT_DIR").map(String::as_str), Some("pinned"));
}
