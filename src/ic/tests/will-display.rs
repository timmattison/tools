//! Black-box tests for the `--will-display` flag, which answers "can `ic` show
//! an image here?" as a process exit status.
//!
//! Every test drives the real binary with a cleared environment, so the answer
//! comes from the variables the test sets and from nothing the test runner
//! inherited. The `PATH` points at a directory that does not exist, which keeps
//! `ps` out of reach: the remote-transport detection then finds no process tree
//! to walk, so a test runner that is itself under mosh cannot change the
//! verdict. The path holds the process id and a nanosecond stamp, so two
//! concurrent runs of this file never name the same directory.

use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

/// A directory that does not exist, unique to this process.
fn unreachable_path_dir() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock must be after the epoch")
        .as_nanos();
    format!(
        "/nonexistent-ic-will-display-{}-{nanos}",
        std::process::id()
    )
}

/// Invoke the freshly-built `ic` binary in a known-empty environment.
fn ic(term: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ic"));
    command.env_clear();
    command.env("PATH", unreachable_path_dir());
    command.env("TERM", term);
    command
}

fn run(output: Output) -> (Option<i32>, String, String) {
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A terminal that renders graphics, with no multiplexer and no remote
/// transport. `ic` must report that it can display an image, and say nothing.
#[test]
fn will_display_succeeds_silently_in_a_graphics_capable_session() {
    let (code, stdout, stderr) = run(ic("xterm-256color")
        .arg("--will-display")
        .output()
        .expect("ic must run"));

    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(stdout, "", "success must print nothing to stdout");
    assert_eq!(stderr, "", "success must print nothing to stderr");
}

/// The Linux console renders no graphics protocol. `ic` must fail with the
/// reason on stderr.
#[test]
fn will_display_fails_and_names_the_terminal_when_graphics_are_unsupported() {
    let (code, _stdout, stderr) = run(ic("linux")
        .arg("--will-display")
        .output()
        .expect("ic must run"));

    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("not supported in this terminal"),
        "stderr must give the reason: {stderr}"
    );
}

/// tmux strips the escape sequences that carry an image. `ic` must fail and
/// name tmux.
#[test]
fn will_display_fails_and_names_tmux() {
    let (code, _stdout, stderr) = run(ic("xterm-256color")
        .arg("--will-display")
        .env("TMUX", "/private/tmp/tmux-501/default,1,0")
        .output()
        .expect("ic must run"));

    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(stderr.contains("tmux"), "stderr must name tmux: {stderr}");
}

/// The flag asks a question. It does not display a file, so pairing it with a
/// file is a mistake `ic` must report instead of quietly ignoring one of them.
#[test]
fn will_display_refuses_to_share_the_command_with_a_file() {
    let (code, _stdout, stderr) = run(ic("xterm-256color")
        .args(["--will-display", "picture.png"])
        .output()
        .expect("ic must run"));

    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("multiple input modes"),
        "stderr must explain the conflict: {stderr}"
    );
}
