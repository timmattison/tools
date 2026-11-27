//! Black-box tests for `--protocol` and `IC_PROTOCOL`, which force the image
//! protocol that `ic` draws with.
//!
//! An xterm.js front end such as ttyd draws Sixel and the iTerm2 protocol
//! through `@xterm/addon-image`, and it draws no Kitty command. The environment
//! that such a front end inherits can name a terminal of the Kitty protocol,
//! and `ic` then writes base64 that lands on the screen as text. A person who
//! knows the terminal forces the protocol instead, and every test here names a
//! terminal of one protocol and forces another one.
//!
//! Each test drives the real binary with a cleared environment, so the
//! terminal comes from the variables that the test sets. The `PATH` reaches no
//! `ps`, or the `ps` of a [`MoshProcessTable`], so the transport comes from the
//! test and not from the machine of whoever runs the suite. Each child runs
//! with no controlling terminal, so no run reads or writes the terminal of a
//! person. [`take_the_terminal_away`] states why.

use std::io::Write;
use std::process::{Command, Output, Stdio};

mod common;

use common::pty::take_the_terminal_away;
use common::{find, unreachable_path_dir, MoshProcessTable, SIXEL_START, TEST_IMAGE};

/// The name that this target puts in the `PATH` of its children.
const TARGET_NAME: &str = "protocol-override";

/// The application program command that opens a Kitty graphics command.
const KITTY_START: &[u8] = b"\x1b_G";

/// The operating system command that opens an iTerm2 inline image.
const ITERM2_START: &[u8] = b"\x1b]1337;";

/// The environment of a Kitty window. `ic` draws the Kitty protocol there.
const KITTY_TERMINAL: &[(&str, &str)] = &[("TERM", "xterm-kitty"), ("KITTY_WINDOW_ID", "1")];

/// The environment of an iTerm2 window. `ic` draws the iTerm2 protocol there.
const ITERM2_TERMINAL: &[(&str, &str)] =
    &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "iTerm.app")];

/// The environment of a Ghostty window, which draws the Kitty protocol.
const GHOSTTY_TERMINAL: &[(&str, &str)] = &[("TERM", "xterm-ghostty")];

/// The arguments that draw the image of stdin at a fixed size, so no test
/// reads the size of a terminal.
const STDIN_ARGS: &[&str] = &["--stdin", "--width", "10", "--height", "5"];

/// The test image as a file on disk, for the path that reads a file argument.
const TEST_IMAGE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test_image.png");

/// Build a command that runs `ic` in a cleared environment.
///
/// # Arguments
/// * `terminal` - The variables that name the terminal.
/// * `args` - The full command line for `ic`.
///
/// # Returns
/// A command with the environment, the pipes and the new session already set.
/// The `PATH` reaches no program, and a caller that wants a `ps` sets its own.
fn ic_command(terminal: &[(&str, &str)], args: &[&str]) -> Command {
    let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_ic"));
    command
        .args(args)
        .env_clear()
        .env("PATH", unreachable_path_dir(TARGET_NAME))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    take_the_terminal_away(&mut command);

    for (name, value) in terminal {
        command.env(name, value);
    }

    command
}

/// Run `command` with the test image on stdin, and give back what it did.
///
/// # Arguments
/// * `command` - The command to run.
///
/// # Returns
/// The exit status and the two streams of the run.
///
/// # Panics
/// Panics when the child does not start or does not take the image.
fn run_with_the_image(command: &mut Command) -> Output {
    let mut child = command.spawn().expect("failed to start ic");

    let mut stdin = child.stdin.take().expect("ic has no stdin pipe");
    stdin
        .write_all(TEST_IMAGE)
        .expect("failed to send the image to ic");
    drop(stdin);

    child.wait_with_output().expect("failed to wait for ic")
}

/// Assert that a run succeeded and drew with the protocol of `expected`.
///
/// # Arguments
/// * `output` - What the run did.
/// * `expected` - The bytes that open the protocol of the choice.
/// * `refused` - The bytes that open the protocol of the terminal, which the
///   choice replaces.
///
/// # Panics
/// Panics when the run failed, when the output holds no `expected`, or when it
/// holds `refused`.
fn assert_drawn_with(output: &Output, expected: &[u8], refused: &[u8]) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "ic exited with {}: {stderr}",
        output.status
    );
    assert!(
        find(&output.stdout, expected).is_some(),
        "the output must hold the protocol of the choice {:?}: {:?}",
        String::from_utf8_lossy(expected),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        find(&output.stdout, refused).is_none(),
        "the output must hold no command of the protocol of the terminal {:?}: {:?}",
        String::from_utf8_lossy(refused),
        String::from_utf8_lossy(&output.stdout)
    );
}

/// `--protocol sixel` draws Sixel in a Kitty window.
#[test]
fn the_protocol_flag_draws_sixel_on_a_kitty_terminal() {
    let mut args = STDIN_ARGS.to_vec();
    args.extend_from_slice(&["--protocol", "sixel"]);

    let output = run_with_the_image(&mut ic_command(KITTY_TERMINAL, &args));

    assert_drawn_with(&output, SIXEL_START, KITTY_START);
}

/// `IC_PROTOCOL=iterm2` draws the iTerm2 protocol in a Kitty window.
///
/// The variable pins the protocol for a whole shell session, which is the way
/// to use a ttyd front end that inherited the environment of a Kitty window.
#[test]
fn ic_protocol_draws_iterm2_on_a_kitty_terminal() {
    let output =
        run_with_the_image(ic_command(KITTY_TERMINAL, STDIN_ARGS).env("IC_PROTOCOL", "iterm2"));

    assert_drawn_with(&output, ITERM2_START, KITTY_START);
}

/// The flag wins over `IC_PROTOCOL`.
#[test]
fn the_protocol_flag_wins_over_ic_protocol() {
    let mut args = STDIN_ARGS.to_vec();
    args.extend_from_slice(&["--protocol", "sixel"]);

    let output = run_with_the_image(ic_command(ITERM2_TERMINAL, &args).env("IC_PROTOCOL", "kitty"));

    assert_drawn_with(&output, SIXEL_START, ITERM2_START);
}

/// A file argument draws in the protocol of the choice as well.
///
/// The file path reads the terminal before it decodes the file, to decide
/// whether the bytes of the file travel as they stand. This test holds that
/// read to the choice too.
#[test]
fn a_file_draws_in_the_protocol_of_the_choice() {
    // A run that reads a file reads no stdin, so the child gets none. A write
    // to the stdin of a child that already exited fails.
    let output = ic_command(
        KITTY_TERMINAL,
        &[
            "--width",
            "10",
            "--height",
            "5",
            "--protocol",
            "iterm2",
            TEST_IMAGE_PATH,
        ],
    )
    .stdin(Stdio::null())
    .output()
    .expect("failed to run ic");

    assert_drawn_with(&output, ITERM2_START, KITTY_START);
}

/// A forced protocol draws under a process tree that names an upstream Mosh.
///
/// The rule about Mosh reads the process tree, and under a multiplexer that
/// tree can name the Mosh of another client. The choice of a protocol is the
/// way past that refusal, and the picture draws in the protocol of the choice.
#[test]
fn a_forced_protocol_draws_under_an_upstream_mosh() {
    let table = MoshProcessTable::new(TARGET_NAME);
    let mut args = STDIN_ARGS.to_vec();
    args.extend_from_slice(&["--protocol", "sixel"]);

    let output = run_with_the_image(ic_command(GHOSTTY_TERMINAL, &args).env("PATH", table.path()));

    assert_drawn_with(&output, SIXEL_START, KITTY_START);
}
