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

/// Printed by a child that ran its body to the end, and required by the parent.
///
/// libtest exits zero when `--exact` matches no test. `hostile_test!` takes the
/// name from the function, but it cannot see a module around that function, and
/// the path of a test in a module is longer than its name. Only a child that
/// reached the end of its body prints this line.
const BODY_RAN: &str = "CHILDENV_CONSTRUCTORS_BODY_RAN";

/// Run `body` in a child process that holds the hostile variables.
///
/// In the child, this function runs `body` and prints [`BODY_RAN`]. In the
/// parent, it starts the test binary again with the hostile variables and
/// asserts that the child passed and printed that line.
fn in_hostile_child(test_name: &str, body: impl FnOnce()) {
    if std::env::var_os(MARKER).is_some() {
        body();
        println!("{BODY_RAN}");
        return;
    }

    let exe = std::env::current_exe().expect("the path of the test binary");
    let mut child = childenv::fixture_command(exe);
    child
        .args(["--exact", test_name, "--nocapture"])
        .env(MARKER, "1");
    for (name, value) in HOSTILE {
        child.env(name, value);
    }
    let output = child.output().expect("start the test binary again");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "the child test {test_name} failed:\n{stdout}\n{stderr}",
    );
    assert!(
        stdout.contains(BODY_RAN),
        "the child test {test_name} ran no body, because `--exact` matched no test:\n{stdout}\n{stderr}",
    );
}

/// Define a test whose body runs in a child process that holds the hostile
/// variables.
///
/// One token names the function and the test that the child runs. A test thus
/// cannot start the body of a different test.
macro_rules! hostile_test {
    ($name:ident, $body:block) => {
        #[test]
        fn $name() {
            in_hostile_child(stringify!($name), || $body);
        }
    };
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

hostile_test!(command_sheds_hook_names_and_keeps_user_intent, {
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
});

hostile_test!(fixture_command_sheds_every_git_name_and_keeps_the_rest, {
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
});

hostile_test!(inherited_git_variables_lists_what_each_rule_sheds, {
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
});

hostile_test!(a_value_pinned_after_the_constructor_wins, {
    let mut pinned = fixture_command(ENV_PROGRAM);
    pinned.env("GIT_DIR", "pinned");
    let seen = printed_environment(pinned);

    assert_eq!(seen.get("GIT_DIR").map(String::as_str), Some("pinned"));
});
