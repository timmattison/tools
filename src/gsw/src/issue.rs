//! Opening the issue that the branch names, from watch mode.
//!
//! The `G` key of watch mode runs one command in the user's own interactive
//! shell. That command is usually a shell function, so only a shell can find
//! it and only a shell can run it. This module asks the shell both questions.
//!
//! The command can carry arguments, which splits the two questions. The shell
//! answers `command -v` about a name, so the question about existence carries
//! the first word alone. The run carries the whole line, because the rest of
//! it is the user's own arguments.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use shellquote::shell_quote;
use tempfile::NamedTempFile;

use crate::child::detach_from_terminal;
use crate::lines::LineSplitter;

/// The variable that holds the command `G` runs.
///
/// The value is a whole command line, so it can carry arguments. See
/// [`IssueCommand`] for what gsw does with each part of it.
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
/// shell about nothing, and it runs nothing. The rule gives the type its
/// second guarantee for free — a value with no space at either end and some
/// character in it always has a first word, which is what
/// [`IssueCommand::probe_word`] returns.
///
/// The value is a whole command line and not one name. `wn` reads
/// `WN_START_COMMAND` the same way, and it is the precedent this variable was
/// added against, so `gh issue view --web` must work here as `gh issue
/// develop` works there. The two halves of the value go to two different
/// places: the shell answers `command -v` about the first word, and it runs
/// the whole line.
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
    ///
    /// The space at each end goes and the words inside stay, so `gh issue view
    /// --web` goes in whole. [`IssueCommand::probe_word`] takes the first word
    /// off it for the question the shell can answer.
    pub(crate) fn new(value: Option<&str>) -> Option<Self> {
        match value {
            None => Some(Self(DEFAULT_ISSUE_COMMAND.to_string())),
            Some(named) => {
                let named = named.trim();
                (!named.is_empty()).then(|| Self(named.to_string()))
            }
        }
    }

    /// The whole value, which is never empty.
    ///
    /// This is the script the shell runs, arguments and all, and it is also
    /// the text every message about the command names. A message that named
    /// the first word alone would report `gh has not finished after 60s` for a
    /// run of `gh issue view --web`, and the user set the whole line.
    pub(crate) fn name(&self) -> &str {
        &self.0
    }

    /// The first word of the value, which is the word the probe asks about.
    ///
    /// A shell answers `command -v` about a name. It answers about nothing
    /// else, so the arguments must stay out of that question: `command -v 'gh
    /// issue view --web'` reports no command in any shell, and the key then
    /// goes quiet for a value that names a command every shell has. The whole
    /// value still goes to [`run_command`], where the shell reads the
    /// arguments the way it reads them at an interactive prompt.
    ///
    /// The word is never empty, because the value is never empty and the value
    /// carries no space at either end. A value of nothing but space turns the
    /// feature off in [`IssueCommand::new`], so every value that reaches here
    /// holds at least one character that is not space, and
    /// [`str::split_whitespace`] finds a word. The fallback states that rule
    /// rather than panic, and it gives the whole value — which is the first
    /// word of every value of one word.
    ///
    /// The cut goes by character and never by byte. A shell function takes any
    /// name the user gives it, and a cut by bytes takes a multi-byte character
    /// in half.
    pub(crate) fn probe_word(&self) -> &str {
        self.0.split_whitespace().next().unwrap_or(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name that `value` resolves to, as a plain string, or `None`.
    fn resolved(value: Option<&str>) -> Option<String> {
        IssueCommand::new(value).map(|command| command.name().to_string())
    }

    /// The word that the probe asks about, for `value`, or `None`.
    fn probe_word(value: Option<&str>) -> Option<String> {
        IssueCommand::new(value).map(|command| command.probe_word().to_string())
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

    #[test]
    fn a_command_of_more_than_one_word_keeps_every_word() {
        // `wn` is the precedent for this variable and it takes this shape, so
        // a reader who copies `gh issue develop` into `GSW_ISSUE_COMMAND`
        // gets the command and not silence.
        assert_eq!(
            resolved(Some("gh issue view --web")),
            Some("gh issue view --web".to_string()),
            "a command with arguments must keep every word",
        );
    }

    #[test]
    fn the_probe_word_of_a_command_with_arguments_is_its_first_word() {
        // `command -v` takes a name. No shell answers about a name that holds
        // the arguments too.
        assert_eq!(
            probe_word(Some("gh issue view --web")),
            Some("gh".to_string()),
            "the probe word must be the first word",
        );
    }

    #[test]
    fn the_probe_word_of_a_command_of_one_word_is_that_command() {
        assert_eq!(probe_word(Some("myfunc")), Some("myfunc".to_string()));
        assert_eq!(
            probe_word(None),
            Some(DEFAULT_ISSUE_COMMAND.to_string()),
            "the default is one word, and it is its own probe word",
        );
    }

    #[test]
    fn the_space_around_a_command_with_arguments_is_dropped() {
        assert_eq!(
            resolved(Some("  gh issue view  ")),
            Some("gh issue view".to_string()),
            "the space at each end goes and the space between the words stays",
        );
        assert_eq!(
            probe_word(Some("  gh issue view  ")),
            Some("gh".to_string())
        );
    }

    #[test]
    fn a_tab_between_two_words_separates_them() {
        // A variable written in an rc file carries whatever space the writer
        // typed. A tab is space, so it ends the first word.
        assert_eq!(
            probe_word(Some("gh\tissue view")),
            Some("gh".to_string()),
            "a tab must end the first word",
        );
        assert_eq!(
            resolved(Some("\t gh issue \t")),
            Some("gh issue".to_string()),
            "a tab at each end goes the way a space goes",
        );
        assert_eq!(
            resolved(Some("\t\t")),
            None,
            "a value of nothing but tabs must turn G off",
        );
    }

    #[test]
    fn a_command_named_outside_the_latin_alphabet_keeps_its_whole_first_word() {
        // A shell function takes any name the user gives it, and `問題` is
        // three characters of three bytes each. A cut by bytes takes a
        // character in half and asks the shell about text no program wrote.
        assert_eq!(
            probe_word(Some("問題 view --web")),
            Some("問題".to_string()),
            "the first word must arrive whole",
        );
        assert_eq!(
            resolved(Some("問題 view --web")),
            Some("問題 view --web".to_string()),
        );
    }
}

/// How long the probe waits for the shell to answer.
///
/// An rc file is somebody else's code, and one that hangs must not leave a
/// process behind for the life of the session. At the deadline the probe kills
/// the shell and every process that shell started — see [`end_probe_child`],
/// which signals the whole process group — and reports the command absent,
/// which is the same answer as a shell that said no.
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
pub(crate) fn user_shell() -> OsString {
    std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"))
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
/// job control off and start anyway. And it carries no `GIT_` variable out of
/// the environment of `gsw`.
///
/// **The rule is the `GIT_` prefix, and never a list of names.** The command
/// the user supplies asks `gh` about the issue, and `gh` reads the origin
/// remote of the directory it runs in. Many variables move that answer, and
/// they are not one family: `GIT_DIR`, `GIT_WORK_TREE` and `GIT_INDEX_FILE` aim
/// git at another repository, `GIT_COMMON_DIR` moves the files git reads
/// outside a worktree — config and refs among them — `GIT_CEILING_DIRECTORIES`
/// stops the walk that finds a repository at all, and `GIT_CONFIG_PARAMETERS`,
/// `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` set any key they like. A list
/// that named all of those today would still be a list, and it strips nothing
/// new the day git adds a variable. So this calls
/// [`gitscratch::shed_inherited_git_environment`], which enumerates
/// [`std::env::vars_os`] and removes every name that starts with `GIT_`. That
/// rule is written once, in the crate that states it, and it is tested there.
///
/// **A sweep is right here, where an allowlist is right for a tool that spawns
/// git itself.** The sweep takes the variables off the environment of `gsw`,
/// and this child is an interactive shell: `-i` makes it read the rc file of
/// the user, so a `GIT_` variable that the user exports on purpose is set again
/// inside the child, after the sweep. What the sweep removes is therefore only
/// what reaches the child from `gsw` itself, and that is exactly the hazard — a
/// `gsw` started from inside a pre-commit hook holds `GIT_DIR`,
/// `GIT_INDEX_FILE`, `GIT_PREFIX` and `GIT_CONFIG_PARAMETERS`, and the user
/// asked for none of them. `nwt` spawns `git` and not an interactive shell, so
/// nothing re-states what it strips, and it needs an allowlist for the
/// variables a user means to keep. This child has the rc file for that.
fn shell_child(shell: &OsStr, script: String) -> Command {
    let mut command = Command::new(shell);
    command.arg("-ic").arg(script);
    gitscratch::shed_inherited_git_environment(&mut command);
    detach_from_terminal(&mut command);
    command
}

/// The child that asks `shell` whether `command` exists.
///
/// `command -v` reports a function and an alias in both bash and zsh, which is
/// what makes this the right question: the thing being looked for is usually
/// neither a file nor a builtin.
///
/// **The question carries the first word of the value and nothing else.** A
/// shell answers `command -v` about a name, so a question that held the
/// arguments too would name a command no shell has, and every value with
/// arguments in it would turn the key off in silence. The whole value still
/// reaches [`run_command`].
///
/// The word goes through [`shell_quote`], which makes it one word whatever
/// characters it holds. The run quotes nothing, for the reason
/// [`run_command`] gives.
fn probe_command(shell: &OsStr, command: &IssueCommand) -> Command {
    let mut child = shell_child(
        shell,
        format!("command -v {}", shell_quote(command.probe_word())),
    );
    // Nothing the probe says belongs on the screen. An rc file that prints a
    // banner would otherwise paint over the frame.
    child
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    child
}

/// End the probe's child, and every process that child started.
///
/// **The signal goes to the process group, not to the process.** A shell that
/// hangs hangs inside some command it started, and not inside a builtin. So a
/// signal to the child alone kills the shell and leaves the command it was
/// waiting for — a grandchild of `gsw`, now with no parent — running for the
/// life of the session. The group holds both.
///
/// [`detach_from_terminal`] made the group. It calls `setsid` between the fork
/// and the exec, so the child leads a session and a group, and the id of that
/// group is the child's own process id.
///
/// **Signaling a group by that id is safe, and it can never reach `gsw`.** A
/// process group id is a process id, and the id of this child is fresh: the
/// system gave it to this child alone, and it gives it to nobody else while
/// the child lives. A group keeps its id reserved for as long as it has
/// members, so no other live group can hold it either. The call therefore
/// reaches the group `setsid` made, or it reaches nothing at all. `gsw` runs in
/// a group of its own id, which is a different number.
///
/// Nothing at all is the case the fallback covers. `setsid` fails with `EPERM`
/// where the caller already leads a process group, and
/// [`detach_from_terminal`] drops that error on purpose. The child then keeps
/// the group it was forked into, no group carries the child's id, and
/// `killpg` answers `ESRCH`. The direct child still has to die, so the
/// fallback kills it the old way.
#[cfg(unix)]
fn end_probe_child(child: &mut Child) {
    let group = libc::pid_t::try_from(child.id()).ok();
    let signaled = group.is_some_and(|group| {
        // SAFETY: `killpg` sends a signal to a process group. It takes one
        // integer and touches no memory of this process. The id is the id of a
        // child this function owns, so it names the group `setsid` made for
        // that child or no group at all.
        unsafe { libc::killpg(group, libc::SIGKILL) == 0 }
    });
    if !signaled {
        let _ = child.kill();
    }
}

/// Neither Unix nor Windows has a process group this code knows how to signal,
/// so the direct child is all this arm ends. See the Unix half above.
#[cfg(not(unix))]
fn end_probe_child(child: &mut Child) {
    let _ = child.kill();
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
                    // The group is killed and the child is then reaped, in
                    // that order. A kill alone leaves a zombie for the life of
                    // the session, which is the process this deadline exists
                    // to prevent. The grandchildren need no such reaping: the
                    // system gives an orphan a new parent that reaps it.
                    end_probe_child(&mut child);
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

/// The last line of `lines` that has text in it, with the space after it
/// dropped.
///
/// The last one, because a command says what it was doing and then says why it
/// stopped. The one with text in it, because a command that ends its last line
/// with a newline leaves an empty line after it, and an empty row under the
/// frame reads as a run that said nothing.
fn last_with_text(lines: &[String]) -> Option<String> {
    lines
        .iter()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim_end().to_string())
}

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
                last_with_text(lines).unwrap_or_else(|| format!("{name} failed ({status})")),
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
            message: Some(match last_with_text(lines) {
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

/// The child that runs `command` in `workdir`.
///
/// The command is the whole script, unquoted, and both halves of that matter.
///
/// **The first word stays unquoted because the shell must expand it.** The
/// probe already asked this shell about that exact word and the shell said
/// yes, so the word names a command this shell has. Quotes around it would
/// stop the shell from expanding an alias, which is one of the two things
/// `command -v` reports.
///
/// **The rest stays unquoted because it belongs to the user.** Everything
/// after the first word is the arguments the user wrote into
/// [`ISSUE_COMMAND_ENV`], and the shell reads them here the way it reads them
/// at an interactive prompt: it splits them at each space, it expands a
/// variable, and it matches a pattern against file names. That is what the
/// user asked for. A value of `gh issue view --web` is four words to the
/// shell, and one quoted word to a shell would be a command no machine has.
///
/// The caller attaches the two files the child writes to. Where the output
/// goes is the one thing about this child that a deadline depends on, so
/// [`start_run`] owns it.
fn run_command(shell: &OsStr, command: &IssueCommand, workdir: &Path) -> Command {
    let mut child = shell_child(shell, command.name().to_string());
    // The command asks `gh` about the issue, and `gh` reads the origin remote
    // of the directory it runs in.
    child.current_dir(workdir).stdin(Stdio::null());
    child
}

/// How long a run waits for the command to finish.
///
/// The probe asks a shell a question it answers out of memory, so
/// [`PROBE_DEADLINE`] is small. A run is the user's own work: `gh` asks a
/// server about the issue, and the browser that opens the page takes a moment
/// of its own. So this number is much larger.
///
/// Sixty seconds is the trade. A shorter deadline reports a slow fetch over a
/// slow network as a run that stopped, which is a lie about a run that goes on
/// to open the page. A longer one holds the key: one run at a time is the
/// rule, so the key means nothing while a run is open, and a user reads a key
/// that does nothing as a key with nothing behind it. Sixty seconds is longer
/// than a slow fetch and shorter than a session.
const RUN_DEADLINE: Duration = Duration::from_secs(60);

/// A run in flight: the child, and the two files it writes to.
///
/// The files are the load-bearing half, and they are files rather than pipes.
/// A pipe makes the reader wait for end of file, and end of file arrives only
/// when the last writer lets go — so a child the command leaves behind holds
/// the run open long after the shell is gone. `xdg-open` leaves exactly such a
/// child. A file has no such wait, and it also cannot fill up and stop the
/// child the way a pipe that nobody reads does.
///
/// Each file is removed when this value is dropped. A child that still holds
/// one keeps writing to it, because a file a process has open outlives its
/// name, and the space comes back when that child ends.
struct RunInFlight {
    /// The shell, which is what the deadline waits for.
    child: Child,
    /// Where the child writes what it did.
    stdout: NamedTempFile,
    /// Where the child writes why it stopped.
    stderr: NamedTempFile,
}

/// Start `command` in `workdir`, with a file for each of its two streams.
fn start_run(
    shell: &OsStr,
    command: &IssueCommand,
    workdir: &Path,
) -> std::io::Result<RunInFlight> {
    let stdout = NamedTempFile::new()?;
    let stderr = NamedTempFile::new()?;
    let mut builder = run_command(shell, command, workdir);
    builder
        .stdout(Stdio::from(stdout.as_file().try_clone()?))
        .stderr(Stdio::from(stderr.as_file().try_clone()?));
    let child = builder.spawn()?;
    Ok(RunInFlight {
        child,
        stdout,
        stderr,
    })
}

/// `bytes` as lines gsw can paint.
///
/// [`LineSplitter`] is the one place a child's bytes become such text — a tab
/// is up to eight columns and an escape sequence repaints the frame in another
/// program's colors.
///
/// **One splitter for each stream, which is the rule [`LineSplitter`] states
/// for itself.** A splitter holds the bytes of a line that has no terminator
/// yet, and it holds them from one call to the next. So a single splitter
/// across both streams gives the tail of standard output to the first line of
/// standard error and reports the two as one line. A command that stops
/// mid-word makes that line a word no program wrote, and a command that stops
/// in the middle of a character puts a replacement character in front of the
/// reason. The reason is the one thing this feature puts on the screen. This
/// function takes one stream, so each caller of it gets a splitter of its own.
fn painted_lines(bytes: &[u8]) -> Vec<String> {
    let mut splitter = LineSplitter::new();
    let mut lines = splitter.feed(bytes);
    lines.extend(splitter.finish());
    lines
}

/// Everything the run has written so far, standard output first.
///
/// That is the order a refusal reads in: a command says what it did on one
/// stream and why it stopped on the other.
///
/// A file that cannot be read counts as a file with nothing in it. The run is
/// over either way, and a read that failed is not something to put in front of
/// the words the command wrote on the other stream.
fn written_lines(run: &RunInFlight) -> Vec<String> {
    let mut lines = painted_lines(&std::fs::read(run.stdout.path()).unwrap_or_default());
    lines.extend(painted_lines(
        &std::fs::read(run.stderr.path()).unwrap_or_default(),
    ));
    lines
}

/// Run `command` in `workdir` and report what to say about it.
///
/// Blocking: the caller runs it on a thread of its own.
pub(crate) fn run(shell: &OsStr, command: &IssueCommand, workdir: &Path) -> IssueOutcome {
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
/// [`PROBE_DEADLINE`]. So a run that hangs hangs in the command, and the
/// command is the user's to end.
///
/// The child still has to be reaped. A dropped [`Child`] is neither killed nor
/// waited for, and a child nobody waits for stays as a defunct entry for the
/// life of the session — which is the cost this deadline exists to avoid. So
/// the child goes to a thread that waits for it. That thread ends when the
/// child ends, so the child is what bounds it.
fn run_with_deadline(
    shell: &OsStr,
    command: &IssueCommand,
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
    use super::stub_shell::{
        alive, kill_now, test_process_can_open_the_terminal, StubShell, ANSWER_DEADLINE,
        GAVE_UP_WITHIN, HANG_DEADLINE, TTY_REFUSED,
    };
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc::channel;

    /// The prefix that makes a variable git's.
    ///
    /// The rule the children hold is this prefix, and never a list of names, so
    /// the test that reads their environment back asks about the prefix too. A
    /// test that asked about a list of names would pass for a variable the list
    /// forgot.
    const GIT_PREFIX: &str = "GIT_";

    /// The variable that tells this test binary it is the child, and that it
    /// must do the work rather than start a child of its own.
    ///
    /// The name carries no [`GIT_PREFIX`], so the sweep under test leaves it
    /// alone and the child can still read it.
    const HOSTILE_MARKER: &str = "GSW_HOSTILE_GIT_ENVIRONMENT";

    /// The line the child prints after its last assertion holds.
    ///
    /// libtest exits 0 when a filter names no test, so a child that ran nothing
    /// reads exactly like a child that passed. The parent looks for this line
    /// as well as for the exit status.
    const CHILD_RAN: &str = "gsw-hostile-environment-child-ran";

    /// How long the parent waits for the child.
    ///
    /// The stub shell answers at once, so a healthy child takes milliseconds.
    /// The bound is here for the child that hangs: a test that waits for such a
    /// child holds the run for the life of the session.
    const CHILD_DEADLINE: Duration = Duration::from_secs(30);

    /// A hostile environment: every variable that aims git, or configures it,
    /// or stops it from finding a repository at all.
    ///
    /// A pre-commit hook exports several of these, so a `gsw` started from
    /// inside one holds them for real. The paths name nothing on this machine,
    /// because a variable that reached a child must be visible as a variable
    /// and never as work done in another repository.
    ///
    /// `GIT_DIR`, `GIT_WORK_TREE` and `GIT_INDEX_FILE` aim git at another
    /// repository. `GIT_COMMON_DIR` moves the files git reads outside a
    /// worktree, config and refs among them, so a leaked one gives `gh` another
    /// `remote.origin.url`. `GIT_CEILING_DIRECTORIES` stops the walk that finds
    /// a repository. `GIT_OBJECT_DIRECTORY` moves the objects.
    /// `GIT_CONFIG_PARAMETERS` and `GIT_CONFIG_GLOBAL` set any key at all.
    const HOSTILE_GIT_ENVIRONMENT: [(&str, &str); 8] = [
        ("GIT_DIR", "/gsw-decoy/.git"),
        ("GIT_WORK_TREE", "/gsw-decoy"),
        ("GIT_INDEX_FILE", "/gsw-decoy/.git/index"),
        ("GIT_COMMON_DIR", "/gsw-decoy/.git"),
        ("GIT_CEILING_DIRECTORIES", "/gsw-decoy"),
        ("GIT_OBJECT_DIRECTORY", "/gsw-decoy/.git/objects"),
        (
            "GIT_CONFIG_PARAMETERS",
            "'remote.origin.url=https://example.invalid/decoy.git'",
        ),
        ("GIT_CONFIG_GLOBAL", "/gsw-decoy/gitconfig"),
    ];

    /// `path` with every symbolic link in it resolved.
    ///
    /// macOS reaches a temporary directory through a symbolic link, so the
    /// path the shell prints is not the path this test asked for. Both sides
    /// are resolved before they are compared.
    fn resolved(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).expect("resolve the path")
    }

    /// The default command, which is what every test here runs.
    fn default_command() -> IssueCommand {
        IssueCommand::new(None).expect("the default names a command")
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
    fn the_run_names_the_command_with_no_quoting_around_it() {
        // Quoting the word stops a shell from expanding an alias, and an alias
        // is one of the two things the probe reports.
        let stub = StubShell::answering(0);
        let workdir = tempfile::tempdir().expect("tempdir");
        let command = IssueCommand::new(Some("myfunc")).expect("a name");
        let _ = run(stub.as_shell(), &command, workdir.path());
        let runs = stub.runs();
        assert!(
            runs.lines().any(|line| line == "myfunc"),
            "the script must be the bare name: {runs:?}",
        );
    }

    #[test]
    fn the_run_is_the_whole_value_with_the_arguments_in_it() {
        // The probe asks about the first word, because that is the word a
        // shell can answer about. The run is the whole line, because the rest
        // of it is the user's own arguments.
        let stub = StubShell::answering(0);
        let workdir = tempfile::tempdir().expect("tempdir");
        let command = IssueCommand::new(Some("gh issue view --web")).expect("a name");
        let _ = run(stub.as_shell(), &command, workdir.path());
        let runs = stub.runs();
        assert!(
            runs.lines().any(|line| line == "gh issue view --web"),
            "the script must be the whole value: {runs:?}",
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
    fn a_tail_on_standard_output_does_not_join_the_first_line_of_standard_error() {
        // A command that stops mid-line on one stream and gives its reason on
        // the other is the shape a refusal arrives in. One splitter across
        // both streams keeps the unterminated bytes of standard output, and
        // the first line of standard error then completes them — the text
        // under the frame reads `partialreason`, which is a word no program
        // wrote. Each stream gets a splitter of its own, so each tail keeps
        // its own row.
        let stub = StubShell::new("printf 'partial'\nprintf 'reason\\n' >&2\nexit 2");
        let workdir = tempfile::tempdir().expect("tempdir");
        let outcome = run(stub.as_shell(), &default_command(), workdir.path());
        assert_eq!(
            outcome.message(),
            Some("reason"),
            "the tail of standard output must not join the first line of standard error",
        );
    }

    #[test]
    fn a_character_cut_in_half_on_standard_output_stays_out_of_the_message() {
        // `日` is three bytes and this stub writes the first two of them. One
        // splitter across both streams decodes that broken tail together with
        // the first line of standard error, which puts a replacement character
        // in front of the reason. Two splitters keep the broken tail on a row
        // of its own, where it costs the reason nothing.
        let stub = StubShell::new("printf '\\346\\227'\nprintf 'reason\\n' >&2\nexit 2");
        let workdir = tempfile::tempdir().expect("tempdir");
        let outcome = run(stub.as_shell(), &default_command(), workdir.path());
        let message = outcome.message();
        assert_eq!(
            message,
            Some("reason"),
            "a character cut in half on standard output must not reach the message",
        );
        assert!(
            !message.is_some_and(|text| text.contains('\u{fffd}')),
            "the message must carry no replacement character: {message:?}",
        );
    }

    #[test]
    fn the_run_child_cannot_open_the_controlling_terminal() {
        // Watch mode holds the alternate screen in raw mode, and this child is
        // an interactive shell. An interactive shell opens `/dev/tty` for job
        // control and for every prompt it paints, so a child that keeps the
        // controlling terminal reads the keys the event thread of `gsw` is
        // waiting for and paints over the frame. Nothing in the tree of this
        // child may be able to open the terminal.
        //
        // The run is the half with the longer reach: the probe asks a shell one
        // question, and this starts the command of the user in that shell.
        if !test_process_can_open_the_terminal() {
            eprintln!(
                "skipped: this test process has no controlling terminal, so /dev/tty is \
                 unopenable for every child regardless - the assertion would hold vacuously",
            );
            return;
        }

        let stub = StubShell::probing_the_terminal();
        let workdir = tempfile::tempdir().expect("tempdir");
        let outcome = run(stub.as_shell(), &default_command(), workdir.path());
        assert_eq!(
            outcome.message(),
            None,
            "the stub exits 0, so the run must report nothing",
        );
        assert_eq!(
            stub.terminal_record(),
            TTY_REFUSED,
            "the run child keeps the controlling terminal, so the command of the user can paint \
             over the frame of gsw and take the keys gsw is reading",
        );
    }

    #[test]
    fn the_run_happens_in_the_work_tree() {
        // The command asks `gh` about the issue, and `gh` reads the origin
        // remote of the directory it runs in.
        let stub = StubShell::answering(0);
        let workdir = tempfile::tempdir().expect("tempdir");
        let _ = run(stub.as_shell(), &default_command(), workdir.path());
        assert_eq!(resolved(&stub.cwd()), resolved(workdir.path()));
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

    #[test]
    fn a_run_returns_when_the_shell_exits_and_not_when_its_children_do() {
        // A child the command leaves behind inherits where the output goes. A
        // pipe makes the run wait for end of file, and that child holds the
        // pipe open after the shell is gone — so the run waited for `sleep 30`
        // rather than for the shell, with the key held for all of it. A file
        // has no such wait.
        //
        // The run happens on a thread of its own, so this test reports the
        // defect rather than a wait of its own: a run that waits for the child
        // never returns inside the bound, and the channel says so.
        let stub = StubShell::outlived_by_a_child();
        let workdir = tempfile::tempdir().expect("tempdir");
        let shell = stub.as_shell().to_os_string();
        let dir = workdir.path().to_path_buf();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let outcome = run_with_deadline(&shell, &default_command(), &dir, RUN_DEADLINE);
            let _ = tx.send(outcome);
        });
        let outcome = rx
            .recv_timeout(GAVE_UP_WITHIN)
            .expect("the run must return when the shell exits, not when its children do");
        assert_eq!(
            outcome.message(),
            None,
            "the shell exited 0, so the browser is the answer",
        );
        kill_now(stub.wait_for_pid());
    }

    /// Start this test binary again, with `test` named and the hostile
    /// environment on it, and fail where that child fails.
    ///
    /// The child writes to two files rather than to two pipes, for the reason
    /// [`RunInFlight`] gives: a pipe is read to its end, and the end arrives
    /// when the last writer lets go.
    ///
    /// The wait is bounded. A child that hangs is killed and reaped, and the
    /// test then fails, because a test that waits for such a child holds the
    /// run for the life of the session.
    ///
    /// The child runs in a directory of its own, which is a temporary directory
    /// and no repository.
    fn a_child_of_this_test_passes(test: &str) {
        let workdir = tempfile::tempdir().expect("tempdir");
        let stdout = NamedTempFile::new().expect("a file for what the child says");
        let stderr = NamedTempFile::new().expect("a file for why the child stopped");
        let mut command = Command::new(std::env::current_exe().expect("the path of this binary"));
        command
            .args(["--exact", "--nocapture", test])
            .current_dir(workdir.path())
            .env(HOSTILE_MARKER, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                stdout.as_file().try_clone().expect("clone the file"),
            ))
            .stderr(Stdio::from(
                stderr.as_file().try_clone().expect("clone the file"),
            ));
        for (name, value) in HOSTILE_GIT_ENVIRONMENT {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("start this test binary again");

        let give_up_at = Instant::now() + CHILD_DEADLINE;
        let status = loop {
            match child.try_wait().expect("ask about the child") {
                Some(status) => break status,
                None => {
                    if Instant::now() >= give_up_at {
                        // The kill comes before the panic. A panic ends the
                        // test where it stands, and the process this test
                        // started is the one thing that must not outlive it.
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!(
                            "the child did not finish within {}s",
                            CHILD_DEADLINE.as_secs(),
                        );
                    }
                    std::thread::sleep(PROBE_POLL);
                }
            }
        };

        let said = format!(
            "{}{}",
            std::fs::read_to_string(stdout.path()).unwrap_or_default(),
            std::fs::read_to_string(stderr.path()).unwrap_or_default(),
        );
        assert!(status.success(), "the child failed ({status}):\n{said}");
        assert!(
            said.contains(CHILD_RAN),
            "the child ran no test, so it passed for the wrong reason. libtest exits 0 when a \
             filter names no test, and the filter was {test:?}:\n{said}",
        );
    }

    /// The name of this test, the way libtest spells it.
    ///
    /// `module_path!` starts with the name of the crate and a test name does
    /// not, so the first part goes. The rest of the path comes from the
    /// compiler, so a module that moves needs no edit here.
    fn test_name(function: &str) -> String {
        let module = module_path!();
        let module = module.split_once("::").map_or(module, |(_, rest)| rest);
        format!("{module}::{function}")
    }

    /// Neither child carries a `GIT_` variable out of the environment of `gsw`.
    ///
    /// **This test starts this test binary again, and the hostile environment
    /// goes on that child.** A `GIT_` variable is process-global state. A test
    /// that sets one in this process changes what every other test in this
    /// binary reads, and several of them run real git. A child holds an
    /// environment of its own, so the hostile values reach the code under test
    /// and reach nothing else. That is also what makes the result the same
    /// under a shell and under the pre-commit hook of this repository, which
    /// exports `GIT_` variables into `cargo test`: the child sets the whole
    /// list itself.
    ///
    /// **The armed control comes first.** The child asserts that it really
    /// holds each hostile variable. An assertion that a variable is absent from
    /// the stub passes just as readily where there was nothing to remove.
    ///
    /// **The stub records what the children got.** A test that reads the
    /// removals off the [`Command`] proves less: a sweep of the `GIT_` prefix
    /// records a removal only for a variable this process holds, so such a test
    /// is vacuous under a shell and meaningful under a hook. The stub is the
    /// environment the child really ran in.
    ///
    /// Both children are covered here, the probe and the run, because both come
    /// from [`shell_child`] and the rule is a property of that one function.
    #[test]
    fn neither_child_carries_a_git_variable_out_of_a_hostile_environment() {
        if std::env::var_os(HOSTILE_MARKER).is_none() {
            a_child_of_this_test_passes(&test_name(
                "neither_child_carries_a_git_variable_out_of_a_hostile_environment",
            ));
            return;
        }

        for (name, _) in HOSTILE_GIT_ENVIRONMENT {
            assert!(
                std::env::var_os(name).is_some(),
                "the child must really hold {name}, or there is nothing here to remove and the \
                 assertion below is measured against nothing",
            );
        }

        let stub = StubShell::answering(0);
        let workdir = tempfile::tempdir().expect("tempdir");
        assert!(
            probe_with_deadline(stub.as_shell(), &default_command(), ANSWER_DEADLINE),
            "the stub answers 0, so the probe must report the command present",
        );
        let outcome = run(stub.as_shell(), &default_command(), workdir.path());
        assert_eq!(
            outcome.message(),
            None,
            "the stub exits 0, so the run must report nothing",
        );

        let environment = stub.environment();
        assert!(
            !environment.is_empty(),
            "both children must record the environment they ran in",
        );
        let carried: Vec<&str> = environment
            .lines()
            .filter(|line| {
                line.split_once('=')
                    .is_some_and(|(key, _)| key.starts_with(GIT_PREFIX))
            })
            .collect();
        assert!(
            carried.is_empty(),
            "a child of gsw carried a git variable out of the environment of gsw. The command \
             asks `gh` about the issue, and `gh` reads the origin remote of the directory it runs \
             in - so each of these aims that question, or configures it, somewhere the user never \
             pointed it: {carried:?}",
        );

        println!("{CHILD_RAN}");
    }
}

#[cfg(all(test, unix))]
mod stub_shell {
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
    pub(super) const ANSWER_DEADLINE: Duration = PROBE_DEADLINE;

    /// The deadline for the stub that hangs.
    ///
    /// This is the one test that waits for a deadline, so the number is small.
    /// [`StubShell::new`] pays the slow first start before the test begins, so
    /// a second of it is a second the stub is already running in.
    pub(super) const HANG_DEADLINE: Duration = Duration::from_secs(1);

    /// Longer than [`HANG_DEADLINE`] and far shorter than the `sleep` the
    /// hanging stub holds. A probe that waits for the shell crosses it.
    pub(super) const GAVE_UP_WITHIN: Duration = Duration::from_secs(15);

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
    pub(super) struct StubShell {
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
        /// The directory of each run.
        cwd: PathBuf,
        /// What the last run found when it reached for the controlling
        /// terminal, written by a stub that looks for one.
        tty: PathBuf,
    }

    /// Where a stub's tail names the file it records its process id in.
    const PID_FILE: &str = "<PID_FILE>";

    /// Where a stub's tail names the file it records the terminal in.
    ///
    /// A second placeholder of the same shape as [`PID_FILE`], because the
    /// answer travels the same way. A stub cannot report through its standard
    /// streams: the probe sends all three to [`Stdio::null`], and a run sends
    /// two of them to temporary files the runner owns. A file of its own is
    /// how a stub says what it found.
    const TTY_FILE: &str = "<TTY_FILE>";

    /// What a stub writes when it **could** open the controlling terminal.
    ///
    /// This is the failure the tests of it exist to catch.
    pub(super) const TTY_OPENED: &str = "opened";

    /// What a stub writes when `/dev/tty` was unopenable.
    ///
    /// This is the only outcome that keeps an interactive shell from painting
    /// a prompt over the frame of `gsw` and taking the keys `gsw` reads.
    pub(super) const TTY_REFUSED: &str = "refused";

    /// Whether the **test process** can open the controlling terminal.
    ///
    /// An assertion about a child is worth making only where there is a
    /// terminal for that child to be denied. A `cargo test` started from a
    /// script, from a runner, or from the pre-commit hook of this repository
    /// has no controlling terminal at all. `/dev/tty` is then unopenable for
    /// every process in the tree, detached or not, and the assertion holds for
    /// a reason that has nothing to do with the code. A test that reads this
    /// skips with a printed reason rather than bank such a vacancy as a green.
    pub(super) fn test_process_can_open_the_terminal() -> bool {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .is_ok()
    }

    impl StubShell {
        /// A stub whose last line is `tail`, with [`PID_FILE`] and
        /// [`TTY_FILE`] in it replaced by the quoted paths of the files the
        /// stub records its id and its terminal in.
        ///
        /// One directory holds the script and every file it writes, so the one
        /// [`TempDir`] this holds owns all of them.
        pub(super) fn new(tail: &str) -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("stub-shell");
            let runs = dir.path().join("runs");
            let environment = dir.path().join("environment");
            let pid = dir.path().join("pid");
            let cwd = dir.path().join("cwd");
            let tty = dir.path().join("tty");
            let tail = tail
                .replace(PID_FILE, &shell_quote(&pid.display().to_string()))
                .replace(TTY_FILE, &shell_quote(&tty.display().to_string()));
            let script = format!(
                "#!/bin/sh\n\
                 [ -n \"${{{WARMUP_VAR}:-}}\" ] && exit 0\n\
                 printf '%s\\n' \"$@\" >> {}\n\
                 env >> {}\n\
                 pwd >> {}\n\
                 {tail}\n",
                shell_quote(&runs.display().to_string()),
                shell_quote(&environment.display().to_string()),
                shell_quote(&cwd.display().to_string()),
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
                cwd,
                tty,
            };
            stub.warm();
            stub
        }

        /// Start the script once, so the test that follows does not pay for
        /// the first start of it.
        pub(super) fn warm(&self) {
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
        pub(super) fn answering(status: u8) -> Self {
            Self::new(&format!("exit {status}"))
        }

        /// A stub that reaches for the controlling terminal, records what it
        /// found, and exits 0.
        ///
        /// This is the shape of the fake `ssh` of
        /// `the_push_child_cannot_open_the_controlling_terminal`, and it asks
        /// the same question of the two children of this module.
        ///
        /// **The subshell is deliberate.** A redirection that fails ends the
        /// shell that carries it, so a bare `exec 3<>/dev/tty` in the stub
        /// itself takes the stub down and the `else` branch is unreachable.
        /// The subshell takes that failure instead, and the stub reads its
        /// status.
        ///
        /// The status is 0, so the probe reports the command present and the
        /// run reports nothing. Each test asserts that as well as the record,
        /// because both say the production path really reached the stub.
        pub(super) fn probing_the_terminal() -> Self {
            Self::new(&format!(
                "if ( exec 3<>/dev/tty ) 2>/dev/null; then\n\
                 \tprintf '{TTY_OPENED}' > {TTY_FILE}\n\
                 else\n\
                 \tprintf '{TTY_REFUSED}' > {TTY_FILE}\n\
                 fi\n\
                 exit 0",
            ))
        }

        /// A stub that records its process id and then never exits.
        ///
        /// `$$` is the shell's own process id, and `exec` keeps it — so the
        /// recorded id is the id of the process that hangs.
        pub(super) fn hanging() -> Self {
            Self::new(&format!("echo $$ > {PID_FILE}\nexec sleep 30"))
        }

        /// A stub that starts a child of its own and then waits for it.
        ///
        /// This is the shape a real rc file hangs in. A shell hangs inside
        /// some command it started, not inside a builtin, so the process that
        /// holds the session open is a grandchild of `gsw` and not the child
        /// `gsw` started. `$!` is that grandchild, so the recorded id is the
        /// id of the process that hangs. [`StubShell::hanging`] records `$$`
        /// after an `exec`, which makes the shell itself the process that
        /// hangs — the narrow case, and the one a signal to the direct child
        /// already covers.
        pub(super) fn hanging_in_a_child() -> Self {
            Self::new(&format!("sleep 30 &\necho $! > {PID_FILE}\nwait"))
        }

        /// A stub that writes `said` and then never exits.
        ///
        /// A command that says why it stopped, and then hangs, is the shape
        /// that makes those words worth a row at a deadline. The shell writes
        /// them before it replaces itself, so they are in the file the moment
        /// the deadline arrives.
        pub(super) fn hanging_after_saying(said: &str) -> Self {
            Self::new(&format!(
                "printf '%s\\n' {}\necho $$ > {PID_FILE}\nexec sleep 30",
                shell_quote(said),
            ))
        }

        /// A stub that starts a child of its own, records that child, and
        /// exits at once.
        ///
        /// The child inherits where the stub writes, and it holds that place
        /// open long after the stub is gone. `$!` is the child, so the
        /// recorded id is the id of the process that outlives the shell.
        pub(super) fn outlived_by_a_child() -> Self {
            Self::new(&format!("sleep 30 &\necho $! > {PID_FILE}\nexit 0"))
        }

        /// Wait for the hanging stub to record its process id.
        ///
        /// The stub writes the file, and the probe kills the stub. Which of
        /// the two happens first is the machine's business, so a test that
        /// reads the file waits for it rather than assuming it is there.
        pub(super) fn wait_for_pid(&self) -> i32 {
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
        pub(super) fn as_shell(&self) -> &OsStr {
            self.path.as_os_str()
        }

        /// Every argument of every run, one for each line, or the empty string
        /// where the stub never ran.
        pub(super) fn runs(&self) -> String {
            std::fs::read_to_string(&self.runs).unwrap_or_default()
        }

        /// The environment of every run.
        pub(super) fn environment(&self) -> String {
            std::fs::read_to_string(&self.environment).unwrap_or_default()
        }

        /// The directory of the last run, as the shell reported it.
        pub(super) fn cwd(&self) -> PathBuf {
            let recorded = std::fs::read_to_string(&self.cwd).expect("the stub must record a cwd");
            PathBuf::from(recorded.trim_end())
        }

        /// What the stub found when it reached for the controlling terminal.
        ///
        /// The read fails where the file is absent, and an absent file means
        /// the stub never ran. A test that took that for an answer would pass
        /// while it proved nothing at all, so the message names that case.
        pub(super) fn terminal_record(&self) -> String {
            std::fs::read_to_string(&self.tty)
                .expect("the stub never ran, so the terminal probe proved nothing")
                .trim()
                .to_string()
        }

        /// The process id the hanging stub holds.
        pub(super) fn recorded_pid(&self) -> i32 {
            std::fs::read_to_string(&self.pid)
                .expect("the stub must record its process id")
                .trim()
                .parse()
                .expect("the recorded process id must be a number")
        }
    }

    /// Whether a process of `pid` still exists.
    pub(super) fn alive(pid: i32) -> bool {
        // SAFETY: `kill` with signal 0 sends nothing. It reports whether the
        // process exists and whether this user may signal it, and it touches
        // no memory of this process.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// Wait for the process of `pid` to go, and report whether it went.
    ///
    /// A signal arrives when the system delivers it, and not when the call
    /// that sent it returns. So a process that is already dead answers for a
    /// moment longer, and a test that looks once reports a process that is on
    /// its way out as a process that stays. This waits, the way
    /// [`StubShell::wait_for_pid`] waits for a file.
    pub(super) fn wait_until_gone(pid: i32) -> bool {
        let give_up_at = Instant::now() + GAVE_UP_WITHIN;
        while alive(pid) {
            if Instant::now() >= give_up_at {
                return false;
            }
            std::thread::sleep(PROBE_POLL);
        }
        true
    }

    /// End the process of `pid` now.
    ///
    /// A test that proves `gsw` leaves a process running owns that process
    /// afterwards. The suite cleans up what it started, so the test that
    /// asked for the process is the one that ends it.
    pub(super) fn kill_now(pid: i32) {
        // SAFETY: `kill` sends a signal to a process this test started. It
        // touches no memory of this process.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

#[cfg(all(test, unix))]
mod probe_tests {
    use super::stub_shell::{
        alive, kill_now, test_process_can_open_the_terminal, wait_until_gone, StubShell,
        ANSWER_DEADLINE, GAVE_UP_WITHIN, HANG_DEADLINE, TTY_REFUSED,
    };
    use super::*;

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
    fn a_shell_that_hangs_in_a_child_leaves_no_grandchild() {
        // The shape a real rc file hangs in. A shell waits inside some command
        // it started, so the process that holds the session open is a
        // grandchild of gsw. A signal to the direct child alone kills the
        // shell and leaves that grandchild running, which is the process this
        // deadline exists to prevent.
        let stub = StubShell::hanging_in_a_child();
        assert!(
            !probe_with_deadline(stub.as_shell(), &default_command(), HANG_DEADLINE),
            "a shell that hangs must report the command absent",
        );
        let pid = stub.wait_for_pid();
        let gone = wait_until_gone(pid);
        // The cleanup comes before the assertion. A failed assertion ends the
        // test where it stands, and the process this test started is the one
        // thing that must not outlive it.
        if !gone {
            kill_now(pid);
        }
        assert!(
            gone,
            "the probe must leave no process behind, and the grandchild {pid} is still running",
        );
    }

    #[test]
    fn the_probe_child_cannot_open_the_controlling_terminal() {
        // The probe runs `$SHELL -ic`, which reads the rc file of the user.
        // That file is somebody else's code, and it runs while watch mode holds
        // the alternate screen in raw mode. An interactive shell that keeps the
        // controlling terminal opens `/dev/tty` for job control, and anything
        // the rc file starts can ask the same terminal for a password. Both
        // take the keys the event thread of `gsw` is waiting for.
        if !test_process_can_open_the_terminal() {
            eprintln!(
                "skipped: this test process has no controlling terminal, so /dev/tty is \
                 unopenable for every child regardless - the assertion would hold vacuously",
            );
            return;
        }

        let stub = StubShell::probing_the_terminal();
        assert!(
            probe_with_deadline(stub.as_shell(), &default_command(), ANSWER_DEADLINE),
            "the stub exits 0, so the probe must report the command present",
        );
        assert_eq!(
            stub.terminal_record(),
            TTY_REFUSED,
            "the probe child keeps the controlling terminal, so the rc file of the user can paint \
             over the frame of gsw and take the keys gsw is reading",
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
        assert!(probe_with_deadline(
            stub.as_shell(),
            &command,
            ANSWER_DEADLINE
        ));
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
    fn the_probe_asks_about_the_first_word_of_a_command_with_arguments() {
        // A value that carries arguments is the shape `wn` takes for the same
        // job. `command -v 'gh issue view --web'` names no command in any
        // shell, so the probe reports the command absent and `G` goes quiet -
        // the one silent state the key has, and the user sees no reason for
        // it.
        let stub = StubShell::answering(0);
        let command = IssueCommand::new(Some("gh issue view --web")).expect("a name");
        assert!(probe_with_deadline(
            stub.as_shell(),
            &command,
            ANSWER_DEADLINE
        ));
        let runs = stub.runs();
        assert!(
            runs.contains("command -v 'gh'"),
            "the probe must ask about the first word: {runs:?}",
        );
        assert!(
            !runs.contains("command -v 'gh issue view --web'"),
            "the probe must not put the arguments inside the quotes: {runs:?}",
        );
    }
}
