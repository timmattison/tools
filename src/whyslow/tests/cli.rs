//! Tests that run the built `whyslow` binary.
//!
//! No test here reads this Mac. The one test that does is `tests/this-mac.rs`,
//! and it reads through the library.

use std::process::Output;

/// The path of the binary that cargo built for these tests.
const BIN: &str = env!("CARGO_BIN_EXE_whyslow");

/// The terminal width that each run states, so that the layout of the help
/// text does not change with the window of the person who runs the tests.
const COLUMNS: &str = "100";

/// The variable that makes `clap` paint its help and its errors on any output.
///
/// The pre-commit hook of this repository sets it for `cargo test`. Each run
/// states it too, so that every run reads the painted output that the hook
/// reads, whatever the environment of the person who runs the tests.
const CLICOLOR_FORCE: &str = "CLICOLOR_FORCE";

/// The value of [`CLICOLOR_FORCE`] that turns the paint on.
const PAINT: &str = "1";

/// The exit code that clap gives for a usage error.
const USAGE_ERROR: i32 = 2;

/// Runs the binary with `args` and returns what it wrote and how it ended.
fn run(args: &[&str]) -> Output {
    childenv::fixture_command(BIN)
        .args(args)
        .env("COLUMNS", COLUMNS)
        .env(CLICOLOR_FORCE, PAINT)
        .output()
        .expect("the whyslow binary starts")
}

/// Gives the glyphs of what the binary wrote, with the escape codes taken out.
///
/// `clap` paints flags and headings when `CLICOLOR_FORCE` is set, and [`run`]
/// sets it. See "Colored Output in Tests" in CLAUDE.md.
fn glyphs(bytes: &[u8]) -> String {
    testcolor::strip_ansi(&String::from_utf8_lossy(bytes))
}

/// Gives the glyphs of `bytes` with each run of white space made one space, so
/// that a line break in the help text does not split a phrase.
fn one_line(bytes: &[u8]) -> String {
    glyphs(bytes)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `--help` names the command, the flag, its default, and the rule for a bare
/// number.
#[test]
fn help_names_the_command_the_flag_and_the_default() {
    let output = run(&["--help"]);
    let stdout = one_line(&output.stdout);

    assert!(
        output.status.success(),
        "--help exits 0: {:?}",
        output.status
    );
    assert!(
        stdout.contains("Usage: whyslow"),
        "the usage names the command: {stdout:?}"
    );
    assert!(
        stdout.contains("--interval <DURATION>"),
        "the help names the flag and its value: {stdout:?}"
    );
    assert!(
        stdout.contains("[default: 1s]"),
        "the help states the default: {stdout:?}"
    );
    assert!(
        stdout.contains("A bare number is a number of seconds."),
        "the help states the rule for a bare number: {stdout:?}"
    );
}

/// `--version` gives the name, the package version, the git hash, and the
/// state of the tree, in the format that every tool of this repository uses.
#[test]
fn version_names_the_tool_the_release_and_the_build() {
    let output = run(&["--version"]);
    let stdout = glyphs(&output.stdout);

    assert!(
        output.status.success(),
        "--version exits 0: {:?}",
        output.status
    );
    let build = stdout
        .strip_prefix("whyslow 0.1.0 (")
        .and_then(|rest| rest.strip_suffix(")\n"))
        .unwrap_or_else(|| panic!("the version line has the form of the repository: {stdout:?}"));
    let (hash, state) = build
        .split_once(", ")
        .unwrap_or_else(|| panic!("the build facts hold a hash and a state: {build:?}"));
    assert!(
        hash == "unknown" || (hash.len() == 7 && hash.chars().all(|c| c.is_ascii_hexdigit())),
        "the hash is 7 hex digits or unknown: {hash:?}"
    );
    assert!(
        ["clean", "dirty", "unknown"].contains(&state),
        "the state is clean, dirty, or unknown: {state:?}"
    );
}

/// A bad interval is a usage error that names the flag and the bad text.
#[test]
fn a_bad_interval_is_a_usage_error_that_names_the_flag() {
    for text in ["0s", "5x", "-1"] {
        let output = run(&["--interval", text]);
        let stderr = one_line(&output.stderr);

        assert_eq!(
            output.status.code(),
            Some(USAGE_ERROR),
            "--interval {text} is a usage error: {stderr:?}"
        );
        assert!(
            stderr.contains("--interval"),
            "the message names the flag for {text:?}: {stderr:?}"
        );
        assert!(
            output.stdout.is_empty(),
            "a refused run prints nothing on the standard output"
        );
    }
}

/// An unknown flag is a usage error that names the flag.
#[test]
fn an_unknown_flag_is_a_usage_error() {
    let output = run(&["--shell-setup"]);
    let stderr = one_line(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(USAGE_ERROR),
        "an unknown flag is a usage error: {stderr:?}"
    );
    assert!(
        stderr.contains("--shell-setup"),
        "the message names the flag: {stderr:?}"
    );
}
