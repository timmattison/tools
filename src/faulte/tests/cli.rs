//! Tests that run the built `faulte` binary.

use std::process::{Command, Output};

/// The path of the binary that cargo built for these tests.
const BIN: &str = env!("CARGO_BIN_EXE_faulte");

/// The terminal width that each run states, so that a layout of the help text
/// does not change with the window of the person who runs the tests.
const COLUMNS: &str = "100";

/// The exit code that clap gives for a usage error.
const USAGE_ERROR: i32 = 2;

/// Runs the binary with `args` and returns what it wrote and how it ended.
fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("COLUMNS", COLUMNS)
        .output()
        .expect("the faulte binary starts")
}

/// Gives `text` with each run of white space made one space, so that a line
/// break in the help text does not split a phrase that a test looks for.
fn one_line(text: &[u8]) -> String {
    String::from_utf8_lossy(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `--version` gives the name, the package version, the git hash, and the
/// state of the tree, in the format that every tool of this repository uses.
#[test]
fn version_names_the_tool_the_release_and_the_build() {
    let output = run(&["--version"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "faulte --version must exit 0, it ended with {:?} and wrote {stderr:?}",
        output.status
    );
    assert!(
        stdout.starts_with("faulte 0.1.0 ("),
        "the version line must name the tool and the release: {stdout:?}"
    );
    assert!(
        stdout.ends_with(")\n"),
        "the version line must end with the build facts in parentheses: {stdout:?}"
    );
}

/// Each flag that takes a duration reads it with the parser of the library.
/// A bad duration is a usage error that names the flag and the bad text, and
/// no command starts.
#[test]
fn a_bad_duration_in_any_flag_is_a_usage_error_that_names_the_text() {
    let cases: [(&[&str], &str, &str); 6] = [
        (&["--interval", "0s"], "--interval", "0s"),
        (&["--interval", "5x"], "--interval", "5x"),
        (&["kill", "--interval", "0"], "--interval", "0"),
        (&["kill", "--older-than=-7d"], "--older-than", "-7d"),
        (&["kill", "--older-than", "7日"], "--older-than", "7日"),
        (&["kill", "--idle-for", "1.5h"], "--idle-for", "1.5h"),
    ];
    for (args, flag, text) in cases {
        let output = run(args);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(
            output.status.code(),
            Some(USAGE_ERROR),
            "faulte {args:?} is a usage error: {stderr:?}"
        );
        assert!(
            stderr.contains(&format!("invalid value '{text}' for '{flag} <DURATION>'")),
            "clap names the flag {flag} and the text {text:?}: {stderr:?}"
        );
        assert!(
            stderr.contains(&format!("{text:?}")),
            "the message of the parser names the text {text:?}: {stderr:?}"
        );
    }
}

/// `--help` gives each flag and its default, so the defaults of the issue are
/// held here: a 5 s sample, 25 rows, sessions older than 7 days, and idle for
/// more than 10 minutes.
#[test]
fn help_gives_each_flag_and_its_default() {
    let top = run(&["--help"]);
    let kill = run(&["kill", "--help"]);
    assert!(top.status.success(), "faulte --help exits 0");
    assert!(kill.status.success(), "faulte kill --help exits 0");

    let top = one_line(&top.stdout);
    for phrase in [
        "--interval <DURATION>",
        "[default: 5s]",
        "--limit <N>",
        "[default: 25]",
        "kill",
    ] {
        assert!(
            top.contains(phrase),
            "faulte --help gives {phrase:?}: {top}"
        );
    }

    let kill = one_line(&kill.stdout);
    for phrase in [
        "--older-than <DURATION>",
        "[default: 7d]",
        "--idle-for <DURATION>",
        "[default: 10m]",
        "--max <N>",
        "--interval <DURATION>",
    ] {
        assert!(
            kill.contains(phrase),
            "faulte kill --help gives {phrase:?}: {kill}"
        );
    }
}
