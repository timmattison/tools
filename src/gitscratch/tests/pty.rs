//! Tests of the pseudo-terminal that `gitscratch::testing::pty` gives a test.
//!
//! A test of `grind` or `grime` runs the tool with its standard output on a
//! pseudo-terminal, and asserts on the bytes that the tool wrote there. Those
//! assertions are only as good as the helper under them. So this file holds
//! the helper to what those tests need from it.
//!
//! Each test opens a pseudo-terminal of its own and starts `sh` or `printenv`,
//! so the tests share no path and no terminal, and they run beside each other.
//! No test starts git, so the scrub of the inherited git environment has
//! nothing to do here.

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
    let mut command = childenv::fixture_command("sh");
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

/// A script that reads one line from standard input when standard input is a
/// terminal, and prints what it read. It prints `pipe` when standard input is
/// not a terminal.
const READ_A_LINE_FROM_A_TERMINAL: &str =
    r#"if [ -t 0 ]; then read answer; printf 'read %s' "$answer"; else printf pipe; fi"#;

/// A child whose standard input is on the terminal reads the line that the
/// test typed.
///
/// A tool that asks a question only when a person can answer it looks for a
/// terminal on standard input, and then reads the answer from there. So a test
/// of the question needs a child that sees a terminal on standard input, and
/// an answer that arrives through that terminal.
#[test]
fn a_child_whose_stdin_is_on_the_terminal_reads_the_typed_line() {
    let output =
        Pty::open(COLUMNS).run_with_stdin_on_terminal(shell(READ_A_LINE_FROM_A_TERMINAL), b"n\n");

    assert_eq!(
        output.stdout,
        b"read n",
        "the child must read the typed line from a terminal on standard input\n{}",
        described(&output)
    );
}

/// A child that reads nothing still ends, and its standard output and its
/// standard error come back on pipes.
///
/// A tool that decides not to ask a question never reads the typed line. The
/// run must end all the same. The test also reads both streams of the child,
/// because a test of the question asserts on what the tool said there.
#[test]
fn a_child_that_reads_nothing_ends_and_gives_back_both_streams() {
    let output = Pty::open(COLUMNS).run_with_stdin_on_terminal(
        shell("if [ -t 1 ]; then printf terminal; else printf pipe; fi; printf said >&2"),
        b"y\n",
    );

    assert!(output.status.success(), "{}", described(&output));
    assert_eq!(
        output.stdout,
        b"pipe",
        "standard output of the child must be a pipe\n{}",
        described(&output)
    );
    assert_eq!(output.stderr, b"said", "{}", described(&output));
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

/// A child whose two output streams are on the terminal gives back both of
/// them through the terminal.
///
/// A tool that draws a display on standard error only when that stream is a
/// terminal needs a run where both streams reach one terminal, as they do for
/// a person. The `stderr` of the result is then empty, because nothing went
/// to a pipe.
#[test]
fn a_child_whose_stdout_and_stderr_are_on_the_terminal_gives_back_both_there() {
    let output =
        Pty::open(COLUMNS).run_with_output_on_terminal(shell("printf out; printf err >&2"));

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("out") && text.contains("err"),
        "the terminal must carry both streams\n{}",
        described(&output)
    );
    assert!(
        output.stderr.is_empty(),
        "no stream may go to a pipe\n{}",
        described(&output)
    );
}

/// The name of the variable that holds the type of a terminal.
const TERM: &str = "TERM";

/// The line that the probe prints when the child sees the terminal type that
/// the helper names.
///
/// A tool that draws a display on a terminal reads `TERM` first, and draws
/// nothing for `dumb` or for no `TERM` at all. A GitHub runner sets no `TERM`,
/// so a child that inherits the environment of the run there draws nothing.
const DEFAULT_TERM_LINE: &[u8] = b"xterm-256color\n";

/// A command that prints the `TERM` of its environment and a newline, and
/// prints nothing and exits 1 when its environment holds no `TERM`.
///
/// The probe is `printenv` and not a shell script, because it must read the
/// environment that the child got. `sh` on macOS sets a shell variable
/// `TERM=dumb` when its environment holds no `TERM`, so `$TERM` in a script
/// shows a value that the helper did not give.
fn print_term() -> Command {
    let mut command = childenv::fixture_command("printenv");
    command.arg(TERM);
    command
}

/// [`print_term`] in an environment that holds nothing.
///
/// The command states no variable, so the child sees only what the helper
/// adds. The `TERM` of whoever started the run does not reach the child, and
/// the test gives one answer under every terminal and under none.
fn print_term_in_an_empty_environment() -> Command {
    let mut command = print_term();
    command.env_clear();
    command
}

/// Run a command from `build` once in each of the four shapes that the helper
/// gives, and give back the name of each shape with what its child printed.
///
/// The `Command` of each run comes fresh from `build`, because a run takes its
/// command. Each run opens a terminal of its own.
fn on_each_shape(build: impl Fn() -> Command) -> Vec<(&'static str, Output)> {
    let given = {
        let pty = Pty::open(COLUMNS);
        let mut command = build();
        pty.give_as_controlling_terminal(&mut command);
        let output = command
            .output()
            .unwrap_or_else(|error| panic!("the child must start: {error}"));
        drop(pty);
        output
    };

    vec![
        ("give_as_controlling_terminal", given),
        (
            "run_with_stdout_on_terminal",
            Pty::open(COLUMNS).run_with_stdout_on_terminal(build()),
        ),
        (
            "run_with_output_on_terminal",
            Pty::open(COLUMNS).run_with_output_on_terminal(build()),
        ),
        (
            "run_with_stdin_on_terminal",
            Pty::open(COLUMNS).run_with_stdin_on_terminal(build(), b""),
        ),
    ]
}

/// A child whose command states no `TERM` sees the terminal type that the
/// helper names.
///
/// A terminal that a person uses always has a type, and a tool that draws a
/// display reads it. A test that gives the tool a terminal must therefore give
/// it a type too, or the test passes only where the run inherits one.
#[test]
fn a_child_whose_command_states_no_term_sees_the_default_term() {
    for (shape, output) in on_each_shape(print_term_in_an_empty_environment) {
        assert_eq!(
            output.stdout,
            DEFAULT_TERM_LINE,
            "{shape}: the child must see the terminal type of the helper\n{}",
            described(&output)
        );
    }
}

/// A `TERM` that the command sets wins over the default of the helper.
///
/// A test of how a tool behaves on a `dumb` terminal sets that type on its
/// command, and the helper must not change it.
#[test]
fn a_term_that_the_command_sets_wins_over_the_default() {
    for (shape, output) in on_each_shape(|| {
        let mut command = print_term_in_an_empty_environment();
        command.env(TERM, "dumb");
        command
    }) {
        assert_eq!(
            output.stdout,
            b"dumb\n",
            "{shape}: the child must see the TERM of its command\n{}",
            described(&output)
        );
    }
}

/// A `TERM` that the command removes stays removed.
///
/// A test of how a tool behaves with no terminal type removes `TERM` from its
/// command, and the helper must not put it back.
#[test]
fn a_term_that_the_command_removes_stays_removed() {
    for (shape, output) in on_each_shape(|| {
        let mut command = print_term();
        command.env_remove(TERM);
        command
    }) {
        assert_eq!(
            (output.status.code(), output.stdout.as_slice()),
            (Some(1), &b""[..]),
            "{shape}: the child must see no TERM\n{}",
            described(&output)
        );
    }
}
