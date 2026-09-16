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
use std::time::{Duration, Instant};

use crate::shell::{last_with_text, start_run, written_lines, ShellCommand, PROBE_POLL};

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
    run_with_deadline(shell, command, workdir, RUN_DEADLINE)
}

/// Run `command` in `workdir`, and stop waiting after `deadline`.
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
fn run_with_deadline(
    shell: &OsStr,
    command: &ShellCommand,
    workdir: &Path,
    deadline: Duration,
) -> IssueOutcome {
    let name = command.name();
    let mut run = match start_run(shell, command, workdir) {
        Ok(run) => run,
        // The shell is gone, it cannot be started, or there is nowhere to put
        // what it writes. Rare, and worth saying plainly: every other failure
        // here is the child's own words.
        Err(error) => {
            return IssueOutcome {
                message: Some(format!("cannot run {name}: {error}")),
            }
        }
    };

    let give_up_at = Instant::now() + deadline;
    loop {
        match run.child.try_wait() {
            Ok(Some(status)) => {
                let lines = written_lines(&run);
                return IssueOutcome::new(name, status.success(), &lines, &status.to_string());
            }
            Ok(None) => {
                if Instant::now() >= give_up_at {
                    // Read the files where they stand. A command that said why
                    // before it stopped answering is worth showing, and the
                    // timeout is worth saying either way.
                    let lines = written_lines(&run);
                    let outcome = IssueOutcome::unfinished(name, &lines, deadline);
                    let mut child = run.child;
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                    return outcome;
                }
                std::thread::sleep(PROBE_POLL);
            }
            // The child cannot be asked about, so nothing can be waited for
            // either. Saying so plainly is the same answer a shell that cannot
            // be started gets.
            Err(error) => {
                return IssueOutcome {
                    message: Some(format!("cannot run {name}: {error}")),
                }
            }
        }
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
    use crate::shell::stub_shell::{alive, kill_now, StubShell, GAVE_UP_WITHIN, HANG_DEADLINE};

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
        let started = Instant::now();
        let outcome = run_with_deadline(
            stub.as_shell(),
            &default_command(),
            workdir.path(),
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
        let outcome = run_with_deadline(
            stub.as_shell(),
            &default_command(),
            workdir.path(),
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
        let _ = run_with_deadline(
            stub.as_shell(),
            &default_command(),
            workdir.path(),
            HANG_DEADLINE,
        );
        let pid = stub.wait_for_pid();
        assert!(
            alive(pid),
            "the run must leave the user's own command running, and {pid} is gone",
        );
        kill_now(pid);
    }
}
