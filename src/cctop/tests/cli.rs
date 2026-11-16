// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error"
)]

//! Tests that run the built `cctop` binary. Standard output is always a pipe,
//! so no test draws a screen.

use std::process::Output;

/// The exit code for a usage error.
const USAGE_ERROR: i32 = 2;

/// Run the binary with `args`, with a config directory that the test owns.
fn run(args: &[&str]) -> Output {
    let config = tempfile::tempdir().unwrap();
    childenv::fixture_command(env!("CARGO_BIN_EXE_cctop"))
        .env("CLAUDE_CONFIG_DIR", config.path())
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn version_names_the_tool_the_hash_and_the_tree_state() {
    let output = run(&["--version"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    let rest = text
        .trim_end()
        .strip_prefix("cctop 0.1.0 (")
        .unwrap_or_else(|| panic!("unexpected version text: {text:?}"));
    let (hash, state) = rest
        .strip_suffix(')')
        .and_then(|inner| inner.split_once(", "))
        .unwrap_or_else(|| panic!("unexpected version text: {text:?}"));
    assert!(!hash.is_empty(), "{text:?}");
    assert!(
        ["clean", "dirty", "unknown"].contains(&state),
        "unexpected tree state in {text:?}"
    );
}

#[test]
fn help_mentions_no_notify() {
    let output = run(&["--help"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("--no-notify"), "{text}");
}

#[test]
fn a_run_with_standard_output_on_a_pipe_exits_with_2() {
    let output = run(&[]);
    assert_eq!(output.status.code(), Some(USAGE_ERROR), "{output:?}");
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("standard output is not a terminal"), "{text}");
}
