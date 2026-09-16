//! The shell that runs a command the user supplies, for every key of watch
//! mode that runs one.
//!
//! Such a command is usually a shell function, so only a shell can find it and
//! only a shell can run it. This module asks the shell both questions, and it
//! holds what every one of those keys needs: the shell itself, the type a
//! command name becomes, the probe that asks whether the command exists, the
//! child that runs it, and the run that reads what that child writes.
//!
//! The command can carry arguments, which splits the two questions. The shell
//! answers `command -v` about a name, so the question about existence carries
//! the first word alone. The run carries the whole line, because the rest of it
//! is the user's own arguments.
//!
//! What belongs to one key stays with that key: its variable, its default name,
//! its deadline, and the words it leaves under the frame. `G` keeps those in
//! [`crate::issue`].

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use shellquote::shell_quote;
use tempfile::NamedTempFile;

use crate::child::detach_from_terminal;
use crate::lines::LineSplitter;

/// A command that a key of watch mode runs.
///
/// A newtype rather than a `String`, because the value holds one rule that
/// every reader of it depends on: it is never empty. An empty name asks the
/// shell about nothing, and it runs nothing. The rule gives the type its
/// second guarantee for free — a value with no space at either end and some
/// character in it always has a first word, which is what
/// [`ShellCommand::probe_word`] returns.
///
/// The value is a whole command line and not one name. `wn` reads
/// `WN_START_COMMAND` the same way, and it is the precedent these variables
/// were added against, so `gh issue view --web` must work here as `gh issue
/// develop` works there. The two halves of the value go to two different
/// places: the shell answers `command -v` about the first word, and it runs
/// the whole line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellCommand(String);

impl ShellCommand {
    /// The command that `value` names, or `None` where the feature is off.
    ///
    /// `value` is the value of the key's own variable, which the caller reads.
    /// The environment is process-global state, and this function takes the
    /// value as an argument so a test of it touches no such state.
    ///
    /// An absent value gives `default`, which is the name the key carries where
    /// the environment names none. A value with nothing but space in it turns
    /// the key off, which is the one way to say "do not do this at all" on a
    /// public repository whose defaults name one person's shell functions.
    ///
    /// **A `default` of nothing but space gives `None` as well.** The one rule
    /// of this type is that the value is never empty, and the default arrives
    /// through a parameter — so a caller must not be able to break the rule
    /// through it.
    ///
    /// The space at each end goes and the words inside stay, so `gh issue view
    /// --web` goes in whole. [`ShellCommand::probe_word`] takes the first word
    /// off it for the question the shell can answer.
    pub(crate) fn new(value: Option<&str>, default: &str) -> Option<Self> {
        let named = value.unwrap_or(default).trim();
        (!named.is_empty()).then(|| Self(named.to_string()))
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
    /// feature off in [`ShellCommand::new`], so every value that reaches here
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
    use crate::issue::DEFAULT_ISSUE_COMMAND;

    /// The name that `value` resolves to, as a plain string, or `None`.
    fn resolved(value: Option<&str>) -> Option<String> {
        ShellCommand::new(value, DEFAULT_ISSUE_COMMAND).map(|command| command.name().to_string())
    }

    /// The word that the probe asks about, for `value`, or `None`.
    fn probe_word(value: Option<&str>) -> Option<String> {
        ShellCommand::new(value, DEFAULT_ISSUE_COMMAND)
            .map(|command| command.probe_word().to_string())
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
    fn a_default_of_nothing_but_space_names_no_command() {
        // The one rule of this type is that the value is never empty, and the
        // default arrives through a parameter — so the rule must hold against
        // the default as well as against the value. A key whose own default is
        // empty is a key with no command behind it, which is the answer an
        // empty variable already gets.
        assert_eq!(
            ShellCommand::new(None, ""),
            None,
            "an empty default must name no command",
        );
        assert_eq!(
            ShellCommand::new(None, "   "),
            None,
            "a default with nothing but space in it must name no command",
        );
        assert_eq!(
            ShellCommand::new(Some("  "), "\t"),
            None,
            "a value of space and a default of space together must name no command",
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
pub(crate) const PROBE_DEADLINE: Duration = Duration::from_secs(5);

/// How often the probe looks to see whether the shell has answered.
///
/// The probe runs once for each `gsw` process, on a thread that does nothing
/// else, so the cost of looking is paid once and it delays no frame.
pub(crate) const PROBE_POLL: Duration = Duration::from_millis(25);

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
/// the environment of `gsw`, except the six that a user states on purpose.
///
/// **The rule is the `GIT_` prefix, and never a list of names.** The command
/// the user supplies runs git, or a program that runs git: the command of `G`
/// asks `gh` about the issue, and `gh` reads the origin remote of the directory
/// it runs in. Many variables move that answer, and they are not one family:
/// `GIT_DIR`, `GIT_WORK_TREE` and `GIT_INDEX_FILE` aim git at another
/// repository, `GIT_COMMON_DIR` moves the files git reads outside a worktree —
/// config and refs among them — `GIT_CEILING_DIRECTORIES` stops the walk that
/// finds a repository at all, and `GIT_CONFIG_PARAMETERS` sets any key it
/// likes. A list that named all of those today would still be a list, and it
/// strips nothing new the day git adds a variable. So this calls
/// [`gitscratch::shed_inherited_git_environment_keeping_user_intent`], which
/// enumerates [`std::env::vars_os`] and removes every name that starts with
/// `GIT_`, except six. That rule is written once, in the crate that states it,
/// and it is tested there.
///
/// **The sweep keeps the six names of
/// [`gitscratch::USER_INTENT_GIT_ENVIRONMENT`], because this child acts for the
/// user.** A `gsw` started from inside a pre-commit hook holds `GIT_DIR`,
/// `GIT_INDEX_FILE`, `GIT_PREFIX` and `GIT_CONFIG_PARAMETERS`, and the user
/// asked for none of them. But a user also exports `GIT_SSH_COMMAND` or
/// `GIT_CONFIG_GLOBAL` at the prompt, through direnv, or through a shim, and
/// the rc file that `-i` reads does not state those again. The command of `R`
/// and `M` pushes, so a child without them fails to authenticate, or rebases
/// under the wrong identity.
pub(crate) fn shell_child(shell: &OsStr, script: String) -> Command {
    let mut command = Command::new(shell);
    command.arg("-ic").arg(script);
    gitscratch::shed_inherited_git_environment_keeping_user_intent(&mut command);
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
fn probe_command(shell: &OsStr, command: &ShellCommand) -> Command {
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
fn probe_with_deadline(shell: &OsStr, command: &ShellCommand, deadline: Duration) -> bool {
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

/// The command a key runs, where the environment names one and the shell has
/// it.
///
/// `value` is the value of the key's own variable and `default` is the name it
/// carries where that variable is unset.
///
/// Blocking: it starts a shell and waits for it. The caller runs it on a
/// thread of its own.
///
/// A value that turns the feature off starts no shell at all, which is what
/// keeps a public repository from asking every user's shell about one person's
/// function.
pub(crate) fn resolve(value: Option<&str>, default: &str, shell: &OsStr) -> Option<ShellCommand> {
    let command = ShellCommand::new(value, default)?;
    probe_with_deadline(shell, &command, PROBE_DEADLINE).then_some(command)
}

/// The last line of `lines` that has text in it, with the space after it
/// dropped.
///
/// The last one, because a command says what it was doing and then says why it
/// stopped. The one with text in it, because a command that ends its last line
/// with a newline leaves an empty line after it, and an empty row under the
/// frame reads as a run that said nothing.
///
/// It takes whatever yields the lines, rather than a slice of them, because the
/// callers hold a run differently: `G` keeps one string for each row it read,
/// and `R` keeps one string with a newline between each row and the one before
/// it. A slice would make the second caller copy the whole run of a pre-push
/// hook to read one line of it.
pub(crate) fn last_with_text<'a, Lines>(lines: Lines) -> Option<String>
where
    Lines: IntoIterator<Item = &'a str>,
    Lines::IntoIter: DoubleEndedIterator,
{
    lines
        .into_iter()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim_end().to_string())
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
/// after the first word is the arguments the user wrote into the key's own
/// variable, and the shell reads them here the way it reads them at an
/// interactive prompt: it splits them at each space, it expands a variable, and
/// it matches a pattern against file names. That is what the user asked for. A
/// value of `gh issue view --web` is four words to the shell, and one quoted
/// word to a shell would be a command no machine has.
///
/// The caller hands this child to [`start_run`], which attaches the two files
/// the child writes to.
pub(crate) fn run_command(shell: &OsStr, command: &ShellCommand, workdir: &Path) -> Command {
    let mut child = shell_child(shell, command.name().to_string());
    // Every command a key runs reads the repository of the directory it runs
    // in. The command of `G` asks `gh` about the issue, and `gh` reads the
    // origin remote of that directory.
    child.current_dir(workdir).stdin(Stdio::null());
    child
}

/// Which stream of a child a line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputStream {
    /// Where the child writes what it did.
    Stdout,
    /// Where the child writes why it stopped.
    Stderr,
}

/// How [`RunInFlight::wait`] ended.
#[derive(Debug)]
pub(crate) enum RunEnd {
    /// The child exited, and every line it wrote has been reported.
    Exited(ExitStatus),
    /// The child was still running at the deadline. The caller owns it now,
    /// and the caller must reap it.
    StillRunning(Child),
}

/// A run in flight: the shell, and the two files it writes to.
///
/// Every key that runs a command the user supplies runs it through this, so
/// the rules of a run are written once.
///
/// **The files are files, and never pipes.** A pipe is read to its end, and the
/// end arrives only when the last writer lets go — so a child the command
/// leaves behind holds the run open long after the shell is gone. `xdg-open`
/// leaves such a child, and so does a push that starts a credential helper. A
/// file has no such wait, and it also cannot fill up and stop the child the way
/// a pipe that nobody reads does: a pre-push hook that builds a workspace
/// writes far more than a pipe holds.
pub(crate) struct RunInFlight {
    /// The shell, which is what the run waits for.
    child: Child,
    /// Where the child writes what it did.
    stdout: Stream,
    /// Where the child writes why it stopped.
    stderr: Stream,
}

/// Start `child`, with a file of `scratch` for each of its two streams.
///
/// The caller builds the child, because each key sets it up in its own way.
/// Where the output goes is the same for every key, so this sets it.
pub(crate) fn start_run(mut child: Command, scratch: &Path) -> std::io::Result<RunInFlight> {
    let stdout = Stream::new(scratch)?;
    let stderr = Stream::new(scratch)?;
    child.stdout(stdout.writer()?).stderr(stderr.writer()?);
    Ok(RunInFlight {
        child: child.spawn()?,
        stdout,
        stderr,
    })
}

impl RunInFlight {
    /// Wait for the child to exit, and give each line to `on_line` as it lands.
    ///
    /// Each poll reads standard output and then standard error. The window
    /// under the frame is the whole reason a poll reads at all: a pre-push hook
    /// takes minutes, and a user who sees nothing for those minutes cannot tell
    /// a slow hook from a hang.
    ///
    /// With a `deadline`, the wait stops there and gives the child back, still
    /// running. It kills nothing. With no deadline, the wait ends only when the
    /// child exits.
    ///
    /// The last read also gives the line each stream left unterminated.
    ///
    /// # Errors
    ///
    /// The error of a child that cannot be asked about.
    pub(crate) fn wait(
        mut self,
        deadline: Option<Duration>,
        on_line: &mut dyn FnMut(OutputStream, String),
    ) -> std::io::Result<RunEnd> {
        let give_up_at = deadline.map(|deadline| Instant::now() + deadline);
        loop {
            if let Some(status) = self.child.try_wait()? {
                // What the command wrote between the last poll and its exit.
                self.drain(on_line);
                self.finish(on_line);
                return Ok(RunEnd::Exited(status));
            }
            self.drain(on_line);
            if give_up_at.is_some_and(|give_up_at| Instant::now() >= give_up_at) {
                self.finish(on_line);
                return Ok(RunEnd::StillRunning(self.child));
            }
            std::thread::sleep(PROBE_POLL);
        }
    }

    /// Give `on_line` every line each stream completed since the last read.
    fn drain(&mut self, on_line: &mut dyn FnMut(OutputStream, String)) {
        for line in self.stdout.new_lines() {
            on_line(OutputStream::Stdout, line);
        }
        for line in self.stderr.new_lines() {
            on_line(OutputStream::Stderr, line);
        }
    }

    /// Give `on_line` the line each stream left unterminated.
    fn finish(&mut self, on_line: &mut dyn FnMut(OutputStream, String)) {
        if let Some(line) = self.stdout.finish() {
            on_line(OutputStream::Stdout, line);
        }
        if let Some(line) = self.stderr.finish() {
            on_line(OutputStream::Stderr, line);
        }
    }
}

/// One stream of a run: the file the child writes to, and the handle gsw reads
/// it through.
///
/// **The two handles are two handles on purpose.** The child writes at the
/// offset it inherited, and this reads at an offset of its own — so every read
/// gives the bytes that arrived since the last one, and neither side moves the
/// other's place in the file.
struct Stream {
    /// The handle the child writes through.
    ///
    /// gsw writes nothing to it. It is held open so that the file lives from
    /// the moment its name goes to the moment the child has a handle of its
    /// own.
    writer: File,
    /// The handle gsw reads through, which has an offset of its own.
    reader: File,
    /// What turns the bytes of this stream into lines.
    ///
    /// **One splitter for each stream, and it lives as long as the stream
    /// does.** A splitter holds the bytes of a line that has no terminator yet,
    /// from one read to the next, and a read stops wherever the child happened
    /// to be. So a splitter made afresh for each read reports that place as the
    /// end of a line: a command drawing a progress bar has its stale state
    /// taken for a row, and a line cut in the middle of a character arrives as
    /// two rows with a replacement character between them. A splitter shared
    /// between the two streams is the other half of the same rule, and
    /// [`crate::lines::LineSplitter`] states it: the tail of one stream would
    /// join the first line of the other.
    splitter: LineSplitter,
}

impl Stream {
    /// A stream whose file is made in `scratch`, and whose name is then taken
    /// off it.
    ///
    /// **The name goes as soon as both handles are open.** A quit kills the
    /// thread of a run where it stands, and a thread that dies runs no
    /// destructor — so a file that still carries a name outlives the session
    /// that made it, and a rebase whose hook builds a workspace leaves a great
    /// deal of it behind. A file with no name is the same file: Unix keeps it
    /// for as long as a process holds it open, so the child goes on writing and
    /// the reader goes on reading, and the space comes back the moment the last
    /// of them lets go.
    ///
    /// The read handle is opened by name, so it is opened before the name goes.
    /// It has an offset of its own, which is what makes each read give the bytes
    /// that arrived since the last one.
    ///
    /// **A name that cannot be removed is no reason to refuse the run.** The
    /// file is there and both handles are open, so the run works exactly as it
    /// always did and the only cost is one file left in a temporary directory —
    /// which is what every run cost before this. To fail here would take the
    /// key away instead.
    fn new(scratch: &Path) -> std::io::Result<Self> {
        let file = NamedTempFile::new_in(scratch)?;
        let reader = file.reopen()?;
        let (writer, path) = file.into_parts();
        let _ = path.close();
        Ok(Self {
            writer,
            reader,
            splitter: LineSplitter::new(),
        })
    }

    /// Every line the child has completed since the last read.
    fn new_lines(&mut self) -> Vec<String> {
        let bytes = self.new_bytes();
        self.splitter.feed(&bytes)
    }

    /// The line the child left unterminated, once gsw reads no more.
    ///
    /// A command that exits without a final newline still said something, and
    /// to drop it is to lose the last line of every command that ends that way.
    fn finish(&mut self) -> Option<String> {
        self.splitter.finish()
    }

    /// Where the child writes this stream.
    fn writer(&self) -> std::io::Result<Stdio> {
        Ok(Stdio::from(self.writer.try_clone()?))
    }

    /// The bytes the child has written since the last read.
    ///
    /// A read that fails ends this pass with what it has. The run is still
    /// going, so the next poll asks again, and an error invented here would put
    /// words in the command's mouth.
    fn new_bytes(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            match self.reader.read(&mut buffer) {
                // The end of the file as it stands. The child writes more after
                // this, and the next read starts where this one stopped.
                Ok(0) => break,
                Ok(read) => bytes.extend_from_slice(&buffer[..read]),
                // A signal arrived mid-read. Nothing was lost and nothing is
                // wrong.
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        bytes
    }
}

#[cfg(all(test, unix))]
mod run_tests {
    use super::stub_shell::{
        a_child_of_this_test_passes, kill_now, shed_git_lines, test_name,
        test_process_can_open_the_terminal, user_intent_lost, user_intent_value, StubShell,
        ANSWER_DEADLINE, CHILD_RAN, GAVE_UP_WITHIN, HOSTILE_GIT_ENVIRONMENT, HOSTILE_MARKER,
        TTY_REFUSED,
    };
    use super::*;
    use crate::issue::{run, DEFAULT_ISSUE_COMMAND};
    use std::path::PathBuf;
    use std::sync::mpsc::channel;

    /// `path` with every symbolic link in it resolved.
    ///
    /// macOS reaches a temporary directory through a symbolic link, so the
    /// path the shell prints is not the path this test asked for. Both sides
    /// are resolved before they are compared.
    fn resolved(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).expect("resolve the path")
    }

    /// The default command, which is what every test here runs.
    fn default_command() -> ShellCommand {
        ShellCommand::new(None, DEFAULT_ISSUE_COMMAND).expect("the default names a command")
    }

    #[test]
    fn the_run_names_the_command_with_no_quoting_around_it() {
        // Quoting the word stops a shell from expanding an alias, and an alias
        // is one of the two things the probe reports.
        let stub = StubShell::answering(0);
        let workdir = tempfile::tempdir().expect("tempdir");
        let command = ShellCommand::new(Some("myfunc"), DEFAULT_ISSUE_COMMAND).expect("a name");
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
        let command =
            ShellCommand::new(Some("gh issue view --web"), DEFAULT_ISSUE_COMMAND).expect("a name");
        let _ = run(stub.as_shell(), &command, workdir.path());
        let runs = stub.runs();
        assert!(
            runs.lines().any(|line| line == "gh issue view --web"),
            "the script must be the whole value: {runs:?}",
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
        // controlling terminal reads the keys the event thread of `gsw` waits
        // for and paints over the frame. No process in the tree of this child
        // can be able to open the terminal.
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
            let outcome = run(&shell, &default_command(), &dir);
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

    /// Each child sheds the `GIT_` variables of `gsw`, and keeps the ones a user
    /// states on purpose.
    ///
    /// The user states `GIT_SSH_COMMAND` or `GIT_CONFIG_GLOBAL` in the session
    /// too, at the prompt or through direnv, and not only in the rc file. An rc
    /// file that the interactive child reads again does not state those, so the
    /// sweep must keep them.
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
    /// holds each hostile variable and each variable of the user. An assertion
    /// that a variable is absent from the stub passes just as readily where
    /// there was nothing to remove.
    ///
    /// **The stub records what the children got.** A test that reads the
    /// removals off the [`Command`] proves less: a sweep of the `GIT_` prefix
    /// records a removal only for a variable this process holds, so such a test
    /// is vacuous under a shell and meaningful under a hook. The stub is the
    /// environment the child really ran in.
    ///
    /// Both children are covered here, the probe and the run, because both come
    /// from [`shell_child`] and the rule is a property of that one function.
    /// Each child has a stub of its own, so a child that kept a variable does
    /// not hide a child that lost it.
    #[test]
    fn each_child_sheds_the_git_variables_of_gsw_and_keeps_those_of_the_user() {
        if std::env::var_os(HOSTILE_MARKER).is_none() {
            a_child_of_this_test_passes(&test_name(
                module_path!(),
                "each_child_sheds_the_git_variables_of_gsw_and_keeps_those_of_the_user",
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
        for name in gitscratch::USER_INTENT_GIT_ENVIRONMENT {
            assert_eq!(
                std::env::var(name).ok(),
                Some(user_intent_value(name)),
                "the child must really hold {name}, or there is nothing here to keep",
            );
        }

        let probe_stub = StubShell::answering(0);
        let run_stub = StubShell::answering(0);
        let workdir = tempfile::tempdir().expect("tempdir");
        assert!(
            probe_with_deadline(probe_stub.as_shell(), &default_command(), ANSWER_DEADLINE),
            "the stub answers 0, so the probe must report the command present",
        );
        let outcome = run(run_stub.as_shell(), &default_command(), workdir.path());
        assert_eq!(
            outcome.message(),
            None,
            "the stub exits 0, so the run must report nothing",
        );

        for (child, stub) in [("probe", &probe_stub), ("run", &run_stub)] {
            let environment = stub.environment();
            assert!(
                !environment.is_empty(),
                "the {child} must record the environment it ran in",
            );
            let carried = shed_git_lines(&environment);
            assert!(
                carried.is_empty(),
                "the {child} carried a git variable out of the environment of gsw. The command \
                 asks `gh` about the issue, and `gh` reads the origin remote of the directory it \
                 runs in - so each of these aims that question, or configures it, somewhere the \
                 user never pointed it: {carried:?}",
            );
            let lost = user_intent_lost(&environment, None);
            assert!(
                lost.is_empty(),
                "the {child} lost a git variable the user states on purpose. Without \
                 GIT_SSH_COMMAND a user who holds a non-default key cannot authenticate, and \
                 without GIT_CONFIG_GLOBAL git reads a configuration the user replaced: {lost:?}",
            );
        }

        println!("{CHILD_RAN}");
    }
}

#[cfg(all(test, unix))]
pub(crate) mod stub_shell {
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
    pub(crate) const ANSWER_DEADLINE: Duration = PROBE_DEADLINE;

    /// The deadline for the stub that hangs.
    ///
    /// This is the one test that waits for a deadline, so the number is small.
    /// [`StubShell::new`] pays the slow first start before the test begins, so
    /// a second of it is a second the stub is already running in.
    pub(crate) const HANG_DEADLINE: Duration = Duration::from_secs(1);

    /// Longer than [`HANG_DEADLINE`] and far shorter than the `sleep` the
    /// hanging stub holds. A probe that waits for the shell crosses it.
    pub(crate) const GAVE_UP_WITHIN: Duration = Duration::from_secs(15);

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
    pub(crate) struct StubShell {
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
        /// The file a stub that waits for a gate waits for, written by the
        /// test rather than by the stub.
        gate: PathBuf,
    }

    /// Where a stub's tail names the file it records its process id in.
    const PID_FILE: &str = "<PID_FILE>";

    /// Where a stub's tail names the file it waits for.
    ///
    /// A third placeholder of the shape [`PID_FILE`] has, and it travels the
    /// other way: the test writes this file, and the stub reads it.
    const GATE_FILE: &str = "<GATE_FILE>";

    /// Most times a stub looks for its gate before it gives up.
    ///
    /// The bound is the point. A stub that waited for a gate nobody opens holds
    /// the suite for the life of the session, and the test that opens the gate
    /// is exactly the test that can fail before it gets there.
    const GATE_POLLS: u32 = 200;

    /// How long a stub sleeps between two looks at its gate, in seconds.
    ///
    /// [`GATE_POLLS`] of these is twenty seconds, which is longer than
    /// [`GAVE_UP_WITHIN`] — so a test gives up first, and the stub that ends
    /// after it says which of the two happened.
    const GATE_POLL: &str = "0.1";

    /// The `printf` that is a program rather than a builtin of the shell.
    ///
    /// A builtin writes through the stdio of the shell, which buffers a whole
    /// block when the output is a file. A separate process flushes when it
    /// ends, so what it wrote is in the file the moment it is gone.
    const EXTERNAL_PRINTF: &str = "/usr/bin/printf";

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
    pub(crate) const TTY_OPENED: &str = "opened";

    /// What a stub writes when `/dev/tty` was unopenable.
    ///
    /// This is the only outcome that stops an interactive shell. Given a
    /// terminal, such a shell paints a prompt over the frame of `gsw` and takes
    /// the keys `gsw` reads.
    pub(crate) const TTY_REFUSED: &str = "refused";

    /// Whether the **test process** can open the controlling terminal.
    ///
    /// An assertion about a child is worth making only where there is a
    /// terminal for that child to be denied. A `cargo test` started from a
    /// script, from a runner, or from the pre-commit hook of this repository
    /// has no controlling terminal at all. `/dev/tty` is then unopenable for
    /// every process in the tree, detached or not, and the assertion holds for
    /// a reason that has nothing to do with the code. A test that reads this
    /// skips with a printed reason rather than bank such a vacancy as a green.
    pub(crate) fn test_process_can_open_the_terminal() -> bool {
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
        pub(crate) fn new(tail: &str) -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("stub-shell");
            let runs = dir.path().join("runs");
            let environment = dir.path().join("environment");
            let pid = dir.path().join("pid");
            let cwd = dir.path().join("cwd");
            let tty = dir.path().join("tty");
            let gate = dir.path().join("gate");
            let tail = tail
                .replace(PID_FILE, &shell_quote(&pid.display().to_string()))
                .replace(TTY_FILE, &shell_quote(&tty.display().to_string()))
                .replace(GATE_FILE, &shell_quote(&gate.display().to_string()));
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
                gate,
            };
            stub.warm();
            stub
        }

        /// Start the script once, so the test that follows does not pay for
        /// the first start of it.
        pub(crate) fn warm(&self) {
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
        pub(crate) fn answering(status: u8) -> Self {
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
        pub(crate) fn probing_the_terminal() -> Self {
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
        pub(crate) fn hanging() -> Self {
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
        pub(crate) fn hanging_in_a_child() -> Self {
            Self::new(&format!("sleep 30 &\necho $! > {PID_FILE}\nwait"))
        }

        /// A stub that writes `said` and then never exits.
        ///
        /// A command that says why it stopped, and then hangs, is the shape
        /// that makes those words worth a row at a deadline. The shell writes
        /// them before it replaces itself, so they are in the file the moment
        /// the deadline arrives.
        pub(crate) fn hanging_after_saying(said: &str) -> Self {
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
        pub(crate) fn outlived_by_a_child() -> Self {
            Self::new(&format!("sleep 30 &\necho $! > {PID_FILE}\nexit 0"))
        }

        /// A stub that writes `said`, waits for its gate, and then exits 0.
        ///
        /// This is the shape that proves a line reached the caller **while the
        /// command was still running.** The caller opens the gate when it sees
        /// the line, so a runner that reports nothing until the child exits
        /// waits for a gate that nobody opens.
        ///
        /// **The wait is bounded**, at [`GATE_POLLS`] looks of [`GATE_POLL`]
        /// each, so such a run ends by itself rather than holding the suite.
        /// The stub exits 1 where the gate never opened, which says in the
        /// outcome which of the two happened.
        ///
        /// **The words go through an external `printf`.** A builtin writes
        /// through the stdio of the shell, and stdio buffers a whole block when
        /// the output is a file — so the line would sit in that buffer until
        /// the shell exited, which is the very thing this stub exists to rule
        /// out. A separate process flushes when it ends.
        pub(crate) fn saying_then_waiting_for_a_gate(said: &str) -> Self {
            Self::new(&format!(
                "{EXTERNAL_PRINTF} '%s\\n' {}\n{}\nexit 0",
                shell_quote(said),
                Self::gate_wait(),
            ))
        }

        /// A stub that says `said`, starts a row it draws over, waits for its
        /// gate, and then draws that row again and ends it.
        ///
        /// `states` is what the row reads before the gate and after it. A
        /// command that draws a progress bar writes exactly this: a carriage
        /// return takes the cursor back to column zero, and the next state is
        /// printed over the one before it, so the row the user sees is the last
        /// state and never the ones under it.
        ///
        /// **The two writes are two writes on purpose.** A reader that starts a
        /// splitter afresh for each read reports the state it happened to stop
        /// on as a row of its own, and the gate is what makes the split between
        /// the reads a fact rather than a race.
        pub(crate) fn redrawing_a_row(said: &str, states: [&str; 2]) -> Self {
            Self::new(&format!(
                "{EXTERNAL_PRINTF} '%s\\n%s' {} {}\n{}\n{EXTERNAL_PRINTF} '\\r%s\\n' {}\nexit 0",
                shell_quote(said),
                shell_quote(states[0]),
                Self::gate_wait(),
                shell_quote(states[1]),
            ))
        }

        /// The shell that waits for [`GATE_FILE`], and exits 1 where it never
        /// arrived.
        ///
        /// It carries no exit of its own for the gate that opened, so a caller
        /// can put more work after it.
        fn gate_wait() -> String {
            format!(
                "i=0\n\
                 while [ $i -lt {GATE_POLLS} ]; do\n\
                 \t[ -f {GATE_FILE} ] && break\n\
                 \tsleep {GATE_POLL}\n\
                 \ti=$((i + 1))\n\
                 done\n\
                 [ -f {GATE_FILE} ] || exit 1",
            )
        }

        /// Open the gate of a stub that waits for one.
        pub(crate) fn open_gate(&self) {
            std::fs::write(&self.gate, b"open").expect("open the gate");
        }

        /// Wait for the hanging stub to record its process id.
        ///
        /// The stub writes the file, and the probe kills the stub. Which of
        /// the two happens first is the machine's business, so a test that
        /// reads the file waits for it rather than assuming it is there.
        pub(crate) fn wait_for_pid(&self) -> i32 {
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

        /// Wait for the stub to record a run, and report whether it did.
        ///
        /// A record says the child is running, so every file the runner made
        /// for it exists by now. The wait is bounded, and it gives an answer
        /// rather than a panic, so a test can open a gate before it asserts.
        pub(crate) fn wait_for_a_run(&self) -> bool {
            let give_up_at = Instant::now() + GAVE_UP_WITHIN;
            while self.runs().is_empty() {
                if Instant::now() >= give_up_at {
                    return false;
                }
                std::thread::sleep(PROBE_POLL);
            }
            true
        }

        /// The stub, as the path to give the probe.
        pub(crate) fn as_shell(&self) -> &OsStr {
            self.path.as_os_str()
        }

        /// Every argument of every run, one for each line, or the empty string
        /// where the stub never ran.
        pub(crate) fn runs(&self) -> String {
            std::fs::read_to_string(&self.runs).unwrap_or_default()
        }

        /// The environment of every run.
        pub(crate) fn environment(&self) -> String {
            std::fs::read_to_string(&self.environment).unwrap_or_default()
        }

        /// The directory of the last run, as the shell reported it.
        pub(crate) fn cwd(&self) -> PathBuf {
            let recorded = std::fs::read_to_string(&self.cwd).expect("the stub must record a cwd");
            PathBuf::from(recorded.trim_end())
        }

        /// What the stub found when it reached for the controlling terminal.
        ///
        /// The read fails where the file is absent, and an absent file means
        /// the stub never ran. A test that took that for an answer passes and
        /// proves nothing at all, so the message names that case.
        pub(crate) fn terminal_record(&self) -> String {
            std::fs::read_to_string(&self.tty)
                .expect("the stub never ran, so the terminal probe proved nothing")
                .trim()
                .to_string()
        }

        /// The process id the hanging stub holds.
        pub(crate) fn recorded_pid(&self) -> i32 {
            std::fs::read_to_string(&self.pid)
                .expect("the stub must record its process id")
                .trim()
                .parse()
                .expect("the recorded process id must be a number")
        }
    }

    /// Whether a process of `pid` still exists.
    pub(crate) fn alive(pid: i32) -> bool {
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
    pub(crate) fn wait_until_gone(pid: i32) -> bool {
        let give_up_at = Instant::now() + GAVE_UP_WITHIN;
        while alive(pid) {
            if Instant::now() >= give_up_at {
                return false;
            }
            std::thread::sleep(PROBE_POLL);
        }
        true
    }

    /// The name of everything in `dir`, in order.
    pub(crate) fn entries_of(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("read the directory")
            .map(|entry| {
                entry
                    .expect("an entry of the directory")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    /// The prefix that makes a variable git's.
    ///
    /// The rule the children hold is this prefix, and never a list of names, so
    /// a test that reads their environment back asks about the prefix too. A
    /// test that asked about a list of names would pass for a variable the list
    /// forgot.
    pub(crate) const GIT_PREFIX: &str = "GIT_";

    /// The variable that tells this test binary it is the child, and that it
    /// must do the work rather than start a child of its own.
    ///
    /// The name carries no [`GIT_PREFIX`], so the sweep under test leaves it
    /// alone and the child can still read it.
    pub(crate) const HOSTILE_MARKER: &str = "GSW_HOSTILE_GIT_ENVIRONMENT";

    /// The line a child prints after its last assertion holds.
    ///
    /// libtest exits 0 when a filter names no test, so a child that ran nothing
    /// reads exactly like a child that passed. The parent looks for this line
    /// as well as for the exit status.
    pub(crate) const CHILD_RAN: &str = "gsw-hostile-environment-child-ran";

    /// How long the parent waits for the child.
    ///
    /// The stub shell answers at once, so a healthy child takes milliseconds.
    /// The bound is here for the child that hangs: a test that waits for such a
    /// child holds the run for the life of the session.
    const CHILD_DEADLINE: Duration = Duration::from_secs(30);

    /// A hostile environment: every variable that aims git, or configures it,
    /// or stops it from finding a repository at all, and that no user states
    /// on purpose. No child of `gsw` may carry one of these.
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
    /// `GIT_CONFIG_PARAMETERS` sets any key at all.
    ///
    /// The child also holds each name of
    /// [`gitscratch::USER_INTENT_GIT_ENVIRONMENT`], with the value
    /// [`user_intent_value`] gives it. Those names come from the constant and
    /// not from this table, so a name gitscratch adds is tested here too.
    pub(crate) const HOSTILE_GIT_ENVIRONMENT: [(&str, &str); 7] = [
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
    ];

    /// The value the hostile environment gives `name`, a name of
    /// [`gitscratch::USER_INTENT_GIT_ENVIRONMENT`].
    ///
    /// Each value holds its own name, so a child that kept the name with
    /// another value fails as surely as a child that lost it. The value is not
    /// `0`, so a run that sets `GIT_TERMINAL_PROMPT=0` after the sweep shows
    /// that its own value wins. The path names nothing on this machine, and no
    /// git in the child connects to a host or asks a question, so no git reads
    /// the value as a program or as a boolean.
    pub(crate) fn user_intent_value(name: &str) -> String {
        format!("/gsw-user-intent/{name}")
    }

    /// The `NAME=value` lines of `environment` that carry a `GIT_` variable no
    /// user states on purpose.
    ///
    /// The rule is the [`GIT_PREFIX`] with the names of
    /// [`gitscratch::USER_INTENT_GIT_ENVIRONMENT`] taken out, which is the rule
    /// the children hold.
    pub(crate) fn shed_git_lines(environment: &str) -> Vec<&str> {
        environment
            .lines()
            .filter(|line| {
                line.split_once('=').is_some_and(|(key, _)| {
                    key.starts_with(GIT_PREFIX)
                        && !gitscratch::USER_INTENT_GIT_ENVIRONMENT.contains(&key)
                })
            })
            .collect()
    }

    /// The names of [`gitscratch::USER_INTENT_GIT_ENVIRONMENT`] that
    /// `environment` does not hold with the value [`user_intent_value`] gives
    /// them, `except` left out.
    pub(crate) fn user_intent_lost(environment: &str, except: Option<&str>) -> Vec<&'static str> {
        gitscratch::USER_INTENT_GIT_ENVIRONMENT
            .iter()
            .copied()
            .filter(|name| Some(*name) != except)
            .filter(|name| {
                let kept = format!("{name}={}", user_intent_value(name));
                !environment.lines().any(|line| line == kept)
            })
            .collect()
    }

    /// Start this test binary again, with `test` named and the hostile
    /// environment on it, and fail where that child fails.
    ///
    /// **A `GIT_` variable is process-global state**, so a test that sets one
    /// in this process changes what every other test in this binary reads, and
    /// several of them run real git. A child holds an environment of its own,
    /// so the hostile values reach the code under test and reach nothing else.
    /// That is also what makes the result the same under a shell and under the
    /// pre-commit hook of this repository, which exports `GIT_` variables into
    /// `cargo test`: the child sets the whole list itself.
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
    ///
    /// # Panics
    ///
    /// Panics where the child cannot be started, where it fails, where it hangs
    /// past [`CHILD_DEADLINE`], and where it printed no [`CHILD_RAN`].
    pub(crate) fn a_child_of_this_test_passes(test: &str) {
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
        for name in gitscratch::USER_INTENT_GIT_ENVIRONMENT {
            command.env(name, user_intent_value(name));
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

    /// The name of the test `function` of `module`, the way libtest spells it.
    ///
    /// `module` is the `module_path!()` of the caller, which starts with the
    /// name of the crate. A test name does not, so the first part goes. The
    /// rest comes from the compiler, so a module that moves needs no edit at
    /// the call site.
    pub(crate) fn test_name(module: &str, function: &str) -> String {
        let module = module.split_once("::").map_or(module, |(_, rest)| rest);
        format!("{module}::{function}")
    }

    /// End the process of `pid` now.
    ///
    /// A test that proves `gsw` leaves a process running owns that process
    /// afterwards. The suite cleans up what it started, so the test that
    /// asked for the process is the one that ends it.
    pub(crate) fn kill_now(pid: i32) {
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
    use crate::issue::DEFAULT_ISSUE_COMMAND;

    /// The command the default name resolves to.
    fn default_command() -> ShellCommand {
        ShellCommand::new(None, DEFAULT_ISSUE_COMMAND).expect("the default names a command")
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
        // take the keys the event thread of `gsw` waits for.
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
            resolve(Some(""), DEFAULT_ISSUE_COMMAND, stub.as_shell()),
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
        let command = ShellCommand::new(Some("myfunc"), DEFAULT_ISSUE_COMMAND).expect("a name");
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
        let command =
            ShellCommand::new(Some("gh issue view --web"), DEFAULT_ISSUE_COMMAND).expect("a name");
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
