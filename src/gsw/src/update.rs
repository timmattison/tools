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
use crate::push::{
    confirm_hint, current_branch, Confirmed, PushOutcome, PushPrompt, SuccessReport,
};
use crate::render::{Operation, Snapshot};
use crate::repo::DETACHED_HEAD;
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

    /// The act that git is holding, for an operation it has not finished.
    ///
    /// git holds a rebase or a merge, and each of them is one of these — so the
    /// refusal that names an unfinished operation reads its word from
    /// [`BaseUpdate::verb`] like every other message. A rebase that git holds is
    /// then called a rebase under both keys, and by the word the `⚠ rebase` row
    /// of the header is already using.
    const fn held(operation: &Operation) -> Self {
        match operation {
            Operation::Rebase { .. } => Self::Rebase,
            Operation::Merge { .. } => Self::Merge,
        }
    }

    /// Whether the question about this act deserves the color of one the user
    /// must read twice.
    ///
    /// A rebase rewrites every commit of the branch, and the command then
    /// force-pushes the result — so a branch that somebody else has pulled is a
    /// branch they must repair. A merge writes one commit and pushes it, which
    /// is the routine act the count in the header is about, and a color of
    /// caution on every question is a color that says nothing.
    const fn caution(self) -> bool {
        match self {
            Self::Rebase => true,
            Self::Merge => false,
        }
    }

    /// What this act says where the repository offers no base for it to act on.
    ///
    /// The two sentences name the base that gsw was looking for, because the
    /// user can do something about that: a repository whose default branch is
    /// neither of those names is one these keys leave alone altogether.
    const fn no_base_refusal(self) -> &'static str {
        match self {
            Self::Rebase => "no main or master branch to rebase onto",
            Self::Merge => "no main or master branch to merge",
        }
    }

    /// The question this act asks about `branch` and `base`, with the branch
    /// `behind` commits behind the base and the user's `command` about to run.
    ///
    /// The two sentences sit side by side because they are the same sentence
    /// about two acts, and the order of the names is the difference between
    /// them: a rebase moves the branch onto the base, and a merge brings the
    /// base into the branch. The count goes with the base under both, because
    /// the base is what the branch is behind.
    ///
    /// Everything that is the same for both — the count, its unit, and the
    /// clause that names the command — is worked out above the match, so no
    /// later reader has to compare two sentences to see whether they agree.
    fn question(self, branch: &str, base: &str, behind: u32, command: &ShellCommand) -> String {
        // "1 commits behind" reads as a defect in the tool, right beside the
        // number it is about.
        let unit = if behind == 1 { "commit" } else { "commits" };
        let base = format!("{base} ({behind} {unit} behind)");
        // The whole value the user wrote into the variable, and not its first
        // word: a question that named `grp` alone would describe a different
        // run from the one `grp --fork-point` carries out.
        let name = command.name();
        match self {
            Self::Rebase => format!("Rebase {branch} onto {base}, then push with {name}?"),
            Self::Merge => format!("Merge {base} into {branch}, then push with {name}?"),
        }
    }

    /// What the row says while this act runs on `branch` and `base`, through
    /// the user's `command`, without the age [`crate::push`] puts after it.
    ///
    /// The same sentence as the question, in the tense of work in flight, and
    /// written beside it for the same reason: the order of the names is what
    /// tells a rebase from a merge. It names the command as well, because a
    /// run of minutes is a run the user has to be able to recognize.
    fn running_notice(self, branch: &str, base: &str, command: &ShellCommand) -> String {
        let name = command.name();
        match self {
            Self::Rebase => format!("Rebasing {branch} onto {base} with {name}…"),
            Self::Merge => format!("Merging {base} into {branch} with {name}…"),
        }
    }

    /// What the row says once this act has worked on `branch` and `base`
    /// through the user's `command`, without the age [`crate::push`] puts after
    /// it.
    ///
    /// The third tense of the one sentence, beside the other two. It names the
    /// command because the command decided what happened: `grp` rebases and
    /// force-pushes, and a sentence that said `Rebased` alone would leave the
    /// user to guess whether anything reached the remote.
    fn done_sentence(self, branch: &str, base: &str, command: &ShellCommand) -> String {
        let name = command.name();
        match self {
            Self::Rebase => format!("Rebased {branch} onto {base} with {name}"),
            Self::Merge => format!("Merged {base} into {branch} with {name}"),
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
        format!("{} {}", self.command.name(), shell_quote(self.base()))
    }
}

/// Decide what pressing the key of `update` does, given the branch state gsw
/// already renders.
///
/// A pure function of the snapshot, the act, and the command. It reads no
/// repository, it starts no shell, and it reads no clock — so every rule below
/// is testable against a value a test writes out, which is what a table of five
/// refusals needs.
///
/// **The only caller of [`BaseUpdateCommand::new`].** That is what keeps a
/// command nobody confirmed from being assembled somewhere else and handed to
/// [`run`]: a value of that type exists only where a user was shown these
/// words and answered them. It is the rule [`crate::push::PushCommand`] states
/// about [`crate::push::prompt_for`], and it is why this function lives here
/// rather than beside that one — the constructor stays private to the module
/// that owns the rules of `R` and `M`.
///
/// It gives the [`PushPrompt`] that `p` gives, because the row that shows the
/// question and the key that answers it are the row and the key of a push.
///
/// **The refusals are read in order, and the first that applies wins.** Several
/// of them describe one repository at once — a rebase that stopped on a
/// conflict is a detached HEAD *and* an operation in progress — and the order
/// puts the thing the user has to deal with first at the top. Each one posts a
/// fading line and asks nothing, because none of them is an error: they are the
/// repository saying that this key has nothing to do here.
pub(crate) fn base_update_prompt_for(
    snapshot: &Snapshot,
    update: BaseUpdate,
    command: &ShellCommand,
) -> PushPrompt {
    let branch = snapshot.branch.as_str();
    let base = snapshot.base.as_str();
    let refuse = |message: String| PushPrompt::Refuse { message };

    // No branch to act on. git refuses `HEAD` as the name of a branch, so
    // `grp` has nothing to rebase and `gmp` has nothing to merge into.
    if branch == DETACHED_HEAD {
        return refuse(format!(
            "{DETACHED_HEAD} is detached — check out a branch to {}",
            update.verb(),
        ));
    }
    // **No base these keys may act on.** `resolve_base` falls back on the
    // target of `origin/HEAD` and then on HEAD itself, and neither is a base
    // for this: a local `trunk` whose base resolves to `origin/trunk` would
    // have `grp` rebase the branch onto its own remote branch and then push the
    // default branch of the repository. The guard inside `grp` exists to stop
    // that push, and it compares two names, and here the names differ.
    if !crate::repo::DEFAULT_BASE_NAMES.contains(&base) {
        return refuse(update.no_base_refusal().to_string());
    }
    // The user is on the base itself, so there is no branch to bring up to
    // date. Above the count below it, because a branch is never behind itself
    // and `main already contains main` says nothing.
    if branch == base {
        return refuse(format!("on {base} — nothing to {}", update.verb()));
    }
    // git is holding an operation that the user must finish or abort, and gsw
    // does neither. The `⚠ rebase` row of the header is showing it already.
    if let Some(operation) = &snapshot.operation {
        return refuse(format!(
            "a {} is in progress — finish it first",
            BaseUpdate::held(operation).verb(),
        ));
    }
    // Nothing to bring over. The count in the header is the whole reason for
    // these keys, and at zero a rebase would rewrite every commit of the branch
    // and force-push the result for no gain.
    if snapshot.commits_behind == 0 {
        return refuse(format!("{branch} already contains {base}"));
    }

    PushPrompt::Confirm {
        question: update.question(branch, base, snapshot.commits_behind, command),
        hint: confirm_hint(update.verb()),
        caution: update.caution(),
        running_notice: update.running_notice(branch, base, command),
        command: Confirmed::BaseUpdate(BaseUpdateCommand::new(
            update,
            branch,
            base,
            command.clone(),
        )),
        // **The last line of the command goes under the sentence of gsw.**
        // `grp` and `gmp` report a push they skipped only there, so the
        // sentence alone would tell the user that the branch is on the remote
        // when it is not.
        success: SuccessReport::WithLastLine {
            sentence: update.done_sentence(branch, base, command),
        },
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
/// **There is no deadline.** `grp` pushes, and a pre-push hook of this
/// workspace builds and tests every crate in it, which takes minutes. The run
/// ends when the shell exits, and the notice counts the time — so a run that
/// hangs shows on the screen as a run that hangs.
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

    // **The branch is compared first, and a mismatch starts no shell.** A
    // question describes the repository as it stood when the key was pressed,
    // and the answer arrives whenever the user presses `y` — long enough for a
    // checkout in another pane to land in between. `grp` reads HEAD when the
    // shell starts it, so it would rebase a branch the question never named and
    // push it. The gap between this read and the shell's own is microseconds
    // rather than seconds, and nothing here closes it entirely, short of a lock
    // git does not offer.
    //
    // `None` means git could not be run at all. The run goes ahead in that
    // case, as a push does: to refuse here would blame a checkout that never
    // happened, and a git that cannot start rebases nothing either.
    if let Some(current) = current_branch(workdir) {
        if current != command.branch() {
            return PushOutcome {
                success: false,
                output: format!(
                    "branch changed from {} to {current} since the confirmation — {}",
                    command.branch(),
                    command.update().retry_advice(),
                ),
            };
        }
    }

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

    // What the command wrote between the last poll and its exit, and then the
    // line it left unterminated on each stream.
    drain(&mut run.stdout, &mut record, on_line);
    drain(&mut run.stderr, &mut record, on_line);
    for line in [run.stdout.finish(), run.stderr.finish()]
        .into_iter()
        .flatten()
    {
        record.push(&line);
        on_line(line);
    }

    let success = status.success();
    let mut text = record.into_text();
    if !success && text.trim().is_empty() {
        // A failure with nothing to show would paint a blank row, and a blank
        // row under the frame reads as a run that worked. The status is all the
        // command left, and the name is the whole line the user wrote into the
        // variable — a message that named the first word alone would report
        // `grp failed` for a run of `grp --fork-point`.
        text = format!("{name} failed ({status})");
    }

    PushOutcome {
        success,
        output: text,
    }
}

/// Report every line `stream` has written since the last read, and keep it.
///
/// The record and the caller get the same lines in the same order, because both
/// are fed here. A drain that also gave its own text back would be a second
/// account of one stream, and to join two such accounts is what puts the verdict
/// of a command in the middle of its output rather than at the end.
fn drain(stream: &mut Stream, record: &mut Record, on_line: &dyn Fn(String)) {
    for line in stream.new_lines() {
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

    /// The line the child left unterminated, once it has gone.
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
    // environment of gsw but the six a user states on purpose, and it is
    // detached from the terminal — see [`shell_child`], which states all three
    // rules and is the one place they are written.
    let mut builder = shell_child(shell, command.script());
    builder
        .current_dir(workdir)
        .stdin(Stdio::null())
        // **After the sweep, so it wins.** The sweep keeps the value of this
        // variable that gsw holds, and this call replaces that value with `0`.
        // A user who exports the variable again in the rc file still wins,
        // because the rc file loads inside the child after this value was
        // placed.
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
/// `push::failure_lines` reads three lines of and a run that worked
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

    /// The record, as the text an outcome carries.
    fn into_text(self) -> String {
        self.text
    }
}

#[cfg(test)]
mod question_tests {
    use super::*;
    use crate::render::Operation;
    use crate::repo::DETACHED_HEAD;

    /// The branch every question here is asked about.
    const BRANCH: &str = "issue-12";

    /// The base every question here names, which is also the name `grp` and
    /// `gmp` fall back on.
    const BASE: &str = "main";

    /// A snapshot of [`BRANCH`], `behind` commits behind [`BASE`], with nothing
    /// in progress.
    ///
    /// The question reads four fields of the snapshot — the branch, the base,
    /// the count, and the operation — so every other field here holds what a
    /// repository with no file and no commit gives.
    fn behind(behind: u32) -> Snapshot {
        Snapshot {
            branch: BRANCH.to_string(),
            base: BASE.to_string(),
            commits_ahead: 0,
            commits_behind: behind,
            files: Vec::new(),
            log: Vec::new(),
            upstream: None,
            operation: None,
            push_remote: None,
            worktree: None,
        }
    }

    /// What `update` puts on the row for `snapshot`, running the command that
    /// `value` names.
    fn prompt(snapshot: &Snapshot, update: BaseUpdate, value: &str) -> PushPrompt {
        base_update_prompt_for(
            snapshot,
            update,
            &ShellCommand::new(Some(value), update.default_command()).expect("a name"),
        )
    }

    /// The question `update` asks about `snapshot`, running the command that
    /// `value` names.
    ///
    /// # Panics
    ///
    /// Panics where the act is refused. A test that reads the question is a
    /// test about the words a user is shown, and a refusal shows none of them.
    fn question_running(snapshot: &Snapshot, update: BaseUpdate, value: &str) -> String {
        match prompt(snapshot, update, value) {
            PushPrompt::Confirm { question, .. } => question,
            PushPrompt::Refuse { message } => {
                panic!("the act must be offered, and it was refused with {message:?}")
            }
        }
    }

    /// The question `update` asks about `snapshot`, running the command it
    /// falls back on.
    ///
    /// # Panics
    ///
    /// Panics where the act is refused, as [`question_running`] does.
    fn question(snapshot: &Snapshot, update: BaseUpdate) -> String {
        question_running(snapshot, update, update.default_command())
    }

    /// The keys that answer the question `update` asks about `snapshot`.
    ///
    /// # Panics
    ///
    /// Panics where the act is refused, as [`question_running`] does.
    fn hint(snapshot: &Snapshot, update: BaseUpdate) -> String {
        match prompt(snapshot, update, update.default_command()) {
            PushPrompt::Confirm { hint, .. } => hint,
            PushPrompt::Refuse { message } => {
                panic!("the act must be offered, and it was refused with {message:?}")
            }
        }
    }

    /// Why `update` refuses to act on `snapshot`.
    ///
    /// # Panics
    ///
    /// Panics where the act is offered. A test that reads a refusal is a test
    /// about a key that must not run anything, and a question is exactly what
    /// it must not raise.
    fn refusal(snapshot: &Snapshot, update: BaseUpdate) -> String {
        match prompt(snapshot, update, update.default_command()) {
            PushPrompt::Refuse { message } => message,
            PushPrompt::Confirm { question, .. } => {
                panic!("the act must be refused, and it asked {question:?}")
            }
        }
    }

    #[test]
    fn the_question_names_the_act_the_branch_the_base_the_count_and_the_command() {
        // Every one of the five is what the user reads in the half second
        // before pressing `y`. The command is named because it belongs to the
        // user: `p` never force-pushes and `grp` does, so a question that left
        // the command out would let the user think the rule of `p` holds here.
        assert_eq!(
            question(&behind(5), BaseUpdate::Rebase),
            "Rebase issue-12 onto main (5 commits behind), then push with grp?",
        );
        assert_eq!(
            question(&behind(5), BaseUpdate::Merge),
            "Merge main (5 commits behind) into issue-12, then push with gmp?",
        );
    }

    #[test]
    fn a_count_of_one_takes_the_singular() {
        assert_eq!(
            question(&behind(1), BaseUpdate::Rebase),
            "Rebase issue-12 onto main (1 commit behind), then push with grp?",
        );
        assert_eq!(
            question(&behind(1), BaseUpdate::Merge),
            "Merge main (1 commit behind) into issue-12, then push with gmp?",
        );
    }

    #[test]
    fn the_question_names_the_whole_command_line_and_not_its_first_word() {
        // The value of the variable is a whole command line, as
        // [`ShellCommand::name`] gives it. A question that named the first word
        // alone would describe a run of `grp` and the shell would run `grp
        // --fork-point`.
        assert_eq!(
            question_running(&behind(2), BaseUpdate::Rebase, "grp --fork-point"),
            "Rebase issue-12 onto main (2 commits behind), then push with grp --fork-point?",
        );
    }

    #[test]
    fn the_hint_names_the_act_that_enter_carries_out() {
        // The hint is spelled out rather than the usual `[y/N]`, because Enter
        // confirms — so the word after `Enter =` is the whole promise. A hint
        // that said `push` under the `R` question would name the act of another
        // key on the one prompt in gsw that force-pushes.
        assert_eq!(
            hint(&behind(5), BaseUpdate::Rebase),
            "[y/Enter = rebase, n/Esc = cancel]",
        );
        assert_eq!(
            hint(&behind(5), BaseUpdate::Merge),
            "[y/Enter = merge, n/Esc = cancel]",
        );
    }

    #[test]
    fn a_detached_head_leaves_no_branch_to_act_on() {
        // git refuses `HEAD` as the name of a branch, so there is nothing for
        // `grp` to rebase and nothing for `gmp` to merge into. A rebase that
        // stopped on a conflict leaves HEAD exactly here.
        let detached = Snapshot {
            branch: DETACHED_HEAD.to_string(),
            ..behind(5)
        };
        assert_eq!(
            refusal(&detached, BaseUpdate::Rebase),
            "HEAD is detached — check out a branch to rebase",
        );
        assert_eq!(
            refusal(&detached, BaseUpdate::Merge),
            "HEAD is detached — check out a branch to merge",
        );
    }

    #[test]
    fn a_base_that_is_neither_main_nor_master_is_refused() {
        // `resolve_base` falls back on the target of `origin/HEAD`, and then on
        // HEAD itself. Take a local `trunk` whose base resolves to
        // `origin/trunk`: `grp` rebases `trunk` onto its own remote branch and
        // then pushes the default branch of the repository. The guard inside
        // `grp` exists to stop that push, and it compares two names, and here
        // the names differ. One key press must not get past it.
        let elsewhere = Snapshot {
            base: "origin/trunk".to_string(),
            ..behind(5)
        };
        assert_eq!(
            refusal(&elsewhere, BaseUpdate::Rebase),
            "no main or master branch to rebase onto",
        );
        assert_eq!(
            refusal(&elsewhere, BaseUpdate::Merge),
            "no main or master branch to merge",
        );
    }

    #[test]
    fn master_is_a_base_that_these_keys_act_on() {
        // The other name `resolve_base` chooses, and the whole reason the base
        // goes on the command line: `grp` falls back on `main`, so a bare `grp`
        // fails here with no such branch or commit.
        let on_master = Snapshot {
            base: "master".to_string(),
            ..behind(2)
        };
        assert_eq!(
            question(&on_master, BaseUpdate::Rebase),
            "Rebase issue-12 onto master (2 commits behind), then push with grp?",
        );
    }

    #[test]
    fn the_base_itself_has_nothing_to_bring_into_it() {
        // The user is on `main`, so there is no branch to bring up to date.
        //
        // **This row wins over the row below it**, which the same snapshot also
        // matches: a branch is never behind itself, so a rule that read the
        // count first would say `main already contains main` and leave the user
        // wondering which branch it meant.
        let on_base = Snapshot {
            branch: BASE.to_string(),
            ..behind(0)
        };
        assert_eq!(
            refusal(&on_base, BaseUpdate::Rebase),
            "on main — nothing to rebase",
        );
        assert_eq!(
            refusal(&on_base, BaseUpdate::Merge),
            "on main — nothing to merge",
        );
    }

    #[test]
    fn an_operation_in_progress_is_named_by_what_git_holds() {
        // A rebase that stopped on a conflict is a rebase the user must finish
        // or abort, and gsw runs neither. The words name the operation git
        // holds rather than the key that was pressed, so a press of `M` during
        // a rebase sends the user to the rebase that is there instead of to a
        // merge that nobody started. It is the operation the `⚠ rebase` row of
        // the header is already showing.
        let rebasing = Snapshot {
            operation: Some(Operation::Rebase {
                step: None,
                conflicts: 1,
            }),
            ..behind(5)
        };
        let merging = Snapshot {
            operation: Some(Operation::Merge { conflicts: 1 }),
            ..behind(5)
        };
        for update in BaseUpdate::ALL {
            assert_eq!(
                refusal(&rebasing, update),
                "a rebase is in progress — finish it first",
            );
            assert_eq!(
                refusal(&merging, update),
                "a merge is in progress — finish it first",
            );
        }
    }

    #[test]
    fn a_branch_that_is_not_behind_already_carries_the_base() {
        // The count in the header is the whole reason for these keys. At zero
        // there is nothing to bring over, and `grp` would rewrite every commit
        // of the branch and force-push the result for no gain at all.
        for update in BaseUpdate::ALL {
            assert_eq!(
                refusal(&behind(0), update),
                "issue-12 already contains main"
            );
        }
    }

    #[test]
    fn the_first_refusal_that_applies_wins() {
        // A rebase that stopped on a conflict detaches HEAD and leaves an
        // operation in progress, so two rows of the table describe it. The
        // higher row wins, because it names the thing the user has to deal with
        // first: there is no branch here to act on whatever git is holding.
        let stopped = Snapshot {
            branch: DETACHED_HEAD.to_string(),
            operation: Some(Operation::Rebase {
                step: None,
                conflicts: 1,
            }),
            ..behind(5)
        };
        assert_eq!(
            refusal(&stopped, BaseUpdate::Rebase),
            "HEAD is detached — check out a branch to rebase",
        );
    }
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
    use crate::repo::DETACHED_HEAD;
    use crate::shell::stub_shell::{
        a_child_of_this_test_passes, kill_now, shed_git_lines, test_name,
        test_process_can_open_the_terminal, user_intent_lost, user_intent_value, StubShell,
        CHILD_RAN, GAVE_UP_WITHIN, HOSTILE_GIT_ENVIRONMENT, HOSTILE_MARKER, TTY_REFUSED,
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

    /// A confirmed merge of [`BASE`] into [`BRANCH`], running the default
    /// command of that act.
    fn confirmed_merge() -> BaseUpdateCommand {
        BaseUpdateCommand::new(
            BaseUpdate::Merge,
            BRANCH,
            BASE,
            ShellCommand::new(None, DEFAULT_MERGE_COMMAND).expect("a name"),
        )
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

    /// The state a drawn row holds before the gate opens.
    const PROGRESS_FIRST: &str = "Writing objects:  12%";

    /// The state that same row holds after it, which is the state the user
    /// reads.
    const PROGRESS_LAST: &str = "Writing objects: 100%";

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

        let (_, outcome) = rest_of_the_run(&rx);
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

    #[test]
    fn a_row_a_command_draws_over_shows_its_newest_state_on_one_row() {
        // A push draws its progress by going back to column zero and printing
        // over the row it drew, so `12%` and `100%` are two states of one row
        // and never two rows. The window under the frame is six rows tall, and
        // a state that is already painted over must not spend one of them.
        //
        // The two writes land in two reads, because the gate opens between
        // them. A reader that starts a splitter afresh for each read reports
        // whatever state it stopped on, so the stale state takes a row of its
        // own — which is what this asserts against.
        let stub = StubShell::redrawing_a_row(FIRST_LINE, [PROGRESS_FIRST, PROGRESS_LAST]);
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
        // The gate is opened before the assertion, so the stub ends whatever
        // this test does next.
        stub.open_gate();
        assert!(
            matches!(&first, Ok(Report::Line(line)) if line == FIRST_LINE),
            "the line before the drawn row must arrive first: {first:?}",
        );

        let (lines, outcome) = rest_of_the_run(&rx);
        assert!(
            outcome.success,
            "the stub exits 0 once the gate is open: {:?}",
            outcome.output,
        );
        assert_eq!(
            lines,
            vec![PROGRESS_LAST.to_string()],
            "the drawn row must arrive once, in its newest state",
        );
    }

    /// Every line `reports` still carries, and the outcome that closes it.
    ///
    /// Bounded, because a run that never ends would otherwise hold the suite
    /// for the life of the session.
    ///
    /// # Panics
    ///
    /// Panics where no outcome arrives inside [`GAVE_UP_WITHIN`].
    fn rest_of_the_run(reports: &Receiver<Report>) -> (Vec<String>, PushOutcome) {
        let give_up_at = Instant::now() + GAVE_UP_WITHIN;
        let mut lines = Vec::new();
        loop {
            let left = give_up_at.saturating_duration_since(Instant::now());
            match reports.recv_timeout(left).expect("the run must end") {
                Report::Line(line) => lines.push(line),
                Report::Done(outcome) => return (lines, outcome),
            }
        }
    }

    #[test]
    fn no_file_of_a_run_keeps_a_name_while_the_run_is_in_flight_or_after_it() {
        // **A quit during a run kills the thread of that run where it stands,
        // and a thread that dies runs no destructor.** So a file that still
        // carries a name stays in the temporary directory for good, and a
        // rebase of a workspace whose hook writes a great deal leaves a great
        // deal of it. Unix keeps an open file that has lost its name, so the
        // child goes on writing and the reader goes on reading, and the space
        // comes back the moment the last of them lets go.
        //
        // The directory is this test's own, so what it reads is the two files
        // of this run and nothing else on the machine.
        let stub = StubShell::saying_then_waiting_for_a_gate(FIRST_LINE);
        let workdir = work_tree();
        let scratch = tempfile::tempdir().expect("tempdir");
        let shell = stub.as_shell().to_os_string();
        let dir = workdir.path().to_path_buf();
        let scratch_path = scratch.path().to_path_buf();
        let (tx, rx) = channel();
        let line_tx = tx.clone();
        std::thread::spawn(move || {
            let outcome = run_in(
                &shell,
                &default_command(),
                &dir,
                &scratch_path,
                &move |line| {
                    let _ = line_tx.send(Report::Line(line));
                },
            );
            let _ = tx.send(Report::Done(outcome));
        });

        // The line says the child is running and has already written, so the
        // two files of this run exist by now.
        let first = rx.recv_timeout(GAVE_UP_WITHIN);
        let in_flight = entries_of(scratch.path());
        // The gate is opened before the assertions, so the stub ends whatever
        // this test does next.
        stub.open_gate();
        assert!(
            matches!(&first, Ok(Report::Line(line)) if line == FIRST_LINE),
            "the run must be in flight and writing: {first:?}",
        );
        assert!(
            in_flight.is_empty(),
            "a file of a run in flight keeps a name, so a quit leaves it behind for good: \
             {in_flight:?}",
        );

        let (_, outcome) = rest_of_the_run(&rx);
        assert!(
            outcome.success,
            "the stub exits 0 once the gate is open: {:?}",
            outcome.output,
        );
        let afterwards = entries_of(scratch.path());
        assert!(
            afterwards.is_empty(),
            "no file may outlive the run: {afterwards:?}",
        );
    }

    /// The name of everything in `dir`, in order.
    fn entries_of(dir: &Path) -> Vec<String> {
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

    #[test]
    fn a_failure_that_said_nothing_names_the_command_and_the_status() {
        // A failure with nothing to show would paint a blank row, and a blank
        // row under the frame reads as a run that worked. The exit status is
        // all the command left, and the name is the whole value the user wrote
        // into the variable.
        let stub = StubShell::answering(1);
        let workdir = work_tree();
        let outcome = run_quiet(stub.as_shell(), &default_command(), workdir.path());
        assert!(!outcome.success, "the stub exits 1");
        assert_eq!(
            outcome.output, "grp failed (exit status: 1)",
            "a failure must always carry something to show",
        );
    }

    #[test]
    fn a_checkout_after_the_confirmation_refuses_the_run_and_starts_no_shell() {
        // The window the question opens. `R` reads the branch and the base
        // while `issue-12` is checked out, `y` arrives seconds later, and a
        // checkout in another pane lands in between. `grp` reads HEAD when the
        // shell starts it, so it would rebase a branch the question never
        // named and then push it. Nothing may run in that case.
        let stub = StubShell::answering(0);
        let workdir = work_tree();
        git(workdir.path(), &["checkout", "-q", BASE]);

        let outcome = run_quiet(stub.as_shell(), &default_command(), workdir.path());

        assert!(
            !outcome.success,
            "a run whose branch changed must not report success: {:?}",
            outcome.output,
        );
        assert!(
            outcome
                .output
                .contains("branch changed from issue-12 to main"),
            "the outcome must name both branches: {:?}",
            outcome.output,
        );
        assert!(
            outcome.output.contains("press R again"),
            "the outcome must say how to ask again: {:?}",
            outcome.output,
        );
        assert_eq!(stub.runs(), "", "a refused run must start no shell at all",);
    }

    #[test]
    fn a_detached_head_after_the_confirmation_refuses_the_run() {
        // The other way the checkout moves: a rebase, a bisect, or a plain
        // checkout of a commit leaves no branch at all. git refuses `HEAD` as
        // the name of a branch, so it can never match the name a question
        // carried.
        let stub = StubShell::answering(0);
        let workdir = work_tree();
        git(workdir.path(), &["checkout", "-q", "--detach"]);

        let outcome = run_quiet(stub.as_shell(), &default_command(), workdir.path());

        assert!(!outcome.success, "got {:?}", outcome.output);
        assert!(
            outcome.output.contains(DETACHED_HEAD),
            "the outcome must name what HEAD is now: {:?}",
            outcome.output,
        );
        assert_eq!(stub.runs(), "", "a refused run must start no shell at all");
    }

    #[test]
    fn a_refused_run_names_the_key_of_its_own_act() {
        // The advice belongs to the act, and not to a constant the push owns:
        // `p` says `press p again`, and a refused merge must say `press M
        // again` rather than send the user to the key of another feature.
        let stub = StubShell::answering(0);
        let workdir = work_tree();
        git(workdir.path(), &["checkout", "-q", BASE]);

        let outcome = run_quiet(stub.as_shell(), &confirmed_merge(), workdir.path());

        assert!(
            outcome.output.contains("press M again"),
            "a refused merge must name the merge key: {:?}",
            outcome.output,
        );
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
    fn the_run_child_sheds_the_git_variables_of_gsw_and_keeps_those_of_the_user() {
        // **This test starts this test binary again, and the hostile
        // environment goes on that child.** A `GIT_` variable is
        // process-global state, and several tests of this binary run real git.
        //
        // The variables matter more here than anywhere else in gsw: `grp`
        // rebases the branch and then pushes it, so a leaked `GIT_DIR` or
        // `GIT_CONFIG_PARAMETERS` rewrites the history of a repository the user
        // never named and sends it to a remote. The push is also why the child
        // keeps what the user states: without `GIT_SSH_COMMAND` a user who
        // holds a non-default key cannot authenticate, and without
        // `GIT_CONFIG_GLOBAL` the rebase writes every commit under the wrong
        // identity.
        //
        // `GIT_TERMINAL_PROMPT` is a name the user states, and the run sets it
        // to `0` after the sweep, so that value must win over the one the
        // child holds.
        //
        // **The armed control comes first.** The child asserts that it really
        // holds each hostile variable and each variable of the user. An
        // assertion that a variable is absent passes just as readily where
        // there was nothing to remove.
        if std::env::var_os(HOSTILE_MARKER).is_none() {
            a_child_of_this_test_passes(&test_name(
                module_path!(),
                "the_run_child_sheds_the_git_variables_of_gsw_and_keeps_those_of_the_user",
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
        assert_ne!(
            std::env::var(TERMINAL_PROMPT_VAR).ok().as_deref(),
            Some("0"),
            "the child must hold a {TERMINAL_PROMPT_VAR} other than 0, or the run's own value \
             wins over nothing",
        );

        let stub = StubShell::answering(0);
        let workdir = work_tree();
        let outcome = run_quiet(stub.as_shell(), &default_command(), workdir.path());
        assert!(outcome.success, "the stub exits 0, so the run must work");

        let environment = stub.environment();
        assert!(
            !environment.is_empty(),
            "the child must record the environment it ran in",
        );
        let carried = shed_git_lines(&environment);
        assert!(
            carried.is_empty(),
            "a child of gsw carried a git variable out of the environment of gsw. Each of these \
             aims the rebase and the push that follows it, or configures them, somewhere the user \
             never pointed them: {carried:?}",
        );
        let lost = user_intent_lost(&environment, Some(TERMINAL_PROMPT_VAR));
        assert!(
            lost.is_empty(),
            "the run child lost a git variable the user states on purpose, so the push that \
             follows the rebase authenticates, or writes commits, in a way the user never chose: \
             {lost:?}",
        );
        let prompts: Vec<&str> = environment
            .lines()
            .filter(|line| {
                line.split_once('=')
                    .is_some_and(|(key, _)| key == TERMINAL_PROMPT_VAR)
            })
            .collect();
        assert_eq!(
            prompts,
            [format!("{TERMINAL_PROMPT_VAR}=0")],
            "the run sets {TERMINAL_PROMPT_VAR}=0 after the sweep, so that value must win over \
             the one the child holds",
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
        // The value is set after the sweep, so it wins over the value gsw
        // holds. A user who exports the variable again in the rc file still
        // wins, because the rc file loads inside the child after this value was
        // placed.
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
