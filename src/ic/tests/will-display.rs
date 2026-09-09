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
//! **The terminal is part of that cleared environment.** A terminal of no name
//! and a pane of a multiplexer both send `ic` to ask the terminal which
//! protocol it draws, and that question goes to `/dev/tty`, which is the
//! terminal the test runner started the suite from. So every run of this file
//! starts in a session of its own: [`run`] gives the child no terminal at all,
//! and [`run_with_terminal`] gives it a pseudo-terminal that answers nothing and
//! that nobody types at. A run therefore reads no byte of the terminal of
//! whoever started the suite, and it writes none there either. The two helpers
//! keep that promise for the whole file, where each test once had to remember
//! it.
//!
//! # One test needs a terminal and the rest need none
//!
//! A pseudo-terminal belongs to the whole machine, which holds a few hundred of
//! them, and the tests of this file run beside each other. A file that opens one
//! for each test therefore asks for more of them than a busy machine has left.
//!
//! Only
//! [`will_display_asks_nothing_of_a_named_terminal_that_reports_no_pixel_size`]
//! reads what a run wrote to a terminal, and a test reads that of a terminal it
//! holds. Every other verdict of this file is an exit status and two pipes, and
//! none of them reads the size of a window: the flag asks for the protocol
//! alone, and the rule that decides whether to ask the terminal reads no window
//! for that question. So the rest of the tests run with no terminal, which
//! answers them exactly as a silent one does.
//!
//! # The flag asks a question and draws nothing
//!
//! `--will-display` reports what the terminal is, and it puts no picture on the
//! screen. So the one fact it needs is the protocol, and the size of a
//! character cell decides nothing it prints.
//! [`will_display_asks_nothing_of_a_named_terminal_that_reports_no_pixel_size`]
//! holds it to that: a run under a named terminal whose window reports no pixel
//! size must write nothing at all to the terminal.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

mod common;

use common::pty::{take_the_terminal_away, Pty, Window};
use common::{unreachable_path_dir, MoshProcessTable};

/// The name that this target puts in the unreachable `PATH` of its children.
const TARGET_NAME: &str = "will-display";

/// The window that the pseudo-terminal of [`run_with_terminal`] reports.
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

/// How long one turn of the runner waits for a byte of the terminal.
///
/// The runner reads the terminal and asks after the child in turn, and this is
/// the length of one turn. A run that asks writes its query inside the first
/// one, and [`Pty::read_until`] gives that query back the moment the request of
/// the attributes ends it.
///
/// **It is the floor of a turn as well as the ceiling.** [`Pty::read_until`]
/// gives the rest of the slice back the moment the needle arrives, and it gives
/// the whole of it back when the read of the terminal fails. The runner sleeps
/// out what such a read left, so a wait for the child costs no core of the
/// machine.
const READ_SLICE: Duration = Duration::from_millis(100);

/// How long the runner waits for `ic` to exit.
///
/// Ten seconds, which is generous because a loaded machine starts a process
/// late. **The wall clock is the bound, and a count of turns is not**: a count
/// is a deadline only while each turn costs what the count assumes, and a turn
/// that ends early then shortens the whole wait without saying so. A change
/// that stopped `ic` from exiting must fail a test instead of holding it, and
/// it must take the stated ten seconds to say so.
const EXIT_BUDGET: Duration = Duration::from_secs(10);

/// Invoke the freshly-built `ic` binary in a known-empty environment.
fn ic(term: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ic"));
    command.env_clear();
    command.env("PATH", unreachable_path_dir(TARGET_NAME));
    command.env("TERM", term);
    command
}

/// What one run of `ic` did.
struct Run {
    /// The exit status of the run, as a number.
    code: Option<i32>,
    /// Every byte that the run wrote to standard output.
    stdout: String,
    /// Every byte that the run wrote to standard error.
    stderr: String,
    /// Every byte that the run wrote to its terminal.
    wrote: Vec<u8>,
}

/// Run `command` with a pseudo-terminal of `window` as its controlling
/// terminal.
///
/// [`Pty::hand_to`] puts the child in a session of its own, which drops the
/// terminal the child inherited from the test runner and claims this one
/// instead. So the run asks this terminal, which answers nothing, and it
/// reaches no terminal of a person.
///
/// **The read of the terminal runs beside the wait for the child, and not
/// behind it.** A process that closes its controlling terminal waits for the
/// output queue of that terminal to drain, and the master end is where it
/// drains to. So a run that wrote a query the runner had not read yet would
/// hang on the way out, and a wait in front of the read would hold the two of
/// them there for good.
///
/// # Arguments
/// * `command` - The command to run. The caller states the environment and the
///   arguments, and this call states the terminal and the three standard
///   streams.
/// * `window` - The size that the pseudo-terminal reports.
///
/// # Returns
/// The exit status, the two captured streams, and every byte that the run wrote
/// to the terminal.
///
/// # Panics
/// Panics when `ic` does not start, and when it does not exit inside the
/// deadline that [`EXIT_BUDGET`] states.
fn run_with_terminal(command: &mut Command, window: Window) -> Run {
    let pty = Pty::open(window);

    // `spawn` inherits the streams of the test runner, where `output` gives a
    // pipe for each of them. So this states all three.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    pty.hand_to(command);

    let mut child = command.spawn().expect("ic must run");

    let deadline = Instant::now() + EXIT_BUDGET;
    let mut wrote = Vec::new();
    let mut exited = None;
    while Instant::now() < deadline {
        let slice = deadline
            .saturating_duration_since(Instant::now())
            .min(READ_SLICE);
        let turn = Instant::now();
        wrote.extend(pty.read_until(ATTRIBUTES_REQUEST, slice));
        if let Some(status) = child.try_wait().expect("failed to ask after ic") {
            exited = Some(status);
            break;
        }
        let spent = turn.elapsed();
        if spent < slice {
            thread::sleep(slice - spent);
        }
    }
    let Some(status) = exited else {
        // The child outlived the deadline, which is the failure this wait is
        // here to report. It ends first: a child that nobody kills and nobody
        // waits for holds a slot of the process table of the machine for as
        // long as the test runner lives.
        let _ = child.kill();
        let _ = child.wait();
        panic!("ic must exit inside the deadline of this file");
    };
    // The child has exited, so every byte it ever wrote to the terminal is
    // waiting there already. One last turn takes the bytes it wrote on its way
    // out.
    wrote.extend(pty.read_until(ATTRIBUTES_REQUEST, READ_SLICE));

    Run {
        code: status.code(),
        stdout: drain(child.stdout.take(), "stdout"),
        stderr: drain(child.stderr.take(), "stderr"),
        wrote,
    }
}

/// Take every byte that one pipe of the child holds.
///
/// The child has exited by the time this runs, so the whole of what it wrote is
/// waiting in the pipe and the read ends at once.
///
/// # Arguments
/// * `pipe` - The end of the pipe that the child wrote to.
/// * `name` - The name of that stream, for the message of a failure.
fn drain(pipe: Option<impl Read>, name: &str) -> String {
    let mut bytes = Vec::new();
    pipe.unwrap_or_else(|| panic!("the run must hold a pipe for {name}"))
        .read_to_end(&mut bytes)
        .unwrap_or_else(|error| panic!("failed to read the {name} of ic: {error}"));
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Run `command` with no terminal at all, and report the three answers that a
/// verdict is made of.
///
/// [`take_the_terminal_away`] puts the child in a session of its own and leaves
/// it no controlling terminal, which is the whole of the hermeticity that these
/// verdicts need. `ic` finds no `/dev/tty` to open, so it asks nothing and
/// answers with the name the environment carries — the answer a terminal that
/// stays silent gives it. This helper therefore opens no pseudo-terminal and
/// reads none, and `output` waits for the child and takes both pipes in one
/// call.
///
/// # Arguments
/// * `command` - The command to run. The caller states the environment and the
///   arguments, and this call states the session and the three standard
///   streams.
///
/// # Returns
/// The exit status and the two captured streams.
///
/// # Panics
/// Panics when `ic` does not start.
fn run(command: &mut Command) -> (Option<i32>, String, String) {
    take_the_terminal_away(command);
    let done = command.output().expect("ic must run");

    (
        done.status.code(),
        String::from_utf8_lossy(&done.stdout).into_owned(),
        String::from_utf8_lossy(&done.stderr).into_owned(),
    )
}

/// A terminal that renders graphics, with no multiplexer and no remote
/// transport. `ic` must report that it can display an image, and say nothing.
#[test]
fn will_display_succeeds_silently_in_a_graphics_capable_session() {
    let (code, stdout, stderr) = run(ic("xterm-256color").arg("--will-display"));

    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(stdout, "", "success must print nothing to stdout");
    assert_eq!(stderr, "", "success must print nothing to stderr");
}

/// The Linux console renders no graphics protocol. `ic` must fail with the
/// reason on stderr.
#[test]
fn will_display_fails_and_names_the_terminal_when_graphics_are_unsupported() {
    let (code, _stdout, stderr) = run(ic("linux").arg("--will-display"));

    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("not supported in this terminal"),
        "stderr must give the reason: {stderr}"
    );
}

/// The value that tmux writes into `TMUX` for a pane of its default socket.
///
/// The three fields name the socket, the process id of the server and the
/// number of the session. `ic` reads that the variable stands in the
/// environment, and it reads no field of it.
const TMUX_PANE: &str = "/private/tmp/tmux-501/default,1,0";

/// tmux strips the escape sequences that carry an image. `ic` must fail and
/// name tmux.
#[test]
fn will_display_fails_and_names_tmux() {
    let (code, _stdout, stderr) = run(ic("xterm-256color")
        .arg("--will-display")
        .env("TMUX", TMUX_PANE));

    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(stderr.contains("tmux"), "stderr must name tmux: {stderr}");
}

/// The flag asks a question. It does not display a file, so pairing it with a
/// file is a mistake `ic` must report instead of quietly ignoring one of them.
#[test]
fn will_display_refuses_to_share_the_command_with_a_file() {
    let (code, _stdout, stderr) = run(ic("xterm-256color").args(["--will-display", "picture.png"]));

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

/// A named terminal whose window reports no pixel size must be asked nothing.
///
/// `--will-display` reads the name of the terminal and whether that terminal
/// draws an image at all. It draws no picture, so it needs no character cell,
/// and the round trip that measures one costs the budget of the query and
/// swallows whatever the user typed while it drained the terminal in raw mode.
#[test]
fn will_display_asks_nothing_of_a_named_terminal_that_reports_no_pixel_size() {
    let outcome = run_with_terminal(ic(TERM_XTERM_KITTY).arg("--will-display"), WINDOW);

    assert!(
        outcome.wrote.is_empty(),
        "--will-display must write nothing to the terminal of a named session, and it wrote {:?}",
        String::from_utf8_lossy(&outcome.wrote)
    );
    assert_eq!(
        outcome.code,
        Some(0),
        "and it must still report that a Kitty terminal displays an image: {}",
        outcome.stderr
    );
    assert_eq!(
        outcome.stdout, "",
        "and success must print nothing to stdout, as it does for every other terminal here"
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
/// This is the command of [`ic`], with the `PATH` of `table` in place of the
/// unreachable one. That `PATH` reaches the stated `ps` of `table` and reaches
/// nothing else, so the transport comes from the table and not from the machine
/// of whoever runs the suite. Every other test of this file points the `PATH` at
/// a directory that does not exist, which is the same rule read the other way: a
/// test states the process tree it covers, and it reads none.
fn ic_under_mosh(term: &str, table: &MoshProcessTable) -> Command {
    let mut command = ic(term);
    command.env("PATH", table.path());
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
        .env("MOSH_IMAGES", MOSH_CARRIES_EVERY_PROTOCOL));

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
        .env("MOSH_IMAGES", MOSH_CARRIES_EVERY_PROTOCOL));

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
    let (code, _stdout, stderr) = run(ic_under_mosh("xterm-ghostty", &table).arg("--will-display"));

    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("ssh user@host"),
        "stderr must name the repair that works: {stderr}"
    );
}

/// A session that shares no protocol with this terminal is refused, and the
/// message names every set that decided it.
///
/// A Mosh that carries sixel alone delivers nothing to a Kitty window, which
/// reads the kitty protocol and reads no other one. This run states
/// `MOSH_CLIENT_IMAGES` as well, so three sets decided the refusal and the
/// message names the three of them. The repair is a different terminal, so the
/// reader needs to see which protocols each party of the session draws.
#[test]
fn will_display_fails_and_names_both_sets_when_the_session_shares_no_protocol() {
    let table = MoshProcessTable::new(TARGET_NAME);
    let (code, _stdout, stderr) = run(ic_under_mosh(TERM_XTERM_KITTY, &table)
        .arg("--will-display")
        .env("MOSH_IMAGES", "sixel")
        .env("MOSH_CLIENT_IMAGES", "sixel"));

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

/// A session that names no terminal of the user is refused, and the message
/// names the two sets that decided it.
///
/// This run carries no `MOSH_CLIENT_IMAGES`, which is the session that
/// `mosh-server new` starts by hand: the server states what it carries, and no
/// wrapper stated what the terminal of the user draws. An absent name is no
/// name of an empty set, so `ic` reads the transport alone and this Kitty
/// window draws no sixel. A message that named the absent set would state that
/// the terminal of the user draws `none`, beside a refusal that never read it.
#[test]
fn will_display_fails_and_names_two_sets_when_the_session_names_no_terminal_of_the_user() {
    let table = MoshProcessTable::new(TARGET_NAME);
    let (code, _stdout, stderr) = run(ic_under_mosh(TERM_XTERM_KITTY, &table)
        .arg("--will-display")
        .env("MOSH_IMAGES", "sixel"));

    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("sixel"),
        "stderr must name what this Mosh carries: {stderr}"
    );
    assert!(
        stderr.contains("kitty"),
        "stderr must name what this terminal draws: {stderr}"
    );
    assert!(
        !stderr.contains("The terminal of the user draws: none"),
        "and it must not state that the terminal of the user draws none, which is a set this refusal never read: {stderr}"
    );
}

/// A pane of tmux inside such a Mosh still takes the refusal that names tmux.
///
/// `MOSH_IMAGES` states what the transport carries, and tmux stands between
/// that transport and the screen. A tmux that draws no image strips every
/// sequence that carries one, and the variable says nothing about this tmux:
/// the shell that starts the tmux server hands the whole environment to the
/// server, and the server hands it to every pane. So a Mosh that carries
/// images lifts the refusal of Mosh alone, and every rule under it still runs.
#[test]
fn will_display_fails_and_names_tmux_inside_a_mosh_that_carries_images() {
    let table = MoshProcessTable::new(TARGET_NAME);
    let (code, _stdout, stderr) = run(ic_under_mosh("tmux-256color", &table)
        .arg("--will-display")
        .env("TMUX", TMUX_PANE)
        .env("MOSH_IMAGES", MOSH_CARRIES_EVERY_PROTOCOL));

    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(stderr.contains("tmux"), "stderr must name tmux: {stderr}");
}

/// A `MOSH_IMAGES` that outlived its session states nothing, and the terminal
/// answers for the run.
///
/// The variable crosses a multiplexer, which is what makes it useful, and that
/// is what makes it stale as well. A tmux server or a Zellij server that a Mosh
/// session started keeps the whole environment of that session, and it hands
/// that environment to every pane it opens after the Mosh session ends. This
/// run states no process table, so the `PATH` reaches no `ps` and the process
/// tree names no Mosh. Ghostty draws a picture, and the variable names sixel
/// alone, so the run must report the terminal and not the variable.
#[test]
fn will_display_succeeds_for_a_named_terminal_with_a_stale_mosh_variable() {
    let (code, stdout, stderr) = run(ic("xterm-ghostty")
        .arg("--will-display")
        .env("MOSH_IMAGES", "sixel"));

    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(stdout, "", "success must print nothing to stdout");
    assert_eq!(stderr, "", "success must print nothing to stderr");
}
