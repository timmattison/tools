//! Opening the issue that the branch names, from watch mode.
//!
//! The `G` key of watch mode runs one command in the user's own interactive
//! shell. That command is usually a shell function, so only a shell can find
//! it and only a shell can run it. [`crate::shell`] asks the shell both
//! questions, and it starts the child — for this key and for every other key
//! that runs a command the user supplies.
//!
//! What is here is what belongs to `G` alone: the variable that names its
//! command, the name it falls back on, how long a run gets, and the words a
//! finished run leaves under the frame.

use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use crate::shell::{last_with_text, run_command, start_run, OutputStream, RunEnd, ShellCommand};

/// The variable that holds the command `G` runs.
///
/// The value is a whole command line, so it can carry arguments. See
/// [`ShellCommand`] for what gsw does with each part of it.
pub(crate) const ISSUE_COMMAND_ENV: &str = "GSW_ISSUE_COMMAND";

/// The command `G` runs when the environment names none.
///
/// This repository ships no `ggs`. It is a shell function that the user
/// supplies, and it is the default here because it is the name that the plans
/// of this repository are written with. [`ISSUE_COMMAND_ENV`] names a
/// different one.
pub(crate) const DEFAULT_ISSUE_COMMAND: &str = "ggs";

/// What a finished run of the issue command leaves under the frame.
///
/// `None` is a run that worked. The browser is the answer, and a monitor that
/// also posted a line would spend a row of the frame saying what the user is
/// already looking at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IssueOutcome {
    message: Option<String>,
}

impl IssueOutcome {
    /// The outcome of a run of `name` that wrote `lines` and ended the way
    /// `status` reads.
    ///
    /// A failure says the last line that has text in it. That text comes from
    /// another program: `ggs` refuses with exit status 2 on a branch that
    /// names no issue, and that refusal is the whole reason the key did
    /// nothing. A failure that wrote nothing has only the status left to
    /// report, and a blank row under the frame would read as success.
    pub(crate) fn new(name: &str, success: bool, lines: &[String], status: &str) -> Self {
        if success {
            return Self { message: None };
        }
        Self {
            message: Some(
                last_with_text(lines.iter().map(String::as_str))
                    .unwrap_or_else(|| format!("{name} failed ({status})")),
            ),
        }
    }

    /// The outcome of a run of `name` that `lines` came from and that was
    /// still running at `deadline`.
    ///
    /// The message names the timeout every time, because the timeout is the
    /// thing the user cannot see: the run goes on, the key is free again, and
    /// no page opened. A row that said only what the command wrote would read
    /// as a run that stopped.
    ///
    /// A command that said why before it stopped answering keeps its words,
    /// after the timeout. `gh` reports a login it needs and then waits for an
    /// answer nobody can give it, and that report is the whole reason the run
    /// went nowhere.
    pub(crate) fn unfinished(name: &str, lines: &[String], deadline: Duration) -> Self {
        let waited = format!("{name} has not finished after {}s", deadline.as_secs());
        Self {
            message: Some(match last_with_text(lines.iter().map(String::as_str)) {
                Some(said) => format!("{waited}: {said}"),
                None => waited,
            }),
        }
    }

    /// The message to put under the frame, or `None` where the run says
    /// nothing.
    pub(crate) fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

/// How long a run waits for the command to finish.
///
/// The probe asks a shell a question it answers out of memory, so
/// [`crate::shell::PROBE_DEADLINE`] is small. A run is the user's own work:
/// `gh` asks a server about the issue, and the browser that opens the page
/// takes a moment of its own. So this number is much larger.
///
/// Sixty seconds is the trade. A shorter deadline reports a slow fetch over a
/// slow network as a run that stopped, which is a lie about a run that goes on
/// to open the page. A longer one holds the key: one run at a time is the
/// rule, so the key means nothing while a run is open, and a user reads a key
/// that does nothing as a key with nothing behind it. Sixty seconds is longer
/// than a slow fetch and shorter than a session.
const RUN_DEADLINE: Duration = Duration::from_secs(60);

/// Run `command` in `workdir` and report what to say about it.
///
/// Blocking: the caller runs it on a thread of its own.
pub(crate) fn run(shell: &OsStr, command: &ShellCommand, workdir: &Path) -> IssueOutcome {
    run_in(shell, command, workdir, &std::env::temp_dir(), RUN_DEADLINE)
}

/// Run `command` in `workdir`, with the two files of the run in `scratch`, and
/// stop waiting after `deadline`.
///
/// The directory is a parameter so a test can watch it. Production passes
/// [`std::env::temp_dir`].
///
/// **At the deadline gsw stops waiting, and it kills nothing.** This is the one
/// place where a run and the probe part company, and the reason is whose
/// process it is. The probe's child is gsw's own question, so gsw ends that
/// child and the whole process group under it. This
/// child is the user's own command, and a command that holds a browser in the
/// foreground is the shape this deadline exists for — `xdg-open` does it. To
/// kill that process group is to close the page the user asked gsw to open.
///
/// The user's rc file is bounded already, one layer up: the probe runs
/// `$SHELL -ic 'command -v <name>'`, which loads that same rc file under
/// [`crate::shell::PROBE_DEADLINE`]. So a run that hangs hangs in the command,
/// and the command is the user's to end.
///
/// The child still has to be reaped. A dropped [`std::process::Child`] is
/// neither killed nor waited for, and a child nobody waits for stays as a
/// defunct entry for the life of the session — which is the cost this deadline
/// exists to avoid. So the child goes to a thread that waits for it. That
/// thread ends when the child ends, so the child is what bounds it.
fn run_in(
    shell: &OsStr,
    command: &ShellCommand,
    workdir: &Path,
    scratch: &Path,
    deadline: Duration,
) -> IssueOutcome {
    let name = command.name();
    // The shell is gone, it cannot be started, there is nowhere to put what it
    // writes, or it cannot be asked about. Rare, and worth saying plainly:
    // every other failure here is the child's own words.
    let cannot_run = |error: std::io::Error| IssueOutcome {
        message: Some(format!("cannot run {name}: {error}")),
    };
    let run = match start_run(run_command(shell, command, workdir), scratch) {
        Ok(run) => run,
        Err(error) => return cannot_run(error),
    };

    // Standard output first, then standard error, whatever order the lines
    // arrived in. That is the order a refusal reads in: a command says what it
    // did on one stream and why it stopped on the other.
    let mut lines = Vec::new();
    let mut reasons = Vec::new();
    let end = run.wait(Some(deadline), &mut |stream, line| match stream {
        OutputStream::Stdout => lines.push(line),
        OutputStream::Stderr => reasons.push(line),
    });
    lines.append(&mut reasons);

    match end {
        Ok(RunEnd::Exited(status)) => {
            IssueOutcome::new(name, status.success(), &lines, &status.to_string())
        }
        // A command that said why before it stopped answering is worth
        // showing, and the timeout is worth saying either way.
        Ok(RunEnd::StillRunning(mut child)) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            IssueOutcome::unfinished(name, &lines, deadline)
        }
        Err(error) => cannot_run(error),
    }
}

#[cfg(test)]
mod outcome_tests {
    use super::*;

    /// The lines of `text`, the way the runner splits what a child wrote.
    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn a_run_that_worked_says_nothing() {
        // The browser is the answer.
        let outcome = IssueOutcome::new("ggs", true, &lines("opened\n"), "exit status: 0");
        assert_eq!(outcome.message(), None);
    }

    #[test]
    fn a_run_that_failed_says_the_last_line_the_child_wrote() {
        // `ggs` refuses on a branch that names no issue, and that refusal is
        // the whole reason the key did nothing.
        let outcome = IssueOutcome::new(
            "ggs",
            false,
            &lines("looking\nbranch main names no issue\n"),
            "exit status: 2",
        );
        assert_eq!(outcome.message(), Some("branch main names no issue"));
    }

    #[test]
    fn a_run_that_failed_ignores_the_empty_lines_after_its_last_word() {
        let outcome = IssueOutcome::new("ggs", false, &lines("no issue\n\n   \n"), "exit: 2");
        assert_eq!(outcome.message(), Some("no issue"));
    }

    #[test]
    fn a_run_that_failed_in_silence_names_the_exit_status() {
        // A blank row under the frame reads as success.
        let outcome = IssueOutcome::new("ggs", false, &[], "exit status: 2");
        assert_eq!(outcome.message(), Some("ggs failed (exit status: 2)"));
    }
}

#[cfg(all(test, unix))]
mod run_tests {
    use super::*;
    use crate::shell::stub_shell::{
        alive, entries_of, kill_now, StubShell, GAVE_UP_WITHIN, HANG_DEADLINE,
    };
    use std::sync::mpsc::channel;
    use std::time::Instant;

    /// The default command, which is what every test here runs.
    fn default_command() -> ShellCommand {
        ShellCommand::new(None, DEFAULT_ISSUE_COMMAND).expect("the default names a command")
    }

    #[test]
    fn a_run_that_worked_leaves_no_message() {
        let stub = StubShell::answering(0);
        let workdir = tempfile::tempdir().expect("tempdir");
        let outcome = run(stub.as_shell(), &default_command(), workdir.path());
        assert_eq!(outcome.message(), None, "the browser is the answer");
        let runs = stub.runs();
        assert!(
            runs.contains("-ic"),
            "the run must be interactive, or the shell has no functions: {runs:?}",
        );
    }

    #[test]
    fn a_run_that_failed_puts_what_the_child_said_under_the_frame() {
        let stub = StubShell::new("echo 'branch main names no issue' >&2\nexit 2");
        let workdir = tempfile::tempdir().expect("tempdir");
        let outcome = run(stub.as_shell(), &default_command(), workdir.path());
        assert_eq!(
            outcome.message(),
            Some("branch main names no issue"),
            "a refusal must reach the screen",
        );
    }

    #[test]
    fn a_run_that_never_finishes_reports_the_timeout() {
        // One run at a time is the rule, so a run that never ends holds the
        // key for the life of the session. Silence is the state the design
        // keeps for a command that does not exist, so a user cannot tell a
        // stuck run from an unbound key. [`StubShell::new`] warms the script,
        // so the bound below measures this code and not the first start of a
        // file this process just wrote.
        let stub = StubShell::hanging();
        let workdir = tempfile::tempdir().expect("tempdir");
        let scratch = tempfile::tempdir().expect("tempdir");
        let started = Instant::now();
        let outcome = run_in(
            stub.as_shell(),
            &default_command(),
            workdir.path(),
            scratch.path(),
            HANG_DEADLINE,
        );
        assert!(
            started.elapsed() < GAVE_UP_WITHIN,
            "the run must give up at its deadline rather than wait for the command",
        );
        let message = outcome
            .message()
            .expect("a run that never finishes must say so");
        assert!(
            message.contains("ggs"),
            "the message must name the command: {message:?}",
        );
        assert!(
            message.contains("has not finished"),
            "the message must name the timeout: {message:?}",
        );
        kill_now(stub.wait_for_pid());
    }

    #[test]
    fn a_run_that_never_finishes_carries_what_the_command_said_first() {
        // A command that says why and then hangs is worth reading. The two
        // files hold those words, and the deadline reads them where they are.
        let stub = StubShell::hanging_after_saying("waiting for the server");
        let workdir = tempfile::tempdir().expect("tempdir");
        let scratch = tempfile::tempdir().expect("tempdir");
        let outcome = run_in(
            stub.as_shell(),
            &default_command(),
            workdir.path(),
            scratch.path(),
            HANG_DEADLINE,
        );
        let message = outcome
            .message()
            .expect("a run that never finishes must say so");
        assert!(
            message.contains("waiting for the server"),
            "the words the command wrote before it hung must reach the screen: {message:?}",
        );
        assert!(
            message.contains("has not finished"),
            "the message must still name the timeout: {message:?}",
        );
        kill_now(stub.wait_for_pid());
    }

    #[test]
    fn a_run_that_never_finishes_leaves_the_command_running() {
        // gsw stops waiting. It does not stop the command. The probe's child
        // is gsw's own question, so gsw kills that one, but this child is the
        // user's own command — a command that holds a browser in the
        // foreground is the usual shape, and killing it closes the page the
        // user asked for.
        let stub = StubShell::hanging();
        let workdir = tempfile::tempdir().expect("tempdir");
        let scratch = tempfile::tempdir().expect("tempdir");
        let _ = run_in(
            stub.as_shell(),
            &default_command(),
            workdir.path(),
            scratch.path(),
            HANG_DEADLINE,
        );
        let pid = stub.wait_for_pid();
        assert!(
            alive(pid),
            "the run must leave the user's own command running, and {pid} is gone",
        );
        kill_now(pid);
    }

    #[test]
    fn no_file_of_a_run_keeps_a_name_while_the_run_is_in_flight_or_after_it() {
        // **A quit during a run kills the thread of that run where it stands,
        // and a thread that dies runs no destructor.** So a file that still
        // carries a name stays in the temporary directory for good. The base
        // update holds the same rule, and this holds it for `G`.
        //
        // The directory is this test's own, so what it reads is the two files
        // of this run and nothing else on the machine.
        let stub = StubShell::saying_then_waiting_for_a_gate("looking up the issue");
        let workdir = tempfile::tempdir().expect("tempdir");
        let scratch = tempfile::tempdir().expect("tempdir");
        let shell = stub.as_shell().to_os_string();
        let dir = workdir.path().to_path_buf();
        let scratch_path = scratch.path().to_path_buf();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let outcome = run_in(
                &shell,
                &default_command(),
                &dir,
                &scratch_path,
                RUN_DEADLINE,
            );
            let _ = tx.send(outcome);
        });

        // A record of the run says the child is running, so the two files of
        // this run exist by now.
        let started = stub.wait_for_a_run();
        let in_flight = entries_of(scratch.path());
        // The gate is opened before the assertions, so the stub ends whatever
        // this test does next.
        stub.open_gate();
        assert!(started, "the stub never started, so no run was in flight");
        assert!(
            in_flight.is_empty(),
            "a file of a run in flight keeps a name, so a quit leaves it behind for good: \
             {in_flight:?}",
        );

        let outcome = rx
            .recv_timeout(GAVE_UP_WITHIN)
            .expect("the run must end once the gate is open");
        assert_eq!(
            outcome.message(),
            None,
            "the stub exits 0 once the gate is open",
        );
        let afterwards = entries_of(scratch.path());
        assert!(
            afterwards.is_empty(),
            "no file may outlive the run: {afterwards:?}",
        );
    }
}
