//! End-to-end tests of the command line of `chf`.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::process::{Command, Stdio};

/// The byte that starts each control sequence of a terminal.
#[cfg(unix)]
const ESCAPE: u8 = 0x1b;

/// The command for the `chf` binary, with an environment that holds only a
/// home directory and a plain path.
fn chf(home: &tempfile::TempDir) -> Command {
    let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_chf"));
    command
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin");
    command
}

#[test]
fn version_prints_name_number_and_build() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let output = chf(&home).arg("--version").output().expect("chf starts");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(
        stdout.starts_with("chf 0.1.0 ("),
        "unexpected version text: {stdout:?}"
    );
    assert!(output.status.success());
}

#[test]
fn help_names_ch_and_che() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let output = chf(&home).arg("--help").output().expect("chf starts");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(output.status.success());
    assert!(stdout.contains("ch "), "help must name ch: {stdout:?}");
    assert!(stdout.contains("che"), "help must name che: {stdout:?}");
}

#[test]
fn a_pipe_for_standard_output_is_refused_with_exit_2() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let output = chf(&home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("chf starts");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "nothing goes to standard output");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(stderr.contains("must be a terminal"), "stderr: {stderr:?}");
}

#[cfg(unix)]
#[test]
fn a_null_standard_input_is_refused_before_chf_draws() {
    let home = tempfile::tempdir().expect("a temporary directory");
    let mut command = chf(&home);
    command.env("TERM", "xterm-256color");
    let output = gitscratch::testing::pty::Pty::open(80).run_with_stdout_on_terminal(command);
    assert_eq!(output.status.code(), Some(2));
    // Standard error goes to a pipe, so the terminal gets only standard
    // output. An escape byte there means that chf changed the terminal, for
    // example to enter the alternate screen, before it refused.
    let shown = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.stdout.contains(&ESCAPE),
        "chf must send no control sequence to the terminal: {shown:?}"
    );
}
