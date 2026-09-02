//! Black-box tests of the `subito` binary.
//!
//! Each test runs the binary that the workspace built, and reads back what it
//! printed and the status it left.
//!
//! No test gives a topic. A run with a topic reads the AWS configuration
//! chain, which reaches the network and can reach an account, and no test of
//! this repository reaches AWS. The paths a test drives are the ones that stop
//! before the tool loads that configuration: the version, the help, a bad
//! quality of service, and a command line that names no topic.
//!
//! Every child process starts from an environment this file scrubbed. A
//! leaked `AWS_PROFILE`, `AWS_ACCESS_KEY_ID` or `AWS_REGION` points a run at a
//! live account, and a leaked `GIT_DIR` or `GIT_INDEX_FILE` points a child at
//! a repository that is not this one. [`subito`] is the one function that
//! builds the command, and every test goes through it, so no test can forget
//! the scrub.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "every unwrap in this file is an assertion about the harness: `.output()` starts the binary the workspace just built, and a failure to start it is a broken harness and not behavior under test, so it must stop the test where it happens. The failures of the tool itself are never unwrapped: each test reads the exit status and the two streams and asserts on them. src/main.rs raises both lints, so this root states its position on both. No `.expect` appears here yet; the allow covers it, so the first one does not silently change the position of this file"
)]

use std::ffi::OsString;
use std::process::Command;

/// The prefixes of every environment variable that a child of this file loses.
///
/// The rule is a shape and not a list of names. A list goes stale, and a stale
/// list reports a clean environment for the one variable it never learned
/// about. `AWS_` covers the region, the profile, the keys, the session token
/// and the paths of the configuration files of the AWS SDK. `GIT_` covers the
/// variables that a git hook exports, which point git at the repository the
/// hook runs in, whatever directory a child starts from.
const SCRUBBED_PREFIXES: [&str; 2] = ["AWS_", "GIT_"];

/// The name of the tool, as the version line and the help start.
const TOOL_NAME: &str = "subito";

/// The count of hex digits of the short commit hash of a version line.
const SHORT_HASH_LENGTH: usize = 7;

/// The word a version line carries when the build holds no local change.
const CLEAN_STATE: &str = "clean";

/// The word a version line carries when the build holds a local change.
const DIRTY_STATE: &str = "dirty";

/// Builds a command that runs the binary with a scrubbed environment.
///
/// This is the one entrance of this file to the binary. Every test goes
/// through it, so no call site can forget to take the AWS and git variables
/// out of the environment of the child.
fn subito() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_subito"));

    for name in scrubbed_names() {
        command.env_remove(name);
    }

    command
}

/// Gives the name of every variable of this process that a child must lose.
///
/// The name of a variable is not always UTF-8, so the filter reads a lossy
/// copy and the answer keeps the name as the operating system holds it. A
/// lossy copy replaces each bad byte with a character that is not ASCII, so no
/// name gains a prefix of [`SCRUBBED_PREFIXES`] that it did not have.
fn scrubbed_names() -> Vec<OsString> {
    std::env::vars_os()
        .map(|(name, _value)| name)
        .filter(|name| {
            let text = name.to_string_lossy().to_ascii_uppercase();
            SCRUBBED_PREFIXES
                .iter()
                .any(|prefix| text.starts_with(prefix))
        })
        .collect()
}

/// Runs the binary with `arguments` and gives back what it did.
///
/// The answer holds the exit status, the standard output and the standard
/// error, in that order. The status is `None` when a signal ended the process.
fn run(arguments: &[&str]) -> (Option<i32>, String, String) {
    let output = subito().args(arguments).output().unwrap();

    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Reads the shape of a version line, and says what is wrong with it.
///
/// The line is `subito <version> (<hash>, <state>)`. The hash changes with
/// every commit, so this function reads the shape and never a literal. A test
/// that held the literal would fail on the next commit, and a test that only
/// looked for the name of the tool would pass for a line that names no build
/// at all.
fn check_version_line(line: &str) -> Result<(), String> {
    let prefix = format!("{TOOL_NAME} ");
    let rest = line
        .strip_prefix(&prefix)
        .ok_or_else(|| format!("the line does not start with {prefix:?}"))?;
    let (version, build) = rest
        .split_once(' ')
        .ok_or_else(|| "the line names no build after the version".to_string())?;
    let inside = build
        .strip_prefix('(')
        .and_then(|text| text.strip_suffix(')'))
        .ok_or_else(|| format!("the build {build:?} is not inside parentheses"))?;
    let (hash, state) = inside
        .split_once(", ")
        .ok_or_else(|| format!("the build {inside:?} names no hash and no state"))?;

    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.chars().all(|digit| digit.is_ascii_digit()))
    {
        return Err(format!("the version {version:?} is not three numbers"));
    }

    if hash.len() != SHORT_HASH_LENGTH
        || !hash
            .chars()
            .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase())
    {
        return Err(format!(
            "the hash {hash:?} is not {SHORT_HASH_LENGTH} lowercase hex digits"
        ));
    }

    if state != CLEAN_STATE && state != DIRTY_STATE {
        return Err(format!(
            "the state {state:?} is not {CLEAN_STATE:?} and not {DIRTY_STATE:?}"
        ));
    }

    Ok(())
}

#[test]
fn the_version_flag_names_the_tool_the_version_and_the_build() {
    let (status, printed, complained) = run(&["--version"]);

    assert_eq!(
        status,
        Some(0),
        "--version stops with success: {complained}"
    );

    let line = printed.lines().next().unwrap_or_default();
    if let Err(reason) = check_version_line(line) {
        panic!("the version line {line:?} is wrong: {reason}");
    }
}

#[test]
fn the_help_names_every_option_of_the_tool() {
    let (status, printed, complained) = run(&["--help"]);

    assert_eq!(status, Some(0), "--help stops with success: {complained}");

    for option in ["--qos", "--endpoint", "--json"] {
        assert!(
            printed.contains(option),
            "the help names {option}: {printed}"
        );
    }
}

#[test]
fn a_quality_of_service_of_three_is_refused() {
    let (status, printed, complained) = run(&["--qos", "3", "sensors/#"]);

    assert_ne!(
        status,
        Some(0),
        "a quality of service of 3 stops the tool: {printed}"
    );
    assert!(
        complained.contains("--qos") && complained.contains('3'),
        "the tool names the flag and the value it refused: {complained}"
    );
}
