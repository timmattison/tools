//! End-to-end tests of the command line of the `cctop-bar` binary. None of
//! them opens a status item.

#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::process::Output;

/// The default chord in its two forms, as `--help` names them.
const DEFAULT_CHORD_TEXT: &str = "ctrl+alt+shift+cmd+backspace";
const DEFAULT_CHORD_SYMBOLS: &str = "⌃⌥⇧⌘⌫";

/// Runs the binary with `arguments`. The width of the help is fixed, so the
/// tests do not depend on the terminal of whoever runs them.
fn run(arguments: &[&str]) -> Output {
    childenv::fixture_command(env!("CARGO_BIN_EXE_cctop-bar"))
        .args(arguments)
        .env("COLUMNS", "200")
        .output()
        .expect("the binary starts")
}

/// The visible text of the standard output.
fn stdout_of(output: &Output) -> String {
    testcolor::strip_ansi(&String::from_utf8_lossy(&output.stdout))
}

/// The visible text of the standard error.
fn stderr_of(output: &Output) -> String {
    testcolor::strip_ansi(&String::from_utf8_lossy(&output.stderr))
}

#[test]
fn version_names_the_tool_and_the_build() {
    let output = run(&["--version"]);
    assert!(output.status.success());
    let text = stdout_of(&output);
    let rest = text
        .trim_end()
        .strip_prefix("cctop-bar 0.1.0 (")
        .unwrap_or_else(|| panic!("unexpected version text: {text}"));
    let (hash, state) = rest
        .strip_suffix(')')
        .and_then(|inner| inner.split_once(", "))
        .unwrap_or_else(|| panic!("unexpected version text: {text}"));
    assert!(!hash.is_empty(), "{text}");
    assert!(
        state == "clean" || state == "dirty" || state == "unknown",
        "{text}"
    );
}

#[test]
fn a_bad_hotkey_exits_with_2_and_names_the_bad_part() {
    let output = run(&["--hotkey", "ctrl+nonsense"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr_of(&output).contains("\"nonsense\""),
        "{}",
        stderr_of(&output)
    );
}

#[test]
fn a_hotkey_with_no_hotkey_exits_with_2() {
    let output = run(&["--hotkey", "ctrl+cmd+k", "--no-hotkey"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn help_names_the_default_chord_in_both_forms() {
    let output = run(&["--help"]);
    assert!(output.status.success());
    let text = stdout_of(&output);
    assert!(text.contains(DEFAULT_CHORD_TEXT), "{text}");
    assert!(text.contains(DEFAULT_CHORD_SYMBOLS), "{text}");
}

#[test]
fn help_explains_the_one_status_item_for_each_user() {
    let text = stdout_of(&run(&["--help"]));
    assert!(text.contains("one status item"), "{text}");
    assert!(text.contains("second run"), "{text}");
    assert!(text.contains("-f"), "{text}");
}

#[cfg(not(target_os = "macos"))]
#[test]
fn a_plain_run_refuses_a_platform_other_than_macos() {
    let output = run(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr_of(&output).contains("cctop-bar supports macOS only."));
}
