//! Putting the number of the pull request of the branch on the tab title, from
//! watch mode.
//!
//! The `T` key runs one command in the user's own interactive shell, with the
//! worktree on the screen as its current directory. The command is usually a
//! shell function (`stp`), so only a shell can find it and only a shell can run
//! it. [`crate::shell`] asks the shell both questions, and it starts the child
//! for this key and for every other key that runs a command the user supplies.
//!
//! What is here is what belongs to `T` alone: the variable that names its
//! command, the name it falls back on, how long a press gets, and the words a
//! finished press leaves under the frame.

use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use crate::shell::{
    cannot_run_line, failure_line, last_with_text, run_until, unfinished_line, Ended, Ran,
    ShellCommand,
};

/// The variable that holds the command `T` runs.
///
/// The value is a whole command line, as the value of
/// [`crate::issue::ISSUE_COMMAND_ENV`] is. gsw adds no argument to it.
pub(crate) const TAB_COMMAND_ENV: &str = "GSW_TAB_COMMAND";

/// The command `T` runs when the environment names none.
///
/// This repository ships no `stp`. It is a shell function that the user
/// supplies. It puts the number of the open pull request of the branch at the
/// front of the zellij tab title, and it keeps the rest of the title.
/// [`TAB_COMMAND_ENV`] names a different one.
pub(crate) const DEFAULT_TAB_COMMAND: &str = "stp";

/// How long a press of `T` gets.
///
/// A press of `T` gets the minute that a press of `G` gets, for the same
/// reasons. `stp` asks `gh` about the pull request over the network, and one
/// run at a time is the rule, so a run that never ends must not hold the key
/// for the session.
pub(crate) const RUN_DEADLINE: Duration = crate::issue::RUN_DEADLINE;

/// What a finished press of `T` leaves under the frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TabOutcome {
    /// The command exited 0. The last line with text that it wrote, or `None`
    /// where it wrote nothing. The line is a report, so it fades.
    Worked(Option<String>),
    /// The command exited non-zero, could not start, or was still running at
    /// the deadline. The line says why, and it waits for a key.
    Failed(String),
}

/// Run `command` in `workdir`, and say how it ended.
///
/// Blocking: the caller runs it on a thread of its own.
pub(crate) fn run(shell: &OsStr, command: &ShellCommand, workdir: &Path) -> TabOutcome {
    run_in(shell, command, workdir, &std::env::temp_dir(), RUN_DEADLINE)
}

/// Run `command` in `workdir`, with the two files of the run in `scratch`, and
/// stop waiting after `deadline`.
///
/// Four rules decide what the press reports:
///
/// - A run that exited 0 reports the last line with text that it wrote, or
///   nothing where it wrote nothing.
/// - A run that failed reports its last line with text, or its name and exit
///   status where it wrote nothing.
/// - A run that was still running at `deadline` reports the timeout. gsw kills
///   nothing, because the command is the user's own.
/// - A run that cannot start reports why.
///
/// The directory is a parameter so a test can watch it. Production passes
/// [`std::env::temp_dir`].
fn run_in(
    shell: &OsStr,
    command: &ShellCommand,
    workdir: &Path,
    scratch: &Path,
    deadline: Duration,
) -> TabOutcome {
    let name = command.name();
    match run_until(shell, command, workdir, scratch, deadline) {
        Ok(Ran {
            ended: Ended::Exited(status),
            lines,
        }) if status.success() => {
            TabOutcome::Worked(last_with_text(lines.iter().map(String::as_str)))
        }
        Ok(Ran {
            ended: Ended::Exited(status),
            lines,
        }) => TabOutcome::Failed(failure_line(name, &lines, &status.to_string())),
        Ok(Ran {
            ended: Ended::StillRunning,
            lines,
        }) => TabOutcome::Failed(unfinished_line(name, &lines, deadline)),
        Err(error) => TabOutcome::Failed(cannot_run_line(name, &error)),
    }
}

#[cfg(all(test, unix))]
mod run_tests {
    use super::*;
    use crate::shell::stub_shell::{alive, kill_now, StubShell, GAVE_UP_WITHIN, HANG_DEADLINE};
    use std::time::Instant;

    /// The default command, which is what most tests here run.
    fn stp() -> ShellCommand {
        ShellCommand::new(None, DEFAULT_TAB_COMMAND).expect("the default names a command")
    }

    #[test]
    fn a_run_that_cannot_start_says_why() {
        // A shell that is gone must not look like an unbound key. A press that
        // does nothing and says nothing reads as a key that is not bound, so
        // the run says that it could not start.
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("no-such-shell");
        let workdir = tempfile::tempdir().expect("tempdir");
        let outcome = run(missing.as_os_str(), &stp(), workdir.path());
        let TabOutcome::Failed(line) = &outcome else {
            panic!("expected a failure, got {outcome:?}");
        };
        let expected = format!("cannot run {DEFAULT_TAB_COMMAND}: ");
        assert!(line.starts_with(&expected), "got {line:?}");
    }

    #[test]
    fn the_run_is_interactive_and_runs_in_the_worktree_on_the_screen() {
        // `stp` is a function of the rc file, so only an interactive shell
        // finds it. It also reads the branch of the directory it runs in, so
        // the directory must be the worktree on the screen and not the one
        // that gsw started in.
        let stub = StubShell::answering(0);
        let workdir = tempfile::tempdir().expect("tempdir");
        let _ = run(stub.as_shell(), &stp(), workdir.path());
        let runs = stub.runs();
        assert!(runs.lines().any(|line| line == "-ic"), "got {runs:?}");
        assert!(
            runs.lines().any(|line| line == DEFAULT_TAB_COMMAND),
            "got {runs:?}"
        );
        let cwds: Vec<_> = stub
            .cwds()
            .iter()
            .map(|dir| dir.canonicalize().expect("canonical cwd"))
            .collect();
        assert_eq!(
            cwds,
            [workdir.path().canonicalize().expect("canonical workdir")],
            "one run, in the worktree on the screen",
        );
    }

    #[test]
    fn a_whole_command_line_runs_whole() {
        // The value of the variable is a command line. gsw adds no argument
        // and splits nothing, so the shell sees exactly what the user wrote.
        let stub = StubShell::answering(0);
        let workdir = tempfile::tempdir().expect("tempdir");
        let command =
            ShellCommand::new(Some("stp 57"), DEFAULT_TAB_COMMAND).expect("a command line");
        let _ = run(stub.as_shell(), &command, workdir.path());
        let runs = stub.runs();
        assert!(runs.lines().any(|line| line == "stp 57"), "got {runs:?}");
    }

    #[test]
    fn a_run_that_worked_shows_the_last_line_it_wrote_to_stderr() {
        // `stp` writes its notes on standard error only. A note such as "the
        // tab already starts with 57" is the whole answer of a run that had
        // nothing to change, so it must reach the screen.
        let stub = StubShell::new("echo 'stp: the tab already starts with 57' >&2\nexit 0");
        let workdir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            run(stub.as_shell(), &stp(), workdir.path()),
            TabOutcome::Worked(Some("stp: the tab already starts with 57".into())),
        );
    }

    #[test]
    fn a_run_that_worked_and_wrote_nothing_says_nothing() {
        // A blank row under the frame would claim a report that does not
        // exist. Silence stays silence.
        let stub = StubShell::answering(0);
        let workdir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            run(stub.as_shell(), &stp(), workdir.path()),
            TabOutcome::Worked(None),
        );
    }

    #[test]
    fn a_title_escape_on_standard_output_is_not_a_line() {
        // In tmux and in a bare terminal, `st` sets the title with an escape
        // sequence on its standard output. gsw reads that output from a file,
        // and `crate::lines` removes every escape sequence and drops a line
        // that becomes empty. So gsw never forwards the sequence, and it posts
        // nothing. This test pins that limit.
        let stub = StubShell::new("printf '\\033]2;57 issue-620\\007'\nexit 0");
        let workdir = tempfile::tempdir().expect("tempdir");
        let outcome = run(stub.as_shell(), &stp(), workdir.path());
        assert!(
            stub.runs().lines().any(|line| line == "-ic"),
            "the stub must have run",
        );
        assert_eq!(outcome, TabOutcome::Worked(None));
    }

    #[test]
    fn a_run_that_failed_shows_its_last_line() {
        // The last line is the reason. `stp` says why it stopped after it
        // says what it looked at.
        let stub = StubShell::new(
            "echo 'looking' >&2\necho 'stp: no open pull request for issue-620' >&2\nexit 1",
        );
        let workdir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            run(stub.as_shell(), &stp(), workdir.path()),
            TabOutcome::Failed("stp: no open pull request for issue-620".into()),
        );
    }

    #[test]
    fn a_run_that_failed_in_silence_names_the_exit_status() {
        // A blank row would read as success, so the status is all that is
        // left to report.
        let stub = StubShell::answering(1);
        let workdir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            run(stub.as_shell(), &stp(), workdir.path()),
            TabOutcome::Failed("stp failed (exit status: 1)".into()),
        );
    }

    #[test]
    fn a_run_past_the_deadline_says_so_and_leaves_the_command_running() {
        // One run at a time is the rule, so a run that never ends holds the
        // key for the session. gsw stops waiting at the deadline, and it kills
        // nothing, because the command is the user's own.
        let stub = StubShell::hanging();
        let workdir = tempfile::tempdir().expect("tempdir");
        let scratch = tempfile::tempdir().expect("tempdir");
        let started = Instant::now();
        let outcome = run_in(
            stub.as_shell(),
            &stp(),
            workdir.path(),
            scratch.path(),
            HANG_DEADLINE,
        );
        assert!(
            started.elapsed() < GAVE_UP_WITHIN,
            "the run must give up at its deadline rather than wait for the command",
        );
        assert_eq!(
            outcome,
            TabOutcome::Failed(format!(
                "stp has not finished after {}s",
                HANG_DEADLINE.as_secs()
            )),
        );
        let pid = stub.wait_for_pid();
        assert!(alive(pid), "gsw must leave the command running");
        kill_now(pid);
    }
}
