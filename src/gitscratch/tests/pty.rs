//! Tests of the pseudo-terminal that `gitscratch::testing::pty` gives a test.
//!
//! A test of `grind` or `grime` runs the tool with its standard output on a
//! pseudo-terminal, and asserts on the bytes that the tool wrote there. Those
//! assertions are only as good as the helper under them. So this file holds
//! the helper to what those tests need from it.
//!
//! Each test opens a pseudo-terminal of its own and starts `sh`, so the tests
//! share no path and no terminal, and they run beside each other. No test
//! starts git, so the scrub of the inherited git environment has nothing to do
//! here.

#![cfg(unix)]

use std::process::{Command, Output};

use gitscratch::testing::pty::Pty;

/// The width of every terminal here.
///
/// An unusual width, so a child that measured some other terminal does not
/// report this number by chance.
const COLUMNS: u16 = 97;

/// A `sh -c` command that runs `script`.
fn shell(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    command
}

/// Run `script` with its standard output on a terminal [`COLUMNS`] wide.
fn on_a_terminal(script: &str) -> Output {
    Pty::open(COLUMNS).run_with_stdout_on_terminal(shell(script))
}

/// The whole picture of a run, for the message of an assertion that fails.
fn described(output: &Output) -> String {
    format!(
        "status: {}\nstdout: {:?}\nstderr: {:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

/// A child whose standard output is on the terminal sees a terminal there.
///
/// This is the reason the helper exists. A tool that decides color by whether
/// standard output is a terminal paints only when it sees one, so a test of
/// that color needs a child that sees one.
#[test]
fn a_child_whose_stdout_is_on_the_terminal_sees_a_terminal() {
    let output = on_a_terminal("if [ -t 1 ]; then printf terminal; else printf pipe; fi");

    assert_eq!(
        output.stdout,
        b"terminal",
        "standard output of the child must be a terminal\n{}",
        described(&output)
    );
}
