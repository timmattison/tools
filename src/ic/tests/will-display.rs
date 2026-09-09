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
//!
//! # The flag asks a question and draws nothing
//!
//! `--will-display` reports what the terminal is, and it puts no picture on the
//! screen. So the one fact it needs is the protocol, and the size of a
//! character cell decides nothing it prints. The last test of this file holds
//! it to that: a run under a named terminal whose window reports no pixel size
//! must write nothing at all to the terminal. It therefore takes a
//! pseudo-terminal of its own, where the other tests need none, and every wait
//! it makes carries a deadline.

use std::process::{Command, Output, Stdio};
use std::time::Duration;

mod common;

use common::pty::{Pty, Window};
use common::{unreachable_path_dir, MoshProcessTable};

/// The name that this target puts in the unreachable `PATH` of its children.
const TARGET_NAME: &str = "will-display";

/// Invoke the freshly-built `ic` binary in a known-empty environment.
fn ic(term: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ic"));
    command.env_clear();
    command.env("PATH", unreachable_path_dir(TARGET_NAME));
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

/// The terminal type that names a Kitty terminal.
///
/// **The name is the point of the test below.** A terminal that named itself
/// answered the question about the protocol already, and `--will-display`
/// prints its verdict from that one fact. So a run under this name has nothing
/// left to ask.
const TERM_XTERM_KITTY: &str = "xterm-kitty";

/// The window that the pseudo-terminal of the test below reports.
///
/// **The pixel size is zero on both axes**, which is what a mosh session, a
/// pane of Zellij and a ttyd panel all report. That is the window that used to
/// make every run ask, whatever the run was for.
const WINDOW: Window = Window {
    columns: 80,
    rows: 24,
    width_px: 0,
    height_px: 0,
};

/// The request of the primary device attributes.
///
/// Every terminal answers it, so its answer is what ends a read of the
/// terminal, and it stands last in the query that `ic` writes. A read that
/// waits for it therefore waits for the whole of a query.
const ATTRIBUTES_REQUEST: &[u8] = b"\x1b[c";

/// How long one read of the terminal waits for a byte.
///
/// The test reads the terminal and asks after the child in turn, and this is
/// the length of one turn. A run that asks writes its query inside the first
/// one, and [`Pty::read_until`] gives that query back the moment the request of
/// the attributes ends it.
const READ_SLICE: Duration = Duration::from_millis(100);

/// How many turns the test takes before it gives up on the child.
///
/// Ten seconds in all, which is generous because a loaded machine starts a
/// process late. **The count is the deadline of the whole wait**: a change that
/// stopped `ic` from exiting must fail this test instead of holding it.
const READ_SLICES: usize = 100;

/// A named terminal whose window reports no pixel size must be asked nothing.
///
/// `--will-display` reads the name of the terminal and whether that terminal
/// draws an image at all. It draws no picture, so it needs no character cell,
/// and the round trip that measures one costs the budget of the query and
/// swallows whatever the user typed while it drained the terminal in raw mode.
///
/// **The read of the terminal runs beside the wait for the child, and not
/// behind it.** A process that closes its controlling terminal waits for the
/// output queue of that terminal to drain, and the master end is where it
/// drains to. So a run that wrote a query the test had not read yet would hang
/// on the way out, and a wait in front of the read would hold the two of them
/// there for good.
#[test]
fn will_display_asks_nothing_of_a_named_terminal_that_reports_no_pixel_size() {
    let pty = Pty::open(WINDOW);

    let mut command = ic(TERM_XTERM_KITTY);
    command
        .arg("--will-display")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    pty.hand_to(&mut command);

    let mut child = command.spawn().expect("ic must run");

    let mut wrote = Vec::new();
    let mut exited = None;
    for _ in 0..READ_SLICES {
        wrote.extend(pty.read_until(ATTRIBUTES_REQUEST, READ_SLICE));
        if let Some(status) = child.try_wait().expect("failed to ask after ic") {
            exited = Some(status);
            break;
        }
    }
    let status = exited.expect("ic must exit inside the deadline of this test");
    // The child has exited, so every byte it ever wrote to the terminal is
    // waiting there already. One last turn takes the bytes it wrote on its way
    // out.
    wrote.extend(pty.read_until(ATTRIBUTES_REQUEST, READ_SLICE));

    assert!(
        wrote.is_empty(),
        "--will-display must write nothing to the terminal of a named session, and it wrote {:?}",
        String::from_utf8_lossy(&wrote)
    );
    assert!(
        status.success(),
        "and it must still report that a Kitty terminal displays an image: {status}"
    );
}

/// The value of `MOSH_IMAGES` for a Mosh that carries every image protocol.
///
/// Only a Mosh that draws images writes this variable. Upstream Mosh strips
/// every image sequence and writes nothing, which is why an absent variable is
/// a refusal and not a question.
const MOSH_CARRIES_EVERY_PROTOCOL: &str = "kitty,sixel,iterm2";

/// Invoke `ic` inside a session that the process tree reports as Mosh.
///
/// The `PATH` reaches the stated `ps` of `table` and reaches nothing else, so
/// the transport comes from the table and not from the machine of whoever runs
/// the suite. Every other test of this file points the `PATH` at a directory
/// that does not exist, which is the same rule read the other way: a test
/// states the process tree it covers, and it reads none.
fn ic_under_mosh(term: &str, table: &MoshProcessTable) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ic"));
    command.env_clear();
    command.env("PATH", table.path());
    command.env("TERM", term);
    command
}

/// A Mosh that carries images draws a picture for a terminal that named
/// itself.
///
/// This is the refusal of issue #471, as the user meets it. The rule about
/// Mosh stood in front of the rule that reads what the terminal draws, so
/// every terminal the environment named took the refusal of upstream Mosh. A
/// query cannot answer that rule inside a multiplexer, so the session states
/// it in the environment instead.
#[test]
fn will_display_succeeds_for_a_named_terminal_of_a_mosh_that_carries_images() {
    let table = MoshProcessTable::new(TARGET_NAME);
    let (code, stdout, stderr) = run(ic_under_mosh("xterm-ghostty", &table)
        .arg("--will-display")
        .env("MOSH_IMAGES", MOSH_CARRIES_EVERY_PROTOCOL)
        .output()
        .expect("ic must run"));

    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(stdout, "", "success must print nothing to stdout");
    assert_eq!(stderr, "", "success must print nothing to stderr");
}

/// A pane of Zellij inside such a Mosh draws a picture as well.
///
/// This is the session the reporter of issue #471 ran. Zellij draws sixel, the
/// Mosh carries sixel, so the picture draws. The variable of Zellij is what
/// named the terminal and took the refusal.
#[test]
fn will_display_succeeds_in_a_zellij_pane_of_a_mosh_that_carries_images() {
    let table = MoshProcessTable::new(TARGET_NAME);
    let (code, _stdout, stderr) = run(ic_under_mosh("xterm-256color", &table)
        .arg("--will-display")
        .env("ZELLIJ", "0")
        .env("MOSH_IMAGES", MOSH_CARRIES_EVERY_PROTOCOL)
        .output()
        .expect("ic must run"));

    assert_eq!(code, Some(0), "stderr: {stderr}");
}

/// An upstream Mosh states no such variable, and it still takes the refusal
/// that names ssh.
///
/// Upstream Mosh strips every escape sequence that carries an image, so a
/// picture there leaves the user with an empty screen and no reason for it.
/// This is the one session that the whole rule protects, and the terminal here
/// names Ghostty, which is exactly the terminal that used to be refused for
/// the wrong reason.
#[test]
fn will_display_fails_and_names_ssh_for_an_upstream_mosh() {
    let table = MoshProcessTable::new(TARGET_NAME);
    let (code, _stdout, stderr) = run(ic_under_mosh("xterm-ghostty", &table)
        .arg("--will-display")
        .output()
        .expect("ic must run"));

    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("ssh user@host"),
        "stderr must name the repair that works: {stderr}"
    );
}

/// A session that shares no protocol with this terminal is refused, and the
/// message names every set it read.
///
/// A Mosh that carries sixel alone delivers nothing to a Kitty window, which
/// reads the kitty protocol and reads no other one. The repair is a different
/// terminal, so the reader needs to see which protocols each party of the
/// session draws.
#[test]
fn will_display_fails_and_names_both_sets_when_the_session_shares_no_protocol() {
    let table = MoshProcessTable::new(TARGET_NAME);
    let (code, _stdout, stderr) = run(ic_under_mosh(TERM_XTERM_KITTY, &table)
        .arg("--will-display")
        .env("MOSH_IMAGES", "sixel")
        .env("MOSH_CLIENT_IMAGES", "sixel")
        .output()
        .expect("ic must run"));

    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("sixel"),
        "stderr must name what the session carries: {stderr}"
    );
    assert!(
        stderr.contains("kitty"),
        "stderr must name what this terminal draws: {stderr}"
    );
    assert!(
        !stderr.contains("ssh user@host"),
        "and it must not send the reader to ssh, which carries no more protocols than this Mosh does: {stderr}"
    );
}
