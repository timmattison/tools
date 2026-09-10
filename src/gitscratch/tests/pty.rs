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

/// The newlines of the child come back as the child wrote them.
///
/// A terminal changes each newline into a carriage return and a newline on its
/// way out, unless its output processing is off. A test compares the bytes of
/// a tool on a terminal with the bytes of the same tool on a pipe, so the
/// terminal must give back what the tool wrote, byte for byte.
#[test]
fn the_newlines_of_the_child_come_back_unchanged() {
    let output = on_a_terminal(r"printf 'a\nb\n'");

    assert_eq!(
        output.stdout,
        b"a\nb\n",
        "the terminal must give back each newline as the child wrote it\n{}",
        described(&output)
    );
}

/// Far more bytes than the buffer of a terminal or of a pipe holds.
///
/// A pseudo-terminal holds a few KiB on macOS, and a pipe holds 64 KiB. A
/// mebibyte is far past both on each system this suite runs on.
const FAR_PAST_ANY_BUFFER: usize = 1 << 20;

/// Output far bigger than any buffer comes back whole, on standard output and
/// on standard error both.
///
/// A child that writes more than a buffer holds stops until somebody reads that
/// buffer. So the helper reads the master end and standard error while the
/// child runs. A helper that reads one stream to its end before it reads the
/// other hangs here: the child waits for a read of the second stream, and the
/// helper waits for the end of the first. The test then hangs and does not
/// fail. That is still a guard, because no run of such a helper gets past it.
#[test]
fn output_far_bigger_than_any_buffer_comes_back_whole_on_both_streams() {
    let script = format!(
        "head -c {FAR_PAST_ANY_BUFFER} /dev/zero | tr '\\0' x; \
         head -c {FAR_PAST_ANY_BUFFER} /dev/zero | tr '\\0' y >&2"
    );

    let output = on_a_terminal(&script);

    assert!(
        output.status.success(),
        "the child must write both streams and succeed: {}",
        output.status
    );
    for (stream, bytes, byte) in [
        ("standard output", &output.stdout, b'x'),
        ("standard error", &output.stderr, b'y'),
    ] {
        assert_eq!(
            bytes.len(),
            FAR_PAST_ANY_BUFFER,
            "{stream} must come back whole"
        );
        assert!(
            bytes.iter().all(|&each| each == byte),
            "{stream} must hold only the byte the child wrote there"
        );
    }
}

/// The terminal is the controlling terminal of the child, at the size it was
/// opened with.
///
/// A tool measures its width through `/dev/tty`, which names the controlling
/// terminal. A child that kept the terminal of whoever started the run measures
/// that window, and a child with no controlling terminal measures nothing. So
/// the child reads the size of `/dev/tty`, and the answer must be the rows and
/// the columns that this test chose.
#[test]
fn the_controlling_terminal_of_the_child_is_the_terminal_at_its_opened_size() {
    let output = on_a_terminal("stty size < /dev/tty");

    assert_eq!(
        output.stdout,
        format!("{} {COLUMNS}\n", Pty::ROWS).as_bytes(),
        "the child must measure this terminal through /dev/tty\n{}",
        described(&output)
    );
}
