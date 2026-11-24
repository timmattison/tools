//! End-to-end tests of `clipboard-random` in a mosh-rs session.
//!
//! In a mosh-rs session the clipboard of the person at the keyboard lives on
//! the client. The server gives each session a `pbcopy` shim and a `pbpaste`
//! shim, and `sysclip` must use them. The tool has three writes to the
//! clipboard: the text mode, the hex and base64 output of the binary mode, and
//! the raw output of the binary mode. Each write has one test here. A change of
//! one call site to a route that skips the shims fails the test of that call
//! site.
//!
//! No child touches the clipboard of the machine. Each child starts with an
//! empty environment. `MOSH_OPEN_SOCKET` is set, and the fake commands of
//! `sysclip::testing::FakeShims` come first on `PATH`. In that route `sysclip`
//! never opens `arboard`, and the fake `pbcopy` keeps the text in a file of the
//! test. Each test has its own temporary directory, so two runs of this file at
//! the same time stay apart.

#![cfg(unix)]

use std::process::Output;

use sysclip::testing::FakeShims;
use tempfile::TempDir;

/// The directory of the fake commands, inside the temporary directory.
const SHIMS_DIRECTORY: &str = "shims";

/// The number of random bytes that the binary tests ask for.
const BINARY_BYTES: usize = 16;

/// The number of characters that the text test asks for.
const TEXT_CHARS: usize = 20;

/// Installs the fake commands in the temporary directory of one test.
fn install_shims(scratch: &TempDir) -> FakeShims {
    FakeShims::install(&scratch.path().join(SHIMS_DIRECTORY))
}

/// Runs the tool in a mosh-rs session, with an environment built from nothing.
fn run_in_session(shims: &FakeShims, args: &[&str]) -> Output {
    let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_clipboard-random"));
    command.env_clear();
    shims
        .enter_session(&mut command)
        .args(args)
        .output()
        .expect("the built binary starts")
}

/// What the run wrote to standard error.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Asserts that the run succeeded, and shows its standard error when it did not.
fn assert_success(output: &Output) {
    assert!(output.status.success(), "stderr: {}", stderr(output));
}

#[test]
fn text_mode_in_a_mosh_session_writes_through_pbcopy() {
    let scratch = TempDir::new().expect("the temporary directory is made");
    let shims = install_shims(&scratch);

    let output = run_in_session(&shims, &["text", &TEXT_CHARS.to_string()]);

    assert_success(&output);
    let held = shims.held();
    assert!(!held.is_empty(), "the fake pbcopy kept no text");
    let line = format!(
        "Text length: {} characters ({} bytes)",
        held.chars().count(),
        held.len()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|l| l == line),
        "stdout lacks {line:?}: {stdout}"
    );
}

#[test]
fn binary_hex_in_a_mosh_session_writes_through_pbcopy() {
    let scratch = TempDir::new().expect("the temporary directory is made");
    let shims = install_shims(&scratch);

    let output = run_in_session(&shims, &["binary", &BINARY_BYTES.to_string()]);

    assert_success(&output);
    let held = shims.held();
    assert_eq!(held.chars().count(), BINARY_BYTES * 2);
    assert!(held.chars().all(|c| c.is_ascii_hexdigit()), "held: {held}");
}

#[test]
fn binary_raw_in_a_mosh_session_writes_through_pbcopy() {
    let scratch = TempDir::new().expect("the temporary directory is made");
    let shims = install_shims(&scratch);

    let output = run_in_session(
        &shims,
        &["binary", &BINARY_BYTES.to_string(), "--format", "raw"],
    );

    assert_success(&output);
    let held = shims.held();
    assert_eq!(held.chars().count(), BINARY_BYTES);
    assert!(held.chars().all(|c| c <= '\u{ff}'), "held: {held:?}");
}
