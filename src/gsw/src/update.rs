//! Bringing the branch up to date with the base, from watch mode.
//!
//! The `R` key rebases the branch onto the base, and the `M` key merges the
//! base into the branch. Neither act is gsw's own. Each key runs one command
//! that the user supplies, in the user's own interactive shell, and that
//! command pushes the branch when it has finished. [`crate::shell`] holds the
//! shell, the type a command name becomes, and the probe that asks whether the
//! command exists.
//!
//! What is here is what belongs to these two keys alone: the variable that
//! names each command, the name each key falls back on, the line the shell
//! runs, and the run itself.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};

use shellquote::shell_quote;
use tempfile::NamedTempFile;

use crate::lines::LineSplitter;
use crate::push::PushOutcome;
use crate::shell::{shell_child, ShellCommand, PROBE_POLL};

/// The variable that holds the command `R` runs.
///
/// The value is a whole command line, so it can carry arguments. See
/// [`ShellCommand`] for what gsw does with each part of it.
pub(crate) const REBASE_COMMAND_ENV: &str = "GSW_REBASE_COMMAND";

/// The variable that holds the command `M` runs.
pub(crate) const MERGE_COMMAND_ENV: &str = "GSW_MERGE_COMMAND";

/// The command `R` runs when the environment names none.
///
/// This repository ships no `grp`. It is a shell function that the user
/// supplies, and it is the default here because it is the name one person's rc
/// file gives it. [`REBASE_COMMAND_ENV`] names a different one, and a value of
/// nothing but space turns the key off.
pub(crate) const DEFAULT_REBASE_COMMAND: &str = "grp";

/// The command `M` runs when the environment names none. See
/// [`DEFAULT_REBASE_COMMAND`] for why this repository ships no such command.
pub(crate) const DEFAULT_MERGE_COMMAND: &str = "gmp";

/// Which act a key of watch mode asks the user's command to carry out.
///
/// **The value answers for its own key.** Two keys run the same code over two
/// sets of words, so every difference between them is a method here: the
/// variable that names the command, the command the key falls back on, the
/// letter the user presses, the verb every message about the act uses, and the
/// advice a refused run gives. A caller that matched on the act a second time
/// to pick one of those would be the place the two sets drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BaseUpdate {
    /// Put the commits of the branch on top of the base, then push. `R`.
    Rebase,
    /// Bring the base into the branch as a commit of its own, then push. `M`.
    Merge,
}

impl BaseUpdate {
    /// Both acts, for a caller that does the same work for each of them.
    ///
    /// The probe starts one shell for each key at startup, and the tests here
    /// state a rule once and hold it for both. An array rather than an
    /// iterator, so a variant added later fails to compile here rather than
    /// going quietly unprobed.
    pub(crate) const ALL: [Self; 2] = [Self::Rebase, Self::Merge];

    /// The variable that names the command this act runs.
    pub(crate) const fn env(self) -> &'static str {
        match self {
            Self::Rebase => REBASE_COMMAND_ENV,
            Self::Merge => MERGE_COMMAND_ENV,
        }
    }

    /// The command this act runs where its variable names none.
    pub(crate) const fn default_command(self) -> &'static str {
        match self {
            Self::Rebase => DEFAULT_REBASE_COMMAND,
            Self::Merge => DEFAULT_MERGE_COMMAND,
        }
    }

    /// The key the user presses for this act.
    ///
    /// Both letters are capitals. A rebase rewrites the commits of the branch
    /// and a merge writes a commit, so neither belongs on a key that a hand
    /// resting on the keyboard reaches by accident.
    pub(crate) const fn key(self) -> char {
        match self {
            Self::Rebase => 'R',
            Self::Merge => 'M',
        }
    }

    /// The word every message about this act uses for it.
    pub(crate) const fn verb(self) -> &'static str {
        match self {
            Self::Rebase => "rebase",
            Self::Merge => "merge",
        }
    }

    /// What a refused run tells the user to do.
    ///
    /// The letter comes from [`BaseUpdate::key`] rather than from a string of
    /// its own, so the advice names the key that exists rather than the key
    /// that existed when somebody wrote the sentence. Pressing it again asks
    /// the question against the repository as it stands now, which is the whole
    /// remedy — the same remedy [`crate::push`] gives, in the same words.
    pub(crate) fn retry_advice(self) -> String {
        format!("press {} again", self.key())
    }
}

/// A confirmed rebase or merge: the act, the branch the question named, the
/// base it named, and the command the user supplied.
///
/// The four are one value because they are one sentence — rebase *this branch*
/// onto *this base*, with *this command*. The command alone does not say which
/// branch: `grp` reads HEAD when the shell starts it, and that is not
/// necessarily what HEAD pointed at when the question went on the screen. The
/// answer arrives whenever the user presses `y`, and a checkout in another pane
/// fits in between. Carrying the branch beside the command is what lets [`run`]
/// refuse a repository that moved on.
///
/// Built only by the question of this module, so a command nobody confirmed
/// cannot be assembled somewhere else and handed to the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BaseUpdateCommand {
    /// Which act the question asked about.
    update: BaseUpdate,
    /// The branch the question named, as [`crate::repo::branch_name`] reports
    /// it.
    branch: String,
    /// The base the question named, which is the last word of the script.
    base: String,
    /// The command the user supplied, arguments and all.
    command: ShellCommand,
}

impl BaseUpdateCommand {
    /// The command that runs `command` for `update` against `base`, on
    /// `branch`.
    ///
    /// Private on purpose. The question of this module is the only caller, so
    /// every value of this type describes a question that a user answered.
    fn new(
        update: BaseUpdate,
        branch: impl Into<String>,
        base: impl Into<String>,
        command: ShellCommand,
    ) -> Self {
        Self {
            update,
            branch: branch.into(),
            base: base.into(),
            command,
        }
    }

    /// Which act the question asked about.
    pub(crate) fn update(&self) -> BaseUpdate {
        self.update
    }

    /// The branch the question named.
    pub(crate) fn branch(&self) -> &str {
        &self.branch
    }

    /// The base the question named.
    pub(crate) fn base(&self) -> &str {
        &self.base
    }

    /// The command the user supplied, which is what every message names.
    pub(crate) fn command(&self) -> &ShellCommand {
        &self.command
    }

    /// The line the shell runs: the whole command line, then the base.
    ///
    /// **The base goes on the line, and it goes last.** `grp` and `gmp` fall
    /// back on `main`, so a bare `grp` fails in a repository whose base is
    /// `master`. The question names the base, and what the user confirms is
    /// what runs, so the name of the base belongs in the line rather than in
    /// the defaults of somebody's shell function. Last, because everything in
    /// front of it is the user's own arguments: a value of `grp --fork-point`
    /// runs `grp --fork-point 'main'`.
    ///
    /// **The base is quoted and the command line is not.** The two halves come
    /// from two places. The command line is what the user wrote into the
    /// variable, and the shell reads it here the way it reads it at an
    /// interactive prompt — see [`crate::shell`], which states that rule for
    /// every key that runs a command. The base is a branch name gsw read out of
    /// a repository, and git accepts a great deal in one: a name that carries a
    /// space, a quotation mark, or a semicolon would otherwise leave the line
    /// as several words, and a semicolon would leave it as several commands.
    /// [`shell_quote`] makes it one word whatever is in it.
    pub(crate) fn script(&self) -> String {
        format!("{} {}", self.command.name(), shell_quote(&self.base))
    }
}

/// Run `command` on a thread of its own and hand the outcome to `on_finish`.
///
/// Off the render thread on purpose, as a push is. A rebase runs a pre-push
/// hook and then a push, which takes minutes, and the watch loop is what keeps
/// the refresh countdown moving, the ages advancing, and a resize repainting.
/// To block it for those minutes is to freeze the monitor at the moment the
/// user is watching it.
///
/// Both callbacks run on that thread. The one production caller sends each
/// line and the outcome down the loop's own channel, so they re-enter the loop
/// the way every other event does — applied between frames rather than during
/// one.
///
/// Takes the whole [`BaseUpdateCommand`] by value, so the branch the question
/// named crosses onto the thread with the script and [`run`] can still refuse a
/// repository that moved on in the meantime.
pub(crate) fn spawn<L, F>(
    shell: OsString,
    command: BaseUpdateCommand,
    workdir: PathBuf,
    on_line: L,
    on_finish: F,
) where
    L: Fn(String) + Send + 'static,
    F: FnOnce(PushOutcome) + Send + 'static,
{
    std::thread::spawn(move || on_finish(run(&shell, &command, &workdir, &on_line)));
}

/// The variable that decides whether git asks a question at the terminal.
///
/// gsw holds the alternate screen in raw mode, so a git that can ask would read
/// the same keys the event thread is reading, behind a question gsw never drew.
/// Set to `0`, git fails at once and says why, and that reason reaches the row
/// like every other failure.
const TERMINAL_PROMPT_VAR: &str = "GIT_TERMINAL_PROMPT";

/// Run `command` in `workdir` to its end, reporting each line as it lands.
///
/// The blocking half of [`spawn`], separated so it can be tested against a stub
/// shell without a thread or a channel in the way.
///
/// The outcome is a [`PushOutcome`], which is what lets the row report a rebase
/// the way it reports a push: both are a command that either worked or wrote a
/// reason.
///
/// **There is no deadline.** A pre-push hook in this workspace builds and tests
/// a workspace, which takes minutes, and `grp` runs one. The run ends when the
/// shell exits.
fn run(
    shell: &OsStr,
    command: &BaseUpdateCommand,
    workdir: &Path,
    on_line: &dyn Fn(String),
) -> PushOutcome {
    run_in(shell, command, workdir, &std::env::temp_dir(), on_line)
}

/// [`run`], with the directory that holds the two files of the run named.
///
/// The directory is a parameter so a test can watch it: the rule that no file
/// of a run keeps a name while the run is in flight is a rule about a
/// directory, and a test that read the whole temporary directory of the machine
/// would read every other program's files as well. Production passes
/// [`std::env::temp_dir`].
fn run_in(
    shell: &OsStr,
    command: &BaseUpdateCommand,
    workdir: &Path,
    scratch: &Path,
    on_line: &dyn Fn(String),
) -> PushOutcome {
    let name = command.command().name();

    let mut run = match start(shell, command, workdir, scratch) {
        Ok(run) => run,
        // The shell is gone, it cannot be started, or there is nowhere to put
        // what it writes. Rare, and worth saying plainly: every other failure
        // here is the command's own words.
        Err(error) => {
            return PushOutcome {
                success: false,
                output: format!("cannot run {name}: {error}"),
            }
        }
    };

    let mut record = Record::new();
    let status = loop {
        match run.child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                // Read where the command stands, then wait. The window under
                // the frame is the whole reason a poll happens at all: a
                // pre-push hook takes minutes, and a user who sees nothing for
                // those minutes cannot tell a slow hook from a hang.
                drain(&mut run.stdout, &mut record, on_line);
                drain(&mut run.stderr, &mut record, on_line);
                std::thread::sleep(PROBE_POLL);
            }
            // The child cannot be asked about, so nothing can be waited for
            // either. Saying so plainly is the answer a shell that cannot be
            // started gets.
            Err(error) => {
                return PushOutcome {
                    success: false,
                    output: format!("cannot wait for {name}: {error}"),
                }
            }
        }
    };

    // What the command wrote between the last poll and its exit.
    drain(&mut run.stdout, &mut record, on_line);
    drain(&mut run.stderr, &mut record, on_line);

    PushOutcome {
        success: status.success(),
        output: record.into_text(),
    }
}

/// Report every line `stream` has written since the last read, and keep it.
///
/// The record and the caller get the same lines in the same order, because both
/// are fed here. A drain that also gave its own text back would be a second
/// account of one stream, and to join two such accounts is what puts the verdict
/// of a command in the middle of its output rather than at the end.
fn drain(stream: &mut Stream, record: &mut Record, on_line: &dyn Fn(String)) {
    for line in painted(&stream.new_bytes()) {
        record.push(&line);
        on_line(line);
    }
}

/// A run in flight: the shell, and the two files it writes to.
///
/// **The files are files, and never pipes.** A pipe is read to its end, and the
/// end arrives only when the last writer lets go — so a child the command
/// leaves behind holds the run open long after the shell is gone. `grp` pushes,
/// and a push starts a credential helper or an agent that does exactly that. A
/// file has no such wait, and it also cannot fill up and stop the child the way
/// a pipe that nobody reads does: a pre-push hook that builds a workspace
/// writes far more than a pipe holds.
struct RunInFlight {
    /// The shell, which is what the run waits for.
    child: Child,
    /// Where the child writes what it did.
    stdout: Stream,
    /// Where the child writes why it stopped.
    stderr: Stream,
}

/// One stream of a run: the file the child writes to, and the handle gsw reads
/// it through.
///
/// **The two handles are two handles on purpose.** The child writes at the
/// offset it inherited, and this reads at an offset of its own — so every read
/// gives the bytes that arrived since the last one, and neither side moves the
/// other's place in the file.
struct Stream {
    /// The file itself. Dropping it takes the file away, and a child that still
    /// holds it goes on writing to a file with no name, which costs the space
    /// only until that child ends.
    file: NamedTempFile,
    /// The handle gsw reads through, which has an offset of its own.
    reader: File,
}

impl Stream {
    /// A stream whose file is made in `scratch`.
    fn new(scratch: &Path) -> std::io::Result<Self> {
        let file = NamedTempFile::new_in(scratch)?;
        let reader = file.reopen()?;
        Ok(Self { file, reader })
    }

    /// Where the child writes this stream.
    fn writer(&self) -> std::io::Result<Stdio> {
        Ok(Stdio::from(self.file.as_file().try_clone()?))
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

/// Start `command` in `workdir`, with a file of `scratch` for each of its two
/// streams.
fn start(
    shell: &OsStr,
    command: &BaseUpdateCommand,
    workdir: &Path,
    scratch: &Path,
) -> std::io::Result<RunInFlight> {
    let stdout = Stream::new(scratch)?;
    let stderr = Stream::new(scratch)?;

    // The child is interactive, it carries no `GIT_` variable out of the
    // environment of gsw, and it is detached from the terminal — see
    // [`shell_child`], which states all three rules and is the one place they
    // are written.
    let mut builder = shell_child(shell, command.script());
    builder
        .current_dir(workdir)
        .stdin(Stdio::null())
        // **After the sweep, so it survives it.** The sweep removes every
        // `GIT_` variable this process holds, and a value placed ahead of it
        // would leave with the rest. A user who exports the variable again in
        // the rc file still wins, because the rc file loads inside the child
        // after this value was placed.
        .env(TERMINAL_PROMPT_VAR, "0")
        .stdout(stdout.writer()?)
        .stderr(stderr.writer()?);

    Ok(RunInFlight {
        child: builder.spawn()?,
        stdout,
        stderr,
    })
}

/// Every line a run has written, in arrival order, as one string with a newline
/// between each line and the one before it.
///
/// **One growing string, and not one [`String`] for each line.** A pre-push
/// hook that builds and tests a workspace prints hundreds of thousands of
/// lines, and a vector of them is a heap allocation each — then one more copy
/// of the whole run to join them at the end, for a record that
/// [`crate::push::failure_lines`] reads three lines of and a run that worked
/// reads none of. Appending in place costs the growth of one buffer instead,
/// and the text it holds is what `join("\n")` gives, byte for byte: the
/// separator goes *between* the lines, so there is no newline at the end and a
/// run that said nothing leaves the empty string behind.
///
/// The flag beside the text is what the text alone cannot say. "The buffer is
/// still empty" is not the question "has a line landed yet": a line can *be*
/// empty — an `echo ""` in a hook keeps its row, by the rule
/// [`crate::lines::LineSplitter`] states — and to ask the buffer would swallow
/// the newline that belongs after such a first line.
struct Record {
    /// What every line said, with a newline between one line and the next.
    text: String,
    /// Whether any line at all has landed.
    any_line_recorded: bool,
}

impl Record {
    /// A record of a run that has said nothing yet.
    fn new() -> Self {
        Self {
            text: String::new(),
            any_line_recorded: false,
        }
    }

    /// Add one line to the end of the record.
    fn push(&mut self, line: &str) {
        if self.any_line_recorded {
            self.text.push('\n');
        }
        self.any_line_recorded = true;
        self.text.push_str(line);
    }

    /// Add every line of `lines` to the end of the record, in order.
    fn extend(&mut self, lines: impl IntoIterator<Item = String>) {
        for line in lines {
            self.push(&line);
        }
    }

    /// The record, as the text an outcome carries.
    fn into_text(self) -> String {
        self.text
    }
}

/// `bytes` as lines gsw can paint.
///
/// [`crate::lines::LineSplitter`] is the one place a child's bytes become such
/// text — a tab is up to eight columns and an escape sequence repaints the
/// frame in another program's colors.
fn painted(bytes: &[u8]) -> Vec<String> {
    let mut splitter = LineSplitter::new();
    let mut lines = splitter.feed(bytes);
    lines.extend(splitter.finish());
    lines
}

#[cfg(test)]
mod script_tests {
    use super::*;

    /// The command a question about `update` on `issue-12` would have carried,
    /// for a variable holding `value` and a base of `base`.
    fn confirmed(update: BaseUpdate, value: &str, base: &str) -> BaseUpdateCommand {
        BaseUpdateCommand::new(
            update,
            "issue-12",
            base,
            ShellCommand::new(Some(value), update.default_command()).expect("a name"),
        )
    }

    #[test]
    fn the_script_ends_with_the_base_as_a_quoted_word() {
        // `grp` falls back on `main`, so a repository whose base is `master`
        // gets `no such branch or commit: 'main'` from a bare `grp`.
        assert_eq!(
            confirmed(BaseUpdate::Rebase, "grp", "master").script(),
            "grp 'master'",
        );
        assert_eq!(
            confirmed(BaseUpdate::Merge, "gmp", "main").script(),
            "gmp 'main'",
        );
    }

    #[test]
    fn the_arguments_of_the_command_stay_in_front_of_the_base() {
        // Everything the user wrote after the first word is an argument of
        // their own command, and the base is an argument gsw adds after them.
        assert_eq!(
            confirmed(BaseUpdate::Rebase, "grp --fork-point", "main").script(),
            "grp --fork-point 'main'",
        );
    }

    #[test]
    fn a_base_that_carries_a_semicolon_is_still_one_word() {
        // git takes a branch name with a semicolon in it, and the shell reads
        // an unquoted semicolon as the end of one command and the start of the
        // next. The quotation is what keeps the name a name.
        assert_eq!(
            confirmed(BaseUpdate::Rebase, "grp", "main;touch pwned").script(),
            "grp 'main;touch pwned'",
        );
    }
}

#[cfg(all(test, unix))]
mod run_tests {
    use super::*;
    use crate::shell::stub_shell::{
        a_child_of_this_test_passes, kill_now, test_name, test_process_can_open_the_terminal,
        StubShell, CHILD_RAN, GAVE_UP_WITHIN, GIT_PREFIX, HOSTILE_GIT_ENVIRONMENT, HOSTILE_MARKER,
        TTY_REFUSED,
    };
    use crate::testrepo::{git, init_repo};
    use std::sync::mpsc::{channel, Receiver};
    use std::time::Instant;
    use tempfile::TempDir;

    /// The base every fixture here names, which is also the name `grp` and
    /// `gmp` fall back on.
    const BASE: &str = "main";

    /// The branch every fixture here was confirmed on.
    const BRANCH: &str = "issue-12";

    /// A confirmed rebase of [`BRANCH`] onto [`BASE`], running `value`.
    fn confirmed(value: &str) -> BaseUpdateCommand {
        BaseUpdateCommand::new(
            BaseUpdate::Rebase,
            BRANCH,
            BASE,
            ShellCommand::new(Some(value), DEFAULT_REBASE_COMMAND).expect("a name"),
        )
    }

    /// A confirmed rebase running the default command.
    fn default_command() -> BaseUpdateCommand {
        confirmed(DEFAULT_REBASE_COMMAND)
    }

    /// [`run`] for a test that does not read what arrived while it ran, which
    /// is every test here but the ones about the live output.
    fn run_quiet(shell: &OsStr, command: &BaseUpdateCommand, workdir: &Path) -> PushOutcome {
        run(shell, command, workdir, &|_| {})
    }

    /// A repository checked out on [`BRANCH`], which is what every run here
    /// acts on.
    ///
    /// A real repository rather than an empty directory, because the run reads
    /// the branch before it starts a shell: a directory that is no repository
    /// reports no branch, and every test here would then measure the refusal
    /// rather than the run.
    fn work_tree() -> TempDir {
        let dir = init_repo();
        git(dir.path(), &["checkout", "-q", "-b", BRANCH]);
        dir
    }

    /// `path` with every symbolic link in it resolved.
    ///
    /// macOS reaches a temporary directory through a symbolic link, so the path
    /// the shell prints is not the path this test asked for. Both sides are
    /// resolved before they are compared.
    fn resolved(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).expect("resolve the path")
    }

    #[test]
    fn the_run_hands_the_whole_script_to_an_interactive_shell() {
        // The command is a shell function, so only a shell that read the rc
        // file can find it. The script is the whole line the question
        // described, base and all.
        let stub = StubShell::answering(0);
        let workdir = work_tree();
        let _ = run_quiet(
            stub.as_shell(),
            &confirmed("grp --fork-point"),
            workdir.path(),
        );
        let runs = stub.runs();
        assert!(
            runs.lines().any(|line| line == "-ic"),
            "the shell must be interactive, or it has no functions: {runs:?}",
        );
        assert!(
            runs.lines().any(|line| line == "grp --fork-point 'main'"),
            "the shell must be given the script of the confirmation: {runs:?}",
        );
    }

    #[test]
    fn the_run_happens_in_the_work_tree() {
        // `grp` rebases the repository of the directory it runs in, and watch
        // mode moves between worktrees of one repository.
        let stub = StubShell::answering(0);
        let workdir = work_tree();
        let _ = run_quiet(stub.as_shell(), &default_command(), workdir.path());
        assert_eq!(resolved(&stub.cwd()), resolved(workdir.path()));
    }

    #[test]
    fn the_exit_status_of_the_shell_decides_the_outcome() {
        let workdir = work_tree();
        let worked = StubShell::answering(0);
        assert!(
            run_quiet(worked.as_shell(), &default_command(), workdir.path()).success,
            "a command that exits 0 rebased and pushed",
        );
        let failed = StubShell::answering(1);
        assert!(
            !run_quiet(failed.as_shell(), &default_command(), workdir.path()).success,
            "a command that exits 1 did not, and the row must say so",
        );
    }

    #[test]
    fn what_the_command_said_reaches_the_outcome() {
        // `grp` reports a skipped push in its last line, and a rebase that
        // stops on a conflict says why. Both are the whole reason the user
        // needs the row.
        let stub = StubShell::new(
            "echo 'rebased onto main'\necho 'no upstream - skipping push' >&2\nexit 1",
        );
        let workdir = work_tree();
        let outcome = run_quiet(stub.as_shell(), &default_command(), workdir.path());
        assert!(
            outcome.output.contains("rebased onto main"),
            "what the command said must reach the outcome: {:?}",
            outcome.output,
        );
        assert!(
            outcome.output.contains("no upstream - skipping push"),
            "why it stopped must reach the outcome too: {:?}",
            outcome.output,
        );
    }

    #[test]
    fn a_run_returns_when_the_shell_exits_and_not_when_its_children_do() {
        // A child the command leaves behind inherits where the output goes. A
        // pipe makes the run wait for end of file, and end of file arrives only
        // when the last writer lets go — so such a child holds the run open
        // long after the shell is gone, and the key is held for all of it. A
        // file has no such wait. `grp` pushes, and a push starts a credential
        // helper or an agent that outlives the command that asked it.
        //
        // The run happens on a thread of its own, so this test reports the
        // defect rather than a wait of its own: a run that waits for the child
        // never returns inside the bound, and the channel says so.
        let stub = StubShell::outlived_by_a_child();
        let workdir = work_tree();
        let shell = stub.as_shell().to_os_string();
        let dir = workdir.path().to_path_buf();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(run(&shell, &default_command(), &dir, &|_| {}));
        });
        let outcome = rx
            .recv_timeout(GAVE_UP_WITHIN)
            .expect("the run must return when the shell exits, not when its children do");
        assert!(outcome.success, "the shell exited 0: {:?}", outcome.output,);
        kill_now(stub.wait_for_pid());
    }

    /// What a run reported, in the order its callbacks fired.
    ///
    /// One channel for both, so the test reads the two in the order the run
    /// produced them: a line that arrived after the outcome is a line this
    /// never sees before the outcome, which is the defect under test.
    #[derive(Debug)]
    enum Report {
        /// A line the command wrote.
        Line(String),
        /// The run ended.
        Done(PushOutcome),
    }

    /// What the gated stub says before it waits.
    const FIRST_LINE: &str = "Rebasing (1/3)";

    #[test]
    fn a_line_reaches_the_caller_before_the_command_exits() {
        // This is the whole of the live window. `grp` runs a pre-push hook that
        // builds and tests a workspace, which takes minutes, and output that
        // arrives only when the command exits is output that arrives when
        // nobody needs it any more.
        //
        // The stub says one line and then waits for a gate that this test opens
        // when it sees that line, so a runner that reports nothing until the
        // child exits waits for a gate that never opens. Every wait here is
        // bounded.
        let stub = StubShell::saying_then_waiting_for_a_gate(FIRST_LINE);
        let workdir = work_tree();
        let shell = stub.as_shell().to_os_string();
        let dir = workdir.path().to_path_buf();
        let (tx, rx) = channel();
        let line_tx = tx.clone();
        std::thread::spawn(move || {
            let outcome = run(&shell, &default_command(), &dir, &move |line| {
                let _ = line_tx.send(Report::Line(line));
            });
            let _ = tx.send(Report::Done(outcome));
        });

        let first = rx.recv_timeout(GAVE_UP_WITHIN);
        // The gate is opened before the assertion. A failed assertion ends the
        // test where it stands, and a stub that waits for a gate nobody opened
        // is the one thing that must not outlive it.
        stub.open_gate();
        match first {
            Ok(Report::Line(line)) => assert_eq!(
                line, FIRST_LINE,
                "the line the command wrote must arrive as the command wrote it",
            ),
            Ok(Report::Done(outcome)) => panic!(
                "the run reported the outcome before it reported the line: {:?}",
                outcome.output,
            ),
            Err(error) => {
                panic!("no line reached the caller while the command was still running: {error}",)
            }
        }

        let outcome = last_report(&rx);
        assert!(
            outcome.success,
            "the stub exits 0 once the gate is open: {:?}",
            outcome.output,
        );
        assert!(
            outcome.output.contains(FIRST_LINE),
            "the record must keep the line as well: {:?}",
            outcome.output,
        );
    }

    /// The outcome of a run, read off `reports` past every line before it.
    ///
    /// Bounded, because a run that never ends would otherwise hold the suite
    /// for the life of the session.
    fn last_report(reports: &Receiver<Report>) -> PushOutcome {
        let give_up_at = Instant::now() + GAVE_UP_WITHIN;
        loop {
            let left = give_up_at.saturating_duration_since(Instant::now());
            match reports.recv_timeout(left).expect("the run must end") {
                Report::Line(_) => {}
                Report::Done(outcome) => return outcome,
            }
        }
    }

    #[test]
    fn the_run_child_cannot_open_the_controlling_terminal() {
        // Watch mode holds the alternate screen in raw mode, and this child is
        // an interactive shell. Such a shell opens `/dev/tty` for job control
        // and for every prompt it paints, so a child that keeps the controlling
        // terminal reads the keys the event thread of `gsw` waits for and
        // paints over the frame. The reach of this child is the longest of the
        // three: `grp` rebases and then pushes, and a push asks `ssh` for a key
        // and `ssh` asks the terminal for the passphrase.
        if !test_process_can_open_the_terminal() {
            eprintln!(
                "skipped: this test process has no controlling terminal, so /dev/tty is \
                 unopenable for every child regardless - the assertion would hold vacuously",
            );
            return;
        }

        let stub = StubShell::probing_the_terminal();
        let workdir = work_tree();
        let outcome = run_quiet(stub.as_shell(), &default_command(), workdir.path());
        assert!(outcome.success, "the stub exits 0, so the run must work");
        assert_eq!(
            stub.terminal_record(),
            TTY_REFUSED,
            "the run child keeps the controlling terminal, so the command of the user can paint \
             over the frame of gsw and take the keys gsw is reading",
        );
    }

    #[test]
    fn the_run_child_carries_no_git_variable_out_of_a_hostile_environment() {
        // **This test starts this test binary again, and the hostile
        // environment goes on that child.** A `GIT_` variable is
        // process-global state, and several tests of this binary run real git.
        //
        // The variables matter more here than anywhere else in gsw: `grp`
        // rebases the branch and then pushes it, so a leaked `GIT_DIR` or
        // `GIT_CONFIG_PARAMETERS` rewrites the history of a repository the user
        // never named and sends it to a remote.
        //
        // **The armed control comes first.** The child asserts that it really
        // holds each hostile variable. An assertion that a variable is absent
        // passes just as readily where there was nothing to remove.
        if std::env::var_os(HOSTILE_MARKER).is_none() {
            a_child_of_this_test_passes(&test_name(
                module_path!(),
                "the_run_child_carries_no_git_variable_out_of_a_hostile_environment",
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
        let workdir = work_tree();
        let outcome = run_quiet(stub.as_shell(), &default_command(), workdir.path());
        assert!(outcome.success, "the stub exits 0, so the run must work");

        let environment = stub.environment();
        assert!(
            !environment.is_empty(),
            "the child must record the environment it ran in",
        );
        let carried: Vec<&str> = environment
            .lines()
            .filter(|line| {
                line.split_once('=').is_some_and(|(key, _)| {
                    key.starts_with(GIT_PREFIX) && key != TERMINAL_PROMPT_VAR
                })
            })
            .collect();
        assert!(
            carried.is_empty(),
            "a child of gsw carried a git variable out of the environment of gsw. Each of these \
             aims the rebase and the push that follows it, or configures them, somewhere the user \
             never pointed them: {carried:?}",
        );

        println!("{CHILD_RAN}");
    }

    #[test]
    fn the_run_child_is_told_not_to_ask_at_the_terminal() {
        // The other half of the terminal rule, and git's own. `grp` pushes,
        // and git asks for an HTTP user name and password itself. Told not to,
        // it fails at once and says why, and that reason reaches the row like
        // every other failure.
        //
        // The value is set after the sweep, so it survives it. A user who
        // exports the variable again in the rc file still wins, because the rc
        // file loads inside the child after this value was placed.
        let stub = StubShell::answering(0);
        let workdir = work_tree();
        let _ = run_quiet(stub.as_shell(), &default_command(), workdir.path());
        let environment = stub.environment();
        assert!(
            environment
                .lines()
                .any(|line| line == format!("{TERMINAL_PROMPT_VAR}=0")),
            "git must be told not to ask at the terminal: {environment:?}",
        );
    }
}
