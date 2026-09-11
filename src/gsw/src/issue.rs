//! Opening the issue that the branch names, from watch mode.
//!
//! The `G` key of watch mode runs one command in the user's own interactive
//! shell. That command is a shell function, so only a shell can find it and
//! only a shell can run it. This module asks the shell both questions.

use std::ffi::{OsStr, OsString};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::child::detach_from_terminal;

/// The variable that names the command `G` runs.
pub(crate) const ISSUE_COMMAND_ENV: &str = "GSW_ISSUE_COMMAND";

/// The command `G` runs when the environment names none.
///
/// This repository ships no `ggs`. It is a shell function that the user
/// supplies, and it is the default here because it is the name that the plans
/// of this repository are written with. [`ISSUE_COMMAND_ENV`] names a
/// different one.
const DEFAULT_ISSUE_COMMAND: &str = "ggs";

/// The command that `G` runs.
///
/// A newtype rather than a `String`, because the value holds one rule that
/// every reader of it depends on: it is never empty. An empty name asks the
/// shell about nothing, and it runs nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IssueCommand(String);

impl IssueCommand {
    /// The command that `value` names, or `None` where the feature is off.
    ///
    /// `value` is the value of [`ISSUE_COMMAND_ENV`], which the caller reads.
    /// The environment is process-global state, and this function takes the
    /// value as an argument so a test of it touches no such state.
    ///
    /// An absent value gives [`DEFAULT_ISSUE_COMMAND`]. A value with nothing
    /// but space in it turns the feature off, which is the one way to say "do
    /// not do this at all" on a public repository whose default names one
    /// person's shell function.
    pub(crate) fn new(value: Option<&str>) -> Option<Self> {
        match value {
            None => Some(Self(DEFAULT_ISSUE_COMMAND.to_string())),
            Some(named) => {
                let named = named.trim();
                (!named.is_empty()).then(|| Self(named.to_string()))
            }
        }
    }

    /// The name of the command, which is never empty.
    pub(crate) fn name(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name that `value` resolves to, as a plain string, or `None`.
    fn resolved(value: Option<&str>) -> Option<String> {
        IssueCommand::new(value).map(|command| command.name().to_string())
    }

    #[test]
    fn an_absent_variable_names_the_default_command() {
        assert_eq!(
            resolved(None),
            Some(DEFAULT_ISSUE_COMMAND.to_string()),
            "an unset GSW_ISSUE_COMMAND must give the default name",
        );
    }

    #[test]
    fn a_variable_with_a_name_in_it_names_that_command() {
        assert_eq!(
            resolved(Some("myfunc")),
            Some("myfunc".to_string()),
            "GSW_ISSUE_COMMAND must name the command that G runs",
        );
    }

    #[test]
    fn an_empty_variable_turns_the_feature_off() {
        // The one way to say "do not do this at all". A public repository must
        // not make one person's shell function a constant with no way out.
        assert_eq!(resolved(Some("")), None, "an empty value must turn G off");
        assert_eq!(
            resolved(Some("   ")),
            None,
            "a value with nothing but space in it must turn G off",
        );
    }

    #[test]
    fn the_space_around_a_name_is_dropped() {
        // A variable written in an rc file collects space. The name inside it
        // is still the name.
        assert_eq!(resolved(Some("  ggs  ")), Some("ggs".to_string()));
    }
}

/// How long the probe waits for the shell to answer.
///
/// An rc file is somebody else's code, and one that hangs must not leave a
/// process behind for the life of the session. At the deadline the probe kills
/// the child and reports the command absent, which is the same answer as a
/// shell that said no.
const PROBE_DEADLINE: Duration = Duration::from_secs(5);

/// How often the probe looks to see whether the shell has answered.
///
/// The probe runs once for each `gsw` process, on a thread that does nothing
/// else, so the cost of looking is paid once and it delays no frame.
const PROBE_POLL: Duration = Duration::from_millis(25);

/// The shell to ask, from `SHELL`, and `/bin/sh` where the variable is unset.
///
/// `nwt` makes the same choice, for the same reason: `SHELL` is set by
/// `login`, by `sshd`, and by every terminal program, so a session without it
/// is a session that has no shell functions to find either.
fn user_shell() -> OsString {
    std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"))
}

/// The variables that aim a git command at a repository.
///
/// The command the user supplies asks `gh` about the issue, and `gh` reads the
/// origin remote of the current directory. An inherited `GIT_DIR` aims that
/// question at another repository. A pre-commit hook exports all three, so a
/// `gsw` started from inside one would carry them into the child.
const GIT_LOCATION_VARS: [&str; 3] = ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"];

/// `value` as one word of a shell script.
///
/// Single quotes stop the shell from reading anything inside them, and an
/// embedded single quote ends the quoting, adds an escaped quote, and starts
/// the quoting again.
fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// A child that runs `script` in an interactive `shell`.
///
/// Interactive is the load-bearing half. The command is a shell function, and
/// a function lives only in a shell that read the rc file, which only `-i`
/// makes a shell do.
///
/// Two rules apply to every child this module starts. It is detached from the
/// terminal, because an interactive shell opens `/dev/tty` and takes the
/// keyboard that `gsw` is reading. Denied a terminal, both zsh and bash turn
/// job control off and start anyway. And it carries no [`GIT_LOCATION_VARS`].
fn shell_child(shell: &OsStr, script: String) -> Command {
    let mut command = Command::new(shell);
    command.arg("-ic").arg(script);
    for name in GIT_LOCATION_VARS {
        command.env_remove(name);
    }
    detach_from_terminal(&mut command);
    command
}

/// The child that asks `shell` whether `command` exists.
///
/// `command -v` reports a function and an alias in both bash and zsh, which is
/// what makes this the right question: the thing being looked for is usually
/// neither a file nor a builtin.
fn probe_command(shell: &OsStr, command: &IssueCommand) -> Command {
    let mut child = shell_child(shell, format!("command -v {}", shell_escape(command.name())));
    // Nothing the probe says belongs on the screen. An rc file that prints a
    // banner would otherwise paint over the frame.
    child
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    child
}

/// Whether `shell` has `command`, giving up after `deadline`.
///
/// Exit status 0 means the command exists. Every other status, a shell that
/// cannot be started, and a shell that never answers all mean it does not.
fn probe_with_deadline(shell: &OsStr, command: &IssueCommand, deadline: Duration) -> bool {
    let Ok(mut child) = probe_command(shell, command).spawn() else {
        // No such shell, or it is not executable. A shell that cannot be
        // started has no functions to find.
        return false;
    };

    let give_up_at = Instant::now() + deadline;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if Instant::now() >= give_up_at {
                    // The child is killed and then reaped, in that order. A
                    // kill alone leaves a zombie for the life of the session,
                    // which is the process this deadline exists to prevent.
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(PROBE_POLL);
            }
            // The child cannot be asked about. Treating that as absent is the
            // same answer every other failure gets.
            Err(_) => return false,
        }
    }
}

/// The command `G` runs, where the environment names one and the shell has it.
///
/// Blocking: it starts a shell and waits for it. The caller runs it on a
/// thread of its own.
///
/// A value that turns the feature off starts no shell at all, which is what
/// keeps a public repository from asking every user's shell about one person's
/// function.
pub(crate) fn resolve(value: Option<&str>, shell: &OsStr) -> Option<IssueCommand> {
    let command = IssueCommand::new(value)?;
    probe_with_deadline(shell, &command, PROBE_DEADLINE).then_some(command)
}

#[cfg(all(test, unix))]
mod probe_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// The deadline for a stub that answers.
    ///
    /// It is the production deadline, and it costs nothing: a probe returns as
    /// soon as the shell exits. A shorter one would measure how fast this
    /// machine starts a process rather than what the probe does with the
    /// answer — the first start of a freshly written script took 600 ms here.
    const ANSWER_DEADLINE: Duration = PROBE_DEADLINE;

    /// The deadline for the stub that hangs.
    ///
    /// This is the one test that waits for a deadline, so the number is small.
    /// [`StubShell::new`] pays the slow first start before the test begins, so
    /// a second of it is a second the stub is already running in.
    const HANG_DEADLINE: Duration = Duration::from_secs(1);

    /// Longer than [`HANG_DEADLINE`] and far shorter than the `sleep` the
    /// hanging stub holds. A probe that waits for the shell crosses it.
    const GAVE_UP_WITHIN: Duration = Duration::from_secs(15);

    /// The variable that tells a stub to do nothing and exit.
    ///
    /// [`StubShell::new`] runs the script once with it set. The first start of
    /// a script this process just wrote is the slow one, and paying it here is
    /// what keeps the deadline of a test a measure of the probe rather than of
    /// the machine.
    const WARMUP_VAR: &str = "GSW_STUB_WARMUP";

    /// A script that stands in for the user's shell.
    ///
    /// It records what it was given and what environment it was given it in,
    /// then does what the test asked of it. The suite never reads the rc file
    /// of whoever runs it: an interactive shell of this machine would answer
    /// about this machine, and the answer would change from host to host.
    struct StubShell {
        /// Owns the files. Dropping it removes them, so it is held for as long
        /// as the test reads them.
        _dir: TempDir,
        /// The script itself, which the test gives to the probe as the shell.
        path: PathBuf,
        /// One line for each argument of each run.
        runs: PathBuf,
        /// The environment of each run.
        environment: PathBuf,
        /// The process id the last run holds, written by a stub that hangs.
        pid: PathBuf,
    }

    /// Where a stub's tail names the file it records its process id in.
    const PID_FILE: &str = "<PID_FILE>";

    impl StubShell {
        /// A stub whose last line is `tail`, with [`PID_FILE`] in it replaced
        /// by the quoted path of the file the stub records its id in.
        ///
        /// One directory holds the script and every file it writes, so the one
        /// [`TempDir`] this holds owns all of them.
        fn new(tail: &str) -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("stub-shell");
            let runs = dir.path().join("runs");
            let environment = dir.path().join("environment");
            let pid = dir.path().join("pid");
            let tail = tail.replace(PID_FILE, &shell_escape(&pid.display().to_string()));
            let script = format!(
                "#!/bin/sh\n                 [ -n \"${{{WARMUP_VAR}:-}}\" ] && exit 0\n                 printf '%s\\n' \"$@\" >> {}\n                 env >> {}\n                 {tail}\n",
                shell_escape(&runs.display().to_string()),
                shell_escape(&environment.display().to_string()),
            );
            std::fs::write(&path, script).expect("write the stub");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("make the stub executable");
            let stub = Self {
                _dir: dir,
                path,
                runs,
                environment,
                pid,
            };
            stub.warm();
            stub
        }

        /// Start the script once, so the test that follows does not pay for
        /// the first start of it.
        fn warm(&self) {
            let status = Command::new(&self.path)
                .env(WARMUP_VAR, "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("start the stub");
            assert!(status.success(), "the stub must start and exit");
            assert_eq!(
                self.runs(),
                "",
                "a warm-up must leave no record behind, or every other test here reads it",
            );
        }

        /// A stub that answers `status` and exits.
        fn answering(status: u8) -> Self {
            Self::new(&format!("exit {status}"))
        }

        /// A stub that records its process id and then never exits.
        ///
        /// `$$` is the shell's own process id, and `exec` keeps it — so the
        /// recorded id is the id of the process that hangs.
        fn hanging() -> Self {
            Self::new(&format!("echo $$ > {PID_FILE}\nexec sleep 30"))
        }

        /// Wait for the hanging stub to record its process id.
        ///
        /// The stub writes the file, and the probe kills the stub. Which of
        /// the two happens first is the machine's business, so a test that
        /// reads the file waits for it rather than assuming it is there.
        fn wait_for_pid(&self) -> i32 {
            let give_up_at = Instant::now() + GAVE_UP_WITHIN;
            while !self.pid.exists() {
                assert!(
                    Instant::now() < give_up_at,
                    "the hanging stub never recorded its process id",
                );
                std::thread::sleep(PROBE_POLL);
            }
            self.recorded_pid()
        }

        /// The stub, as the path to give the probe.
        fn as_shell(&self) -> &OsStr {
            self.path.as_os_str()
        }

        /// Every argument of every run, one for each line, or the empty string
        /// where the stub never ran.
        fn runs(&self) -> String {
            std::fs::read_to_string(&self.runs).unwrap_or_default()
        }

        /// The environment of every run.
        fn environment(&self) -> String {
            std::fs::read_to_string(&self.environment).unwrap_or_default()
        }

        /// The process id the hanging stub holds.
        fn recorded_pid(&self) -> i32 {
            std::fs::read_to_string(&self.pid)
                .expect("the stub must record its process id")
                .trim()
                .parse()
                .expect("the recorded process id must be a number")
        }
    }

    /// Whether a process of `pid` still exists.
    fn alive(pid: i32) -> bool {
        // SAFETY: `kill` with signal 0 sends nothing. It reports whether the
        // process exists and whether this user may signal it, and it touches
        // no memory of this process.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// The command the default name resolves to.
    fn default_command() -> IssueCommand {
        IssueCommand::new(None).expect("the default names a command")
    }

    #[test]
    fn a_shell_that_answers_zero_has_the_command() {
        let stub = StubShell::answering(0);
        assert!(
            probe_with_deadline(stub.as_shell(), &default_command(), ANSWER_DEADLINE),
            "exit status 0 means the command exists",
        );
        assert!(
            !stub.runs().is_empty(),
            "the probe must really ask the shell",
        );
    }

    #[test]
    fn a_shell_that_answers_one_does_not_have_the_command() {
        let stub = StubShell::answering(1);
        assert!(
            !probe_with_deadline(stub.as_shell(), &default_command(), ANSWER_DEADLINE),
            "every status but 0 means the command does not exist",
        );
    }

    #[test]
    fn a_shell_that_never_answers_gives_up_and_leaves_no_child() {
        let stub = StubShell::hanging();
        let started = Instant::now();
        assert!(
            !probe_with_deadline(stub.as_shell(), &default_command(), HANG_DEADLINE),
            "a shell that hangs must report the command absent",
        );
        assert!(
            started.elapsed() < GAVE_UP_WITHIN,
            "the probe must give up at its deadline rather than wait for the shell",
        );
        let pid = stub.wait_for_pid();
        assert!(
            !alive(pid),
            "the probe must leave no child behind, and {pid} is still running",
        );
    }

    #[test]
    fn a_value_that_turns_the_feature_off_starts_no_shell() {
        let stub = StubShell::answering(0);
        assert_eq!(
            resolve(Some(""), stub.as_shell()),
            None,
            "an empty value must turn the feature off",
        );
        assert_eq!(
            stub.runs(),
            "",
            "a feature that is off must ask no shell anything",
        );
    }

    #[test]
    fn the_probe_asks_about_the_command_the_variable_names() {
        let stub = StubShell::answering(0);
        let command = IssueCommand::new(Some("myfunc")).expect("a name");
        assert!(probe_with_deadline(stub.as_shell(), &command, ANSWER_DEADLINE));
        let runs = stub.runs();
        assert!(
            runs.contains("-ic"),
            "the shell must be interactive, or it reads no rc file: {runs:?}",
        );
        assert!(
            runs.contains("command -v 'myfunc'"),
            "the probe must ask about the name the variable holds: {runs:?}",
        );
    }

    #[test]
    fn the_probe_child_carries_no_git_location() {
        // The one guarantee that matters, read off the command itself: a
        // removal holds whatever the parent's environment says, which a test
        // that writes the process-global environment could not prove without
        // racing every other test in this binary.
        let command = probe_command(OsStr::new("/bin/sh"), &default_command());
        for name in GIT_LOCATION_VARS {
            assert!(
                command
                    .get_envs()
                    .any(|(key, value)| key == OsStr::new(name) && value.is_none()),
                "the probe child must carry no {name}",
            );
        }
    }

    #[test]
    fn the_probe_child_runs_with_no_git_location_in_its_environment() {
        let stub = StubShell::answering(0);
        assert!(probe_with_deadline(
            stub.as_shell(),
            &default_command(),
            ANSWER_DEADLINE
        ));
        let environment = stub.environment();
        for name in GIT_LOCATION_VARS {
            assert!(
                !environment.contains(&format!("{name}=")),
                "the child of the probe must not see {name}: {environment:?}",
            );
        }
    }
}
