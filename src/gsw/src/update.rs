//! Bringing the branch up to date with the base, from watch mode.
//!
//! The `R` key rebases the branch onto the base, and the `M` key merges the
//! base into the branch. Neither act is gsw's own. Each key runs one command
//! that the user supplies, in the user's own interactive shell, and that
//! command pushes the branch when it has finished. [`crate::shell`] holds the
//! shell, the type a command name becomes, the probe that asks whether the
//! command exists, and the run that reads what the command writes.
//!
//! What is here is what belongs to these two keys alone: the variable that
//! names each command, the name each key falls back on, the question and its
//! refusals, the line the shell runs, the reads of the work tree before and
//! after the run, and the record of what the run said.
//!
//! **One act here is gsw's own: the abort.** The run has no terminal, so
//! nobody can resolve a conflict inside it. After the command exits, gsw
//! aborts a rebase or a merge that the run started and left stopped, and no
//! other operation. See `run` for the rule.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use shellquote::shell_quote;

use crate::push::{
    confirm_hint, current_branch, Confirmed, PushOutcome, PushPrompt, SuccessReport,
};
use crate::render::{Operation, Snapshot};
use crate::repo::{OperationStart, DETACHED_HEAD};
use crate::shell::{shell_child, start_run, RunEnd, ShellCommand};

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
    /// state a rule once and hold it for both. One list of names makes the
    /// array and a match with no wildcard, so an act that the enum has and the
    /// list does not fails to compile here.
    pub(crate) const ALL: [Self; 2] = {
        macro_rules! every_act {
            ($($act:ident),+) => {
                match Self::Rebase {
                    $(Self::$act)|+ => [$(Self::$act),+],
                }
            };
        }
        every_act!(Rebase, Merge)
    };

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

/// What `R` and `M` say while git holds `operation`.
///
/// **One function for the question and for the run.** The question refuses a
/// snapshot that shows an operation, and the run refuses a work tree where an
/// operation started after the question. The user reads the same words for the
/// same repository, whichever of the two found the operation.
///
/// The verb is the operation that git holds, and not the key that was pressed.
/// The parameter is the operation for that reason: a caller cannot hand this
/// function the act of the key by mistake. See [`BaseUpdate::held`].
fn in_progress_refusal(operation: &Operation) -> String {
    format!(
        "a {} is in progress — finish it first",
        BaseUpdate::held(operation).verb(),
    )
}

/// What became of an operation that git held after the command of a run
/// exited.
///
/// [`stopped_sentence`] takes this value, so the words of every case are in
/// that one function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cleanup {
    /// gsw aborted the operation, and git holds no operation now.
    Aborted,
    /// gsw tried to abort the operation, and git refused. The operation is
    /// still in progress.
    AbortFailed,
    /// gsw cannot show that the run started the operation, so gsw did not try
    /// to abort it. The operation is still in progress. See [`run_in`] for the
    /// conditions.
    LeftAsItIs,
}

/// The last line of a run after which git held `operation`, with the result
/// `cleanup`.
///
/// **The sentence of gsw, and not a line of the command.** The lines of the
/// command above it describe an operation that is still in progress. After an
/// abort, the `⚠` row of that operation is gone, and without this sentence
/// the row describes a work tree that no longer exists. After an abort that
/// git refused, the operation stays, and the sentence says so, in agreement
/// with the `⚠` row of the next frame. It names the act, the count of
/// conflicts, and what gsw did.
///
/// **An operation that gsw did not start gets words of its own, with no
/// count.** gsw did not try to abort it, and gsw cannot show that the command
/// stopped it. So the words say only what gsw knows: git holds the operation,
/// gsw did not start it, and gsw left it as it is.
///
/// The verb is the operation that git held, for the reason
/// [`in_progress_refusal`] gives. The count is the count of the read before
/// the abort, which is the count of the `⚠` row, in the words of that row —
/// see [`crate::render::conflict_words`]. With no conflict, the sentence drops
/// the count, as the `⚠` row does.
fn stopped_sentence(operation: &Operation, cleanup: Cleanup) -> String {
    let verb = BaseUpdate::held(operation).verb();
    let what_gsw_did = match cleanup {
        Cleanup::Aborted => "gsw aborted it",
        Cleanup::AbortFailed => "gsw could not abort it, and it is still in progress",
        Cleanup::LeftAsItIs => {
            return format!("a {verb} is in progress that gsw did not start — left as it is");
        }
    };
    let conflicts = match operation {
        Operation::Rebase { conflicts, .. } | Operation::Merge { conflicts } => *conflicts,
    };
    let on = crate::render::conflict_words(conflicts)
        .map(|words| format!(" on {words}"))
        .unwrap_or_default();
    format!("{verb} stopped{on} — {what_gsw_did}")
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
/// refuse a repository that moved on. It is also what lets [`run`] abort only
/// an operation on the branch that the question named.
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
/// puts the thing the user has to deal with first at the top. That is why the
/// operation that git holds comes first of all: the detached HEAD of a stopped
/// rebase is a result of the rebase, so the words send the user to the rebase.
/// Each one posts a fading line and asks nothing, because none of them is an
/// error: they are the repository saying that this key has nothing to do here.
pub(crate) fn base_update_prompt_for(
    snapshot: &Snapshot,
    update: BaseUpdate,
    command: &ShellCommand,
) -> PushPrompt {
    let branch = snapshot.branch.as_str();
    let base = snapshot.base.as_str();
    let refuse = |message: String| PushPrompt::Refuse { message };

    // git is holding an operation that the user must finish or abort. gsw
    // did not start it, so gsw does neither: the run aborts only an operation
    // that the run started. The `⚠ rebase` row of the header is showing it
    // already.
    //
    // **At the top, above the detached HEAD.** A rebase that stops on a
    // conflict detaches HEAD, and the advice to check out a branch is wrong in
    // the middle of a rebase: the detached HEAD is a result of the operation,
    // and it goes away when the user finishes or aborts that operation.
    if let Some(operation) = &snapshot.operation {
        return refuse(in_progress_refusal(operation));
    }
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
/// reason. A run worked when the command exited 0 and left no operation that
/// git holds. A run that left one is a failure whatever its exit status, and
/// its last line is the sentence of gsw about that operation.
///
/// **The run reads the work tree before the shell starts and after it exits.**
/// The read before refuses a work tree where an operation or a checkout
/// started after the question, and then no shell starts. The read after aborts
/// a rebase or a merge that the run started and left stopped, because the run
/// has no terminal and nobody can resolve a conflict inside it. gsw aborts only
/// an operation that it can show the run started — [`run_in`] states the
/// conditions. Any other operation stays as it is, and the last line says so.
/// An abort that git refuses leaves the operation in progress, and the outcome
/// gives the reason of git above the sentence that says so. Both reads go to
/// `workdir`, which is the work tree of the run.
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

    // **The work tree is read again before the shell starts, and a change
    // starts no shell.** A question describes the repository as it stood when
    // the key was pressed, and the answer arrives whenever the user presses `y`
    // — long enough for another pane to act in between. Two reads cover that
    // gap: the operation that git holds, and then the branch. A third read, of
    // the commit that HEAD holds, refuses nothing, and the read after the
    // shell uses it. The gap between these reads and the shell's own is
    // microseconds rather than seconds, and nothing here closes it entirely,
    // short of a lock git does not offer.
    //
    // **The operation is read first.** A merge or a rebase that started in the
    // gap is one the command would meet and did not start. gsw did not start
    // it either, so gsw does not abort it: it stays as it is, and the words are
    // the words of the question. A rebase must be read before the branch,
    // because a stopped rebase detaches HEAD. The branch check would then read
    // `HEAD` and blame a checkout that never happened.
    //
    // A repository that cannot be read holds no operation here, and the run
    // goes ahead, for the reason the branch check gives below.
    if let Some(operation) = crate::repo::held_operation(workdir) {
        return PushOutcome {
            success: false,
            output: in_progress_refusal(&operation),
        };
    }

    // **The branch is compared next.** A checkout in another pane moves HEAD to
    // a different branch, and `grp` reads HEAD when the shell starts it, so it
    // would rebase a branch the question never named and push it.
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

    // **The commit that HEAD holds is read last, just before the shell
    // starts.** It refuses nothing. The read after the shell compares it with
    // the commit where a held operation started, which is condition 3 of the
    // abort below. A HEAD that cannot be read gives `None`, and gsw then
    // aborts nothing, because it cannot show that the run started an
    // operation.
    //
    // gix reads it, and not a git child as for the branch: for a merge, the
    // value that the read after the shell compares it with is the same read of
    // HEAD through gix. See [`crate::repo::head_commit`].
    let head_before = crate::repo::head_commit(workdir);

    // The child is interactive, it carries no `GIT_` variable out of the
    // environment of gsw but the six a user states on purpose, and it is
    // detached from the terminal — see [`shell_child`], which states all three
    // rules and is the one place they are written.
    let mut child = shell_child(shell, command.script());
    child
        .current_dir(workdir)
        .stdin(Stdio::null())
        // **After the sweep, so it wins.** The sweep keeps the value of this
        // variable that gsw holds, and this call replaces that value with `0`.
        // A user who exports the variable again in the rc file still wins,
        // because the rc file loads inside the child after this value was
        // placed.
        .env(TERMINAL_PROMPT_VAR, "0");

    let run = match start_run(child, scratch) {
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

    // The record and the caller get the same lines in the same order, because
    // both are fed here. A second account of one stream is what puts the
    // verdict of a command in the middle of its output rather than at the end.
    let mut record = Record::new();
    let status = match run.wait(None, &mut |_, line| {
        record.push(&line);
        on_line(line);
    }) {
        Ok(RunEnd::Exited(status)) => status,
        Ok(RunEnd::StillRunning(_)) => {
            unreachable!("a wait with no deadline ends only when the child exits")
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
    };

    // **The work tree is read again after the shell exits.** The run has no
    // terminal, so nobody can resolve a conflict inside it. A rebase or a
    // merge that the run started and left stopped is thus an operation that
    // nobody can finish from here, and gsw aborts it.
    //
    // Only after an exit. A wait that failed says nothing about the child, and
    // a command that still runs can still finish its own operation.
    //
    // **gsw aborts only an operation that it can show the run started.** An
    // abort discards work, and that work can belong to the user. So gsw
    // aborts the operation only when each of these conditions is true:
    //
    // 1. No operation was in progress when gsw read the work tree just before
    //    the shell started. The read at the top of this function refuses the
    //    run otherwise, so this condition is true here. An operation that was
    //    in progress before the run is the work of the user.
    // 2. The operation is on the branch of the question. Condition 1 leaves a
    //    gap between that read and the start of the shell, and in that gap
    //    another pane can check out a different branch and start an operation
    //    there. The command of the user can also check out a different
    //    branch. gsw cannot tell those two cases apart, so it cannot show that
    //    the run started an operation on a branch that the question did not
    //    name.
    // 3. The operation started from the commit that HEAD held just before the
    //    shell started. In the same gap, another pane can also commit on the
    //    branch of the question and start an operation from that commit. The
    //    command can also commit first. gsw cannot tell those two cases apart
    //    either. git records the commit where an operation started, so gsw
    //    compares it with the HEAD that it read.
    //
    // Conditions 2 and 3 close most of the gap of condition 1, and not all of
    // it. An operation that another pane starts in the gap, on the same branch
    // and from the same commit, looks the same as an operation of the run. The
    // branch check at the top of this function has the same gap.
    //
    // A value that cannot be read matches nothing, so gsw then cannot show
    // that the run started the operation. An operation that fails a condition
    // stays as it is, and the last line says so. The outcome is a failure, and
    // the `⚠` row of the next frame shows the operation.
    //
    // The abort goes to `workdir`, which is the work tree of the run. The run
    // got that path by value when the user pressed `y`, and it never reads the
    // worktree on the screen. The key table keeps the arrow keys inert while
    // the run is in flight, but the abort does not depend on that rule.
    //
    // **The sentence of gsw goes last.** The cut to three rows always keeps
    // the last line of a failure. The lines of the command stay above it. The
    // cut keeps the `CONFLICT` line of git, which names the file that
    // conflicted, before any other line of the command.
    //
    // **An abort that git refuses gives its reason above the sentence.** The
    // operation then stays, and the `⚠` row of the next frame shows it. The
    // lines of git go between the lines of the command and the sentence, so
    // the reason is the text just above the sentence, and the sentence says
    // that the operation is still in progress.
    let held = crate::repo::held_operation(workdir);
    if let Some(operation) = &held {
        let start = crate::repo::operation_start(workdir, operation);
        let cleanup = if started_by_the_run(&start, command.branch(), head_before.as_ref()) {
            match abort(workdir, operation) {
                Ok(()) => Cleanup::Aborted,
                Err(reason) => {
                    for line in &reason {
                        record.push(line);
                    }
                    Cleanup::AbortFailed
                }
            }
        } else {
            Cleanup::LeftAsItIs
        };
        record.push(&stopped_sentence(operation, cleanup));
    }

    // **The repository decides the outcome, and not the exit status alone.**
    // A shell function returns the status of its last command, so a command
    // can stop a rebase and then exit 0. The row would then report a rebase
    // and a push that did not occur. A run that left an operation that git
    // holds is a failure, whatever its exit status.
    let success = status.success() && held.is_none();
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

/// Whether the run can show that it started an operation that started at
/// `start`, for a question about `branch` and a HEAD that held `head_before`
/// just before the shell started.
///
/// Conditions 2 and 3 of the rule that [`run_in`] states before its abort:
/// the operation is on the branch of the question, and it started from the
/// commit that HEAD held before the run. Condition 1 is true before this
/// function is called.
///
/// **`None` never matches, on either side.** A branch that cannot be read, a
/// detached HEAD, and a commit that cannot be read are `None` in `start`. A
/// HEAD that cannot be read before the run is `None` in `head_before`. Two
/// values that gsw could not read are not two values that agree, so gsw then
/// cannot show that the run started the operation, and it does not abort it.
fn started_by_the_run(
    start: &OperationStart,
    branch: &str,
    head_before: Option<&gix::ObjectId>,
) -> bool {
    let on_the_branch = start.branch.as_deref() == Some(branch);
    let from_the_head_before = head_before.is_some_and(|head| start.commit.as_ref() == Some(head));
    on_the_branch && from_the_head_before
}

/// The child that aborts `operation`, which git holds in the work tree at
/// `workdir`.
///
/// **The abort follows the operation that git holds, and not the key that
/// started the run.** The command of a key belongs to the user, so `R` can
/// leave a merge stopped, and `git rebase --abort` does not end a merge. The
/// match names the commands of git here, and not [`BaseUpdate::verb`]: those
/// words are the words of gsw, and a change to them must not change what git
/// runs.
///
/// **The rules of every git child of gsw apply.** The child sheds the
/// inherited git environment and keeps the six variables that a user states.
/// A `gsw` that a pre-commit hook started holds `GIT_DIR`, and an abort that
/// obeyed it would abort a rebase in a different repository. The child reads
/// no stdin and has no terminal, because gsw holds the terminal in raw mode.
///
/// **`workdir` is the work tree of the run.** git reads the state of an
/// operation from the git dir of the worktree it runs in. A linked worktree
/// thus gets its own operation aborted, and no other.
///
/// [`abort`] runs the child with [`Command::output`], so what git writes goes
/// to gsw and never to the screen. It reaches the outcome when git refuses the
/// abort.
fn abort_child(workdir: &Path, operation: &Operation) -> Command {
    let subcommand = match operation {
        Operation::Rebase { .. } => "rebase",
        Operation::Merge { .. } => "merge",
    };
    let mut command = Command::new("git");
    gitscratch::shed_inherited_git_environment_keeping_user_intent(&mut command);
    command
        .args([subcommand, "--abort"])
        .current_dir(workdir)
        .stdin(Stdio::null());
    crate::child::detach_from_terminal(&mut command);
    command
}

/// Abort `operation`, which git holds in the work tree at `workdir`.
///
/// # Errors
///
/// Gives the lines that say why the abort failed, when git did not abort the
/// operation. See [`abort_result`].
fn abort(workdir: &Path, operation: &Operation) -> Result<(), Vec<String>> {
    let mut child = abort_child(workdir, operation);
    let attempt = child.output();
    abort_result(&command_line(&child), attempt)
}

/// Whether `attempt`, a run of the abort that `line` names, aborted the
/// operation.
///
/// Separate from [`abort`] so a test can hand it a child that did not start,
/// which no real git of a test can give.
///
/// **Every line that git wrote, stdout first and stderr after it.**
/// [`Command::output`] reads the two streams apart, so the order between them
/// is lost. git writes the reason of a failed abort to stderr and nothing to
/// stdout, so the lost order costs nothing here.
///
/// # Errors
///
/// Gives the lines that say why the abort failed:
///
/// - An abort that cannot start gives one line that says so, in the words of
///   a run that cannot start.
/// - An abort that exits with a status other than 0 gives every line that git
///   wrote. When git wrote no line with text in it, the line is the exit
///   status, for the rule that [`run_in`] states: a failure with nothing to
///   show reads as a failure that did not occur.
fn abort_result(line: &str, attempt: std::io::Result<Output>) -> Result<(), Vec<String>> {
    let output = match attempt {
        Ok(output) => output,
        Err(error) => return Err(vec![format!("cannot run {line}: {error}")]),
    };
    if output.status.success() {
        return Ok(());
    }
    let written: Vec<String> = [&output.stdout, &output.stderr]
        .into_iter()
        .flat_map(|stream| {
            String::from_utf8_lossy(stream)
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    if written.iter().all(|written| written.trim().is_empty()) {
        return Err(vec![format!("{line} failed ({})", output.status)]);
    }
    Err(written)
}

/// The command line that `command` runs, as the words of gsw name it.
///
/// Read from the child itself, so a line that names the abort cannot name a
/// different command from the one that ran.
fn command_line(command: &Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(OsStr::to_string_lossy)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every line a run has written, in arrival order, as one string with a newline
/// between each line and the one before it.
///
/// A run whose command left an operation stopped also gets the sentence of gsw
/// about that operation, after the lines of the command. It goes through
/// [`Record::push`] like every other line, so the one rule for the newlines
/// holds for it too, and an empty record gets no blank line in front of it.
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
        // `grp` to rebase and nothing for `gmp` to merge into. A checkout of a
        // commit or a bisect leaves HEAD here with no operation in progress. A
        // rebase that stopped on a conflict also detaches HEAD, but the refusal
        // that names the rebase wins there — see
        // `a_stopped_rebase_is_named_as_the_rebase_and_not_as_a_detached_head`.
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
    fn a_stopped_rebase_is_named_as_the_rebase_and_not_as_a_detached_head() {
        // A rebase that stops on a conflict detaches HEAD. The advice to check
        // out a branch is wrong in the middle of a rebase: a checkout there
        // leaves the rebase behind and does not finish it. The user must
        // continue or abort the rebase, so both keys send the user to it.
        let stopped = Snapshot {
            branch: DETACHED_HEAD.to_string(),
            operation: Some(Operation::Rebase {
                step: None,
                conflicts: 1,
            }),
            ..behind(5)
        };
        for update in BaseUpdate::ALL {
            assert_eq!(
                refusal(&stopped, update),
                "a rebase is in progress — finish it first",
            );
        }
    }

    #[test]
    fn the_first_refusal_that_applies_wins() {
        // `resolve_base` falls back on the target of `origin/HEAD` and then on
        // HEAD itself, and neither is `main` or `master`. So a checkout of a
        // commit in a repository with no `main` and no `master` matches two
        // rows of the table. HEAD is detached, and the base is not one that
        // these keys act on. The loop takes one base from each fallback.
        //
        // **The detached HEAD wins.** git refuses `HEAD` as the name of a
        // branch, so `grp` has nothing to rebase and `gmp` has nothing to merge
        // into. Every question about the base is a question about what to bring
        // into a branch. So the user must check out a branch before the base
        // means anything, and the higher row says so. After that checkout, the
        // next press of the key gives the refusal of the missing base.
        //
        // The operation that git holds wins over both rows.
        // `a_stopped_rebase_is_named_as_the_rebase_and_not_as_a_detached_head`
        // holds the pair of the operation and the detached HEAD.
        for base in ["origin/trunk", "HEAD"] {
            let detached_with_no_base = Snapshot {
                branch: DETACHED_HEAD.to_string(),
                base: base.to_string(),
                ..behind(5)
            };
            assert_eq!(
                refusal(&detached_with_no_base, BaseUpdate::Rebase),
                "HEAD is detached — check out a branch to rebase",
            );
            assert_eq!(
                refusal(&detached_with_no_base, BaseUpdate::Merge),
                "HEAD is detached — check out a branch to merge",
            );
        }
    }
}

#[cfg(test)]
mod sentence_tests {
    use super::*;

    /// A rebase that stopped on `conflicts` conflicts.
    fn rebase(conflicts: u32) -> Operation {
        Operation::Rebase {
            step: None,
            conflicts,
        }
    }

    #[test]
    fn one_conflict_takes_the_singular() {
        // "1 conflicts" reads as a defect in the tool, right beside the
        // number it is about. The `⚠` row says "1 conflict" for the same
        // work tree.
        assert_eq!(
            stopped_sentence(&rebase(1), Cleanup::Aborted),
            "rebase stopped on 1 conflict — gsw aborted it",
        );
    }

    #[test]
    fn more_conflicts_than_one_take_the_plural() {
        assert_eq!(
            stopped_sentence(&rebase(2), Cleanup::Aborted),
            "rebase stopped on 2 conflicts — gsw aborted it",
        );
    }

    #[test]
    fn no_conflict_drops_the_count() {
        // A rebase also stops on an `edit` step or on a failed `exec` step,
        // with no conflict at all. "on 0 conflicts" describes a stop that
        // did not occur, and the `⚠` row drops the clause for the same work
        // tree.
        assert_eq!(
            stopped_sentence(&rebase(0), Cleanup::Aborted),
            "rebase stopped — gsw aborted it",
        );
    }

    #[test]
    fn a_merge_is_named_as_the_merge() {
        // The verb is the operation that git held, as in the refusal.
        assert_eq!(
            stopped_sentence(&Operation::Merge { conflicts: 1 }, Cleanup::Aborted),
            "merge stopped on 1 conflict — gsw aborted it",
        );
    }

    #[test]
    fn an_abort_that_failed_says_that_the_operation_is_still_in_progress() {
        // The `⚠` row of the next frame shows the operation, so the sentence
        // must agree with it. The count follows the same rule as after an
        // abort that worked.
        assert_eq!(
            stopped_sentence(&rebase(1), Cleanup::AbortFailed),
            "rebase stopped on 1 conflict — gsw could not abort it, and it is still in progress",
        );
        assert_eq!(
            stopped_sentence(&rebase(0), Cleanup::AbortFailed),
            "rebase stopped — gsw could not abort it, and it is still in progress",
        );
        assert_eq!(
            stopped_sentence(&Operation::Merge { conflicts: 2 }, Cleanup::AbortFailed),
            "merge stopped on 2 conflicts — gsw could not abort it, and it is still in progress",
        );
    }
}

#[cfg(test)]
mod started_tests {
    use super::*;

    /// The branch of every question here.
    const BRANCH: &str = "issue-12";

    /// A commit id. The rule compares ids and reads no repository, so any
    /// full id serves.
    fn head() -> gix::ObjectId {
        gix::ObjectId::from_hex(b"1111111111111111111111111111111111111111").expect("a full id")
    }

    #[test]
    fn two_values_that_gsw_could_not_read_do_not_agree() {
        // `None == None` is true in Rust. A comparison of the two options
        // would thus abort an operation whose start commit gsw cannot read,
        // after a run whose HEAD gsw could not read either. gsw can show
        // nothing in that case, so it must abort nothing.
        //
        // **The armed control comes first.** A start on the branch and the
        // HEAD of the question matches, so the assertions below are not
        // measured against a rule that never matches.
        //
        // This guard is not red-first: the rule came with the comparison. A
        // mutation proved it: `start.commit.as_ref() == head_before` fails
        // this test.
        let head = head();
        let read = OperationStart {
            branch: Some(BRANCH.to_string()),
            commit: Some(head),
        };
        assert!(
            started_by_the_run(&read, BRANCH, Some(&head)),
            "a start on the branch and the HEAD of the question must match",
        );

        let unread = OperationStart {
            branch: Some(BRANCH.to_string()),
            commit: None,
        };
        assert!(
            !started_by_the_run(&unread, BRANCH, None),
            "two commits that gsw could not read must not match",
        );
        assert!(
            !started_by_the_run(&unread, BRANCH, Some(&head)),
            "a start commit that gsw could not read must not match",
        );
        assert!(
            !started_by_the_run(&read, BRANCH, None),
            "a HEAD that gsw could not read before the run must not match",
        );
        assert!(
            !started_by_the_run(
                &OperationStart {
                    branch: None,
                    commit: Some(head),
                },
                BRANCH,
                Some(&head),
            ),
            "a branch that gsw could not read must not match",
        );
    }
}

#[cfg(all(test, unix))]
mod abort_tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    /// The command line that every abort here names.
    const REBASE_ABORT: &str = "git rebase --abort";

    /// What a child gives that exited with `code` and wrote `stdout` and
    /// `stderr`.
    fn exited(code: i32, stdout: &str, stderr: &str) -> std::io::Result<Output> {
        Ok(Output {
            // A wait status holds the exit code in its second byte.
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    #[test]
    fn the_abort_child_is_named_by_the_command_it_runs() {
        // The line that names a failed abort reads the child itself, so it
        // names the abort of the operation that git held.
        let dir = std::env::temp_dir();
        let rebase = Operation::Rebase {
            step: None,
            conflicts: 1,
        };
        let merge = Operation::Merge { conflicts: 1 };
        assert_eq!(command_line(&abort_child(&dir, &rebase)), REBASE_ABORT);
        assert_eq!(
            command_line(&abort_child(&dir, &merge)),
            "git merge --abort"
        );
    }

    #[test]
    fn an_abort_that_exits_0_worked() {
        assert_eq!(abort_result(REBASE_ABORT, exited(0, "", "")), Ok(()));
    }

    #[test]
    fn an_abort_that_git_refuses_gives_every_line_of_git_stdout_first() {
        // The lines go to the outcome as git wrote them, blank lines too. The
        // row drops the blank lines, and the text keeps them.
        assert_eq!(
            abort_result(
                REBASE_ABORT,
                exited(
                    128,
                    "out\n",
                    "error: Unable to create 'index.lock'\n\nfatal: could not move back\n"
                ),
            ),
            Err(vec![
                "out".to_string(),
                "error: Unable to create 'index.lock'".to_string(),
                String::new(),
                "fatal: could not move back".to_string(),
            ]),
        );
    }

    #[test]
    fn an_abort_that_fails_and_says_nothing_gives_its_exit_status() {
        // A failure with nothing to show reads as a failure that did not
        // occur. The exit status is all that git left.
        assert_eq!(
            abort_result(REBASE_ABORT, exited(1, "", " \n")),
            Err(vec![
                "git rebase --abort failed (exit status: 1)".to_string()
            ]),
        );
    }

    #[test]
    fn an_abort_that_cannot_start_says_so() {
        // git is not on the path of gsw, although the shell of the user found
        // it for the command. The reader of the operation is gsw's own, so it
        // still found the operation that the command left stopped.
        assert_eq!(
            abort_result(
                REBASE_ABORT,
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "No such file or directory"
                )),
            ),
            Err(vec![
                "cannot run git rebase --abort: No such file or directory".to_string()
            ]),
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
        a_child_of_this_test_passes, a_child_of_this_test_passes_with, entries_of, kill_now,
        shed_git_lines, test_name, test_process_can_open_the_terminal, user_intent_lost,
        user_intent_value, StubShell, CHILD_RAN, GAVE_UP_WITHIN, HOSTILE_GIT_ENVIRONMENT,
        HOSTILE_MARKER, TTY_OPENED, TTY_REFUSED,
    };
    use crate::testrepo::{
        git, git_allowing_failure, git_output, git_stdout, init_repo, init_repo_with_worktree,
    };
    use std::os::unix::fs::PermissionsExt;
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
        confirmed_act(BaseUpdate::Merge)
    }

    /// A confirmed `update` of [`BRANCH`] against [`BASE`], running the
    /// default command of that act.
    fn confirmed_act(update: BaseUpdate) -> BaseUpdateCommand {
        BaseUpdateCommand::new(
            update,
            BRANCH,
            BASE,
            ShellCommand::new(None, update.default_command()).expect("a name"),
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

    /// Give the repository of `checkout` a real conflict between [`BASE`] and
    /// [`BRANCH`], and leave [`BRANCH`] checked out in `checkout`.
    ///
    /// Line 1 of `a.txt` changes in one way on the base and in a different way
    /// on the branch, each in a commit of its own. `git rebase main` and `git
    /// merge main` then both stop with `CONFLICT (content): Merge conflict in
    /// a.txt`, which is the state a user who pressed `y` finds.
    ///
    /// `checkout` is the main worktree of an [`init_repo`] repository, or a
    /// linked worktree of a [`crate::testrepo::init_repo_with_worktree`]
    /// repository. The commit on the base goes through the main worktree of
    /// the repository, where [`BASE`] is checked out, because git refuses a
    /// checkout of [`BASE`] in a second worktree. The branch starts from the
    /// commit that `checkout` holds before the call, so the two commits share
    /// one parent.
    ///
    /// # Panics
    ///
    /// Panics where the main worktree does not have [`BASE`] checked out, or
    /// where git refuses a step. A fixture that did not build its conflict
    /// makes every later assertion measure something else.
    fn conflicting_branches(checkout: &Path) {
        let fork_point = git_stdout(checkout, &["rev-parse", "HEAD"]);
        let common_dir = PathBuf::from(git_stdout(
            checkout,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ));
        let main_worktree = common_dir
            .parent()
            .expect("the git dir of the main worktree is inside that worktree");
        assert_eq!(
            git_stdout(main_worktree, &["symbolic-ref", "--short", "HEAD"]),
            BASE,
            "the main worktree must have the base checked out, or the base gets no commit",
        );

        std::fs::write(main_worktree.join("a.txt"), "base\n").expect("write a.txt on the base");
        git(
            main_worktree,
            &["commit", "-q", "-am", "change a.txt on the base"],
        );

        git(checkout, &["checkout", "-q", "-b", BRANCH, &fork_point]);
        std::fs::write(checkout.join("a.txt"), "branch\n").expect("write a.txt on the branch");
        git(
            checkout,
            &["commit", "-q", "-am", "change a.txt on the branch"],
        );
    }

    /// Whether git holds a merge in the work tree at `dir`.
    ///
    /// git is the oracle here, and not the reader of gsw: a test of the reader
    /// that asks the reader proves nothing. `MERGE_HEAD` is a ref of the
    /// worktree, so a linked worktree answers about its own merge.
    fn merge_in_progress(dir: &Path) -> bool {
        git_output(dir, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])
            .status
            .success()
    }

    /// Whether git holds a rebase in the work tree at `dir`.
    ///
    /// git is the oracle, as for [`merge_in_progress`]. git keeps a rebase in
    /// `rebase-merge/` or in `rebase-apply/` of the git dir of the worktree,
    /// and `--git-path` names the directory in that git dir, so a linked
    /// worktree answers about its own rebase.
    fn rebase_in_progress(dir: &Path) -> bool {
        ["rebase-merge", "rebase-apply"].into_iter().any(|name| {
            PathBuf::from(git_stdout(
                dir,
                &["rev-parse", "--path-format=absolute", "--git-path", name],
            ))
            .is_dir()
        })
    }

    /// The branch that the work tree at `dir` has checked out, and the commit
    /// that HEAD holds, as git reports them.
    ///
    /// A detached HEAD gives [`DETACHED_HEAD`] as the branch, so a test that
    /// compares the pair also sees a rebase that left HEAD detached.
    fn checkout_of(dir: &Path) -> (String, String) {
        let branch = git_output(dir, &["symbolic-ref", "-q", "--short", "HEAD"]);
        let branch = if branch.status.success() {
            String::from_utf8_lossy(&branch.stdout).trim().to_string()
        } else {
            DETACHED_HEAD.to_string()
        };
        (branch, git_stdout(dir, &["rev-parse", "HEAD"]))
    }

    /// The line of a stub shell that runs real git with `arguments`.
    ///
    /// The run sheds every `GIT_` variable except the six that a user states,
    /// and `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` are two of those six.
    /// The line pins both to `/dev/null`, so the global configuration of the
    /// developer does not decide what the test reads.
    fn real_git(arguments: &str) -> String {
        format!("GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null git {arguments}")
    }

    /// A [`crate::RenderConfig`] for a walk of a fixture. The walk reads git,
    /// and the settings of the frame do not change what it reads.
    fn walk_config() -> crate::RenderConfig {
        crate::RenderConfig {
            base: None,
            max_files: None,
            bar_width: 20,
            log_lines: 0,
            truecolor: false,
            width_offset: 0,
            refresh_interval: None,
        }
    }

    /// A pane wider than every line here and taller than every overlay, so a
    /// test about the words of the row is not also a test about clipping.
    const ROOMY_PANE: crate::watch::Dimensions = crate::watch::Dimensions {
        width: 200,
        height: 20,
    };

    /// The outcome of a run of `update` through `stub` in the work tree at
    /// `dir`, and the rows that the row under the frame then shows.
    ///
    /// The chain that a user drives, from end to end. A walk of the work tree
    /// gives the snapshot, the key asks its question, `y` confirms it, the run
    /// acts on the work tree, and the outcome goes to the row. The rows are the
    /// glyphs that a user reads: the escapes are forced on and then taken out
    /// again, as the tests of the row do it.
    ///
    /// # Panics
    ///
    /// Panics where the key refuses the work tree. A test that reads the rows
    /// of a run needs a run.
    fn run_through_the_row(
        stub: &StubShell,
        update: BaseUpdate,
        dir: &Path,
    ) -> (PushOutcome, String) {
        let walked = gix::open(dir).expect("open the work tree");
        let snapshot =
            crate::collect_snapshot(&walked, &walk_config()).expect("walk the work tree");
        let command = ShellCommand::new(None, update.default_command()).expect("a name");
        let now = Instant::now();
        let mut ui = crate::push::PushUi::new(false);
        ui.request_base_update(&snapshot, update, &command, ROOMY_PANE, now);
        let Some(Confirmed::BaseUpdate(confirmed)) = ui.confirm(now) else {
            panic!(
                "the {} key must ask about this work tree, and not refuse it",
                update.key(),
            );
        };
        let outcome = run_quiet(stub.as_shell(), &confirmed, dir);
        ui.finished(outcome.clone(), now);
        let rows = testcolor::strip_ansi(&testcolor::with_forced_ansi(|| {
            ui.overlay(ROOMY_PANE, now).text()
        }));
        (outcome, rows)
    }

    /// The last line of `output`, and every line above it.
    fn last_line_of(output: &str) -> (&str, &str) {
        output.rsplit_once('\n').unwrap_or(("", output))
    }

    /// What git writes when a rebase or a merge of the conflict fixture stops.
    const CONFLICT_LINE: &str = "CONFLICT (content): Merge conflict in a.txt";

    /// The sentence of gsw under a rebase that stopped on one conflict, which
    /// gsw then aborted.
    const REBASE_ABORTED: &str = "rebase stopped on 1 conflict — gsw aborted it";

    /// The sentence of gsw under a rebase that stopped on one conflict, which
    /// gsw then could not abort.
    const REBASE_NOT_ABORTED: &str =
        "rebase stopped on 1 conflict — gsw could not abort it, and it is still in progress";

    /// The sentence of gsw under a rebase that the run did not start, which gsw
    /// left as it is.
    const REBASE_LEFT: &str = "a rebase is in progress that gsw did not start — left as it is";

    /// The sentence of gsw under a merge that the run did not start, which gsw
    /// left as it is.
    const MERGE_LEFT: &str = "a merge is in progress that gsw did not start — left as it is";

    /// The variable that names the directory of the recording git to the
    /// child that finds it first on its `PATH`.
    ///
    /// The name carries no `GIT_` prefix, so no sweep of gsw takes it away.
    const RECORDING_GIT_VAR: &str = "GSW_RECORDING_GIT";

    /// One call of the recording git.
    struct RecordedCall {
        /// Each argument of the call, in order.
        arguments: Vec<String>,
        /// The directory of the call, with every symbolic link in it resolved.
        cwd: PathBuf,
        /// The environment of the call, one `NAME=value` line for each
        /// variable.
        environment: String,
        /// [`TTY_OPENED`] where the call could open the controlling terminal,
        /// and [`TTY_REFUSED`] where it could not.
        terminal: String,
    }

    /// Run `test` in a child of this test binary whose `PATH` finds a
    /// recording git first, and fail where that child fails.
    ///
    /// **The recording git is how a test reads the environment of a git child
    /// of gsw itself.** gsw starts git by name, so the first `git` on the
    /// `PATH` is the process that gsw starts. That program writes down its
    /// arguments, its directory and its environment, and it tries to open the
    /// controlling terminal, as [`StubShell::probing_the_terminal`] does. Then
    /// it puts back the `PATH` of this process and gives the call to the real
    /// git, so the run does all of its real work.
    ///
    /// A read of the removals off the [`Command`] proves less. It reads the
    /// command that a function builds, and not the process that the run
    /// starts. A change that [`abort`] makes to the command after
    /// [`abort_child`] built it does not show in that read, and neither does a
    /// second builder that the run uses in place of [`abort_child`]. A git
    /// hook proves less too. git adds variables of its own to the environment
    /// of a hook, so a record from a hook cannot use the rule of the `GIT_`
    /// prefix.
    ///
    /// **The `PATH` goes on the child, and never on this process**, for the
    /// reason [`a_child_of_this_test_passes_with`] gives. This process writes
    /// the program into a temporary directory of its own, and holds that
    /// directory until the child ends.
    ///
    /// # Panics
    ///
    /// Panics where the program cannot be written, and where
    /// [`a_child_of_this_test_passes_with`] panics.
    fn a_child_with_a_recording_git_passes(test: &str) {
        let dir = tempfile::tempdir().expect("tempdir");
        let own_path = std::env::var_os("PATH").unwrap_or_default();
        let program = dir.path().join("git");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\n\
                 call=$(mktemp -d {dir}/call.XXXXXX) || exit 1\n\
                 printf '%s\\n' \"$@\" > \"$call/arguments\"\n\
                 pwd -P > \"$call/cwd\"\n\
                 env > \"$call/environment\"\n\
                 if ( exec 3<>/dev/tty ) 2>/dev/null; then\n\
                 \tprintf '{TTY_OPENED}' > \"$call/terminal\"\n\
                 else\n\
                 \tprintf '{TTY_REFUSED}' > \"$call/terminal\"\n\
                 fi\n\
                 PATH={own_path}\n\
                 export PATH\n\
                 exec git \"$@\"\n",
                dir = shell_quote(&dir.path().display().to_string()),
                own_path = shell_quote(&own_path.to_string_lossy()),
            ),
        )
        .expect("write the recording git");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("make the recording git executable");
        let path = std::env::join_paths(
            std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(&own_path)),
        )
        .expect("a PATH that holds the recording git");
        a_child_of_this_test_passes_with(
            test,
            &[
                ("PATH", path.as_os_str()),
                (RECORDING_GIT_VAR, dir.path().as_os_str()),
            ],
        );
    }

    /// Each call that the recording git of this child recorded, in no order.
    ///
    /// # Panics
    ///
    /// Panics where [`a_child_with_a_recording_git_passes`] did not start this
    /// process, and where a record cannot be read.
    fn recorded_git_calls() -> Vec<RecordedCall> {
        let dir = PathBuf::from(
            std::env::var_os(RECORDING_GIT_VAR).expect("the parent must name the recording git"),
        );
        let read = |call: &Path, name: &str| {
            std::fs::read_to_string(call.join(name)).expect("read a record of the call")
        };
        std::fs::read_dir(&dir)
            .expect("read the records of the recording git")
            .map(|entry| entry.expect("an entry of the records").path())
            .filter(|path| path.is_dir())
            .map(|call| RecordedCall {
                arguments: read(&call, "arguments")
                    .lines()
                    .map(str::to_string)
                    .collect(),
                cwd: PathBuf::from(read(&call, "cwd").trim_end_matches('\n')),
                environment: read(&call, "environment"),
                terminal: read(&call, "terminal"),
            })
            .collect()
    }

    /// The calls of the recording git of this child that abort a rebase.
    ///
    /// The arguments to look for are the arguments of [`abort_child`], read
    /// from the child itself, so the record and the abort cannot name two
    /// different commands.
    fn recorded_rebase_aborts() -> Vec<RecordedCall> {
        let rebase = Operation::Rebase {
            step: None,
            conflicts: 1,
        };
        let arguments: Vec<String> = abort_child(Path::new("."), &rebase)
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        recorded_git_calls()
            .into_iter()
            .filter(|call| call.arguments == arguments)
            .collect()
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
    fn a_merge_that_started_after_the_confirmation_refuses_the_run_and_stays() {
        // The window the question opens, for an operation instead of a
        // checkout. `M` reads the snapshot while nothing is in progress, `y`
        // arrives seconds later, and a `git merge` in another pane stops on a
        // conflict in between. A merge keeps HEAD on the branch, so the branch
        // is the branch of the question, and `gmp` would meet a merge that it
        // did not start. Nothing may run in that case.
        //
        // gsw did not start that merge either, so the merge stays as it is:
        // the user started it, and only the user decides to finish or abort it.
        let stub = StubShell::answering(0);
        let workdir = init_repo();
        conflicting_branches(workdir.path());
        git_allowing_failure(workdir.path(), &["merge", "-q", BASE]);
        assert!(
            merge_in_progress(workdir.path()),
            "the fixture must hold a real merge, or the run has nothing to refuse",
        );

        let outcome = run_quiet(stub.as_shell(), &confirmed_merge(), workdir.path());

        assert!(
            !outcome.success,
            "a run over a merge in progress must not report success: {:?}",
            outcome.output,
        );
        assert_eq!(
            outcome.output, "a merge is in progress — finish it first",
            "the run must refuse with the words of the question",
        );
        assert_eq!(stub.runs(), "", "a refused run must start no shell at all");
        assert!(
            merge_in_progress(workdir.path()),
            "gsw did not start the merge, so the merge must still be in progress",
        );
    }

    #[test]
    fn a_rebase_that_started_after_the_confirmation_is_named_and_not_blamed_on_a_checkout() {
        // The same window, for a rebase. A `git rebase` in another pane stops
        // on a conflict between the question and the `y`, and a stopped rebase
        // detaches HEAD. The branch then reads as `HEAD`, and the words of the
        // branch check would blame a checkout that never happened. The user
        // must finish or abort the rebase, so the words name the rebase.
        //
        // gsw did not start that rebase, so the rebase stays as it is.
        let stub = StubShell::answering(0);
        let workdir = init_repo();
        conflicting_branches(workdir.path());
        git_allowing_failure(workdir.path(), &["rebase", BASE]);
        assert!(
            rebase_in_progress(workdir.path()),
            "the fixture must hold a real stopped rebase, or the run has nothing to refuse",
        );

        let outcome = run_quiet(stub.as_shell(), &default_command(), workdir.path());

        assert!(
            !outcome.success,
            "a run over a rebase in progress must not report success: {:?}",
            outcome.output,
        );
        assert_eq!(
            outcome.output, "a rebase is in progress — finish it first",
            "the run must name the rebase, and not a change of branch",
        );
        assert_eq!(stub.runs(), "", "a refused run must start no shell at all");
        assert!(
            rebase_in_progress(workdir.path()),
            "gsw did not start the rebase, so the rebase must still be in progress",
        );
    }

    #[test]
    fn a_rebase_that_the_run_left_stopped_is_aborted_in_the_linked_worktree_of_the_run() {
        // The run has no terminal, so nobody can resolve a conflict inside it.
        // A rebase that the command started and left stopped is thus a rebase
        // that gsw aborts, and the branch goes back to where it was.
        //
        // **The run is in a linked worktree, and this process is somewhere
        // else.** The current directory of this process is the directory of
        // the crate, and git keeps the rebase of a linked worktree in the git
        // dir of that worktree, not in the `.git` dir of the repository. An
        // abort that goes to the wrong directory, or to the wrong git dir,
        // leaves the rebase in place, and the checks below see it.
        let (repo, linked) = init_repo_with_worktree();
        conflicting_branches(&linked);
        let before = checkout_of(&linked);
        assert_eq!(
            before.0, BRANCH,
            "the linked worktree must have the branch of the question checked out",
        );
        let main_before = checkout_of(repo.path());
        let stub = StubShell::new(&real_git(&format!("rebase {BASE}")));

        let outcome = run_quiet(stub.as_shell(), &default_command(), &linked);

        assert!(
            !rebase_in_progress(&linked),
            "the run started the rebase and left it stopped, so gsw must abort it: {:?}",
            outcome.output,
        );
        assert_eq!(
            crate::repo::held_operation(&linked),
            None,
            "the reader of gsw must see no operation in the work tree of the run",
        );
        assert_eq!(
            checkout_of(&linked),
            before,
            "the branch must be checked out again, at its commit from before the run",
        );
        assert_eq!(
            git_stdout(&linked, &["status", "--porcelain"]),
            "",
            "the abort must leave the work tree clean",
        );
        assert!(
            !outcome.success,
            "a run that stopped on a conflict must not report success: {:?}",
            outcome.output,
        );

        // The watch loop walks the repository after every outcome of a run,
        // and this walk is the same walk. An operation on the snapshot is the
        // `⚠` row of the next frame.
        let walked = gix::open(&linked).expect("open the linked worktree");
        let snapshot = crate::collect_snapshot(&walked, &walk_config()).expect("walk the worktree");
        assert_eq!(
            snapshot.operation, None,
            "the next frame must show no ⚠ row for a rebase that gsw aborted",
        );

        // The main worktree holds no rebase, and it did not move.
        assert!(
            !rebase_in_progress(repo.path()),
            "the main worktree must hold no rebase",
        );
        assert_eq!(
            checkout_of(repo.path()),
            main_before,
            "the main worktree must stay on its branch, at its commit",
        );
    }

    #[test]
    fn a_merge_that_the_run_left_stopped_is_aborted_whichever_key_started_the_run() {
        // The same rule for a merge. A merge that stops on a conflict keeps
        // HEAD on the branch, so the branch and its commit do not show it. The
        // merge itself and the conflicted file do.
        //
        // **The abort follows the operation that git holds, and not the key.**
        // The command of `R` belongs to the user and can merge, and `git
        // rebase --abort` does not end a merge. So both keys run a command that
        // stops a merge here, and both runs must end with no merge.
        let stub = StubShell::new(&real_git(&format!("merge {BASE}")));
        for update in BaseUpdate::ALL {
            let key = update.key();
            let workdir = init_repo();
            conflicting_branches(workdir.path());
            let before = checkout_of(workdir.path());

            let outcome = run_quiet(stub.as_shell(), &confirmed_act(update), workdir.path());

            assert!(
                !merge_in_progress(workdir.path()),
                "the {key} run started the merge and left it stopped, so gsw must abort it: {:?}",
                outcome.output,
            );
            assert_eq!(
                crate::repo::held_operation(workdir.path()),
                None,
                "the reader of gsw must see no operation after the {key} run",
            );
            assert_eq!(
                checkout_of(workdir.path()),
                before,
                "the branch must stay checked out at its commit from before the {key} run",
            );
            assert_eq!(
                git_stdout(workdir.path(), &["status", "--porcelain"]),
                "",
                "the abort must leave the work tree of the {key} run clean",
            );
            assert!(
                !outcome.success,
                "a {key} run that stopped on a conflict must not report success: {:?}",
                outcome.output,
            );
        }
    }

    #[test]
    fn a_rebase_that_gsw_aborted_is_named_in_the_last_line_and_in_the_last_row() {
        // The command wrote why it stopped, and then gsw aborted the rebase.
        // The words of the command alone describe a rebase that is still in
        // progress, and the `⚠` row of that rebase is gone. So gsw says what
        // it did, in a sentence of its own, as the last line.
        //
        // **Last, so that it survives the cut to three rows.** The cut always
        // keeps the last line of a failure. git names the file that conflicted
        // in its `CONFLICT` line, and then writes more lines of its own. The
        // cut keeps that line before them, so the rows that the user reads
        // still name the file, and not only the text of the outcome.
        let workdir = init_repo();
        conflicting_branches(workdir.path());
        let stub = StubShell::new(&real_git(&format!("rebase {BASE}")));

        let (outcome, rows) = run_through_the_row(&stub, BaseUpdate::Rebase, workdir.path());

        assert!(
            !rebase_in_progress(workdir.path()),
            "the fixture must leave a rebase that gsw aborted: {:?}",
            outcome.output,
        );
        let (above, last) = last_line_of(&outcome.output);
        assert_eq!(
            last, REBASE_ABORTED,
            "the sentence of gsw must be the last line of the outcome: {:?}",
            outcome.output,
        );
        assert!(
            above.lines().any(|line| line == CONFLICT_LINE),
            "the line of git that names the file must stay above the sentence: {:?}",
            outcome.output,
        );
        assert_eq!(
            rows.lines().last(),
            Some(REBASE_ABORTED),
            "the sentence must be the last row, so that the cut to three rows keeps it: {rows:?}",
        );
        assert!(
            rows.lines().any(|line| line == CONFLICT_LINE),
            "the line of git that names the file must reach the rows that the user reads, \
             and not only the text of the outcome: {rows:?} from {:?}",
            outcome.output,
        );
    }

    #[test]
    fn a_command_that_stops_a_rebase_and_exits_zero_is_aborted_and_fails() {
        // A command can stop a rebase and then exit 0. A shell function
        // returns the status of its last command, and a `grp` that ends with
        // an `echo` or a `true` exits 0 after any rebase. The row would then
        // say `Rebased issue-12 onto main with grp`, and nothing was rebased
        // and nothing was pushed.
        //
        // So the repository after the run decides the outcome, and not the
        // exit status alone.
        let workdir = init_repo();
        conflicting_branches(workdir.path());
        let stub = StubShell::new(&format!("{}; true", real_git(&format!("rebase {BASE}"))));

        let (outcome, rows) = run_through_the_row(&stub, BaseUpdate::Rebase, workdir.path());

        assert!(
            !rebase_in_progress(workdir.path()),
            "a rebase that the run left stopped must be aborted whatever the exit status: {:?}",
            outcome.output,
        );
        assert!(
            !outcome.success,
            "a run that gsw had to abort must not report success: {:?}",
            outcome.output,
        );
        assert_eq!(
            last_line_of(&outcome.output).1,
            REBASE_ABORTED,
            "the last line must say that gsw aborted the rebase: {:?}",
            outcome.output,
        );
        assert!(
            !rows.contains("Rebased issue-12 onto main with grp"),
            "the row must not say that the rebase worked: {rows:?}",
        );
        assert_eq!(
            rows.lines().last(),
            Some(REBASE_ABORTED),
            "the row must end with the sentence of the abort: {rows:?}",
        );
    }

    #[test]
    fn an_abort_that_git_refuses_gives_the_reason_of_git_and_says_the_rebase_stays() {
        // git refuses an abort that cannot take the lock of the index. A git
        // process holds that lock while it works, and a git that crashed
        // leaves it behind. The rebase then stays, and the `⚠` row of the
        // next frame shows it. A sentence that said "aborted" would disagree
        // with that row and send the user away from a rebase that is still
        // there.
        //
        // The reason is the reason of git, so the lines of git go above the
        // sentence of gsw.
        //
        // The tail takes the lock after the rebase stopped. The rebase is thus
        // a real one, and only the abort meets the lock.
        let workdir = init_repo();
        conflicting_branches(workdir.path());
        let stub = StubShell::new(&format!(
            "{}; touch \"$({})\"",
            real_git(&format!("rebase {BASE}")),
            real_git("rev-parse --git-path index.lock"),
        ));

        let (outcome, rows) = run_through_the_row(&stub, BaseUpdate::Rebase, workdir.path());

        let still_in_progress = rebase_in_progress(workdir.path());
        let lock = PathBuf::from(git_stdout(
            workdir.path(),
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "index.lock",
            ],
        ));
        let lock_was_taken = lock.is_file();
        // The lock goes before the assertions, so a failed assertion leaves no
        // lock for a later read of this work tree to meet.
        let _ = std::fs::remove_file(&lock);
        assert!(
            lock_was_taken,
            "the fixture must take the lock, or the abort had nothing to meet",
        );
        assert!(
            still_in_progress,
            "git refused the abort, so the rebase must still be in progress: {:?}",
            outcome.output,
        );
        assert!(
            !outcome.success,
            "a run that left a rebase in progress must not report success: {:?}",
            outcome.output,
        );
        let (above, last) = last_line_of(&outcome.output);
        assert_eq!(
            last, REBASE_NOT_ABORTED,
            "the last line must say that gsw could not abort the rebase and that it stays: {:?}",
            outcome.output,
        );
        assert!(
            above
                .lines()
                .any(|line| line.contains("index.lock") && line.contains("File exists")),
            "the reason of git must stay above the sentence: {:?}",
            outcome.output,
        );
        assert_eq!(
            rows.lines().last(),
            Some(REBASE_NOT_ABORTED),
            "the row must end with the sentence of the failed abort: {rows:?}",
        );
    }

    #[test]
    fn an_operation_that_the_run_left_on_a_different_branch_is_left_as_it_is() {
        // **gsw aborts only an operation that it can show the run started.**
        // The question named one branch, and the command of the user can check
        // out a different one. A rebase or a merge that stops there is on a
        // branch that the question did not name. It can be work of the user
        // that the command only continued, so gsw does not abort it, and the
        // last line says so. The outcome is a failure, because an operation is
        // still in progress.
        //
        // **Only the branch differs.** `other` holds the commit that the branch
        // of the question holds. Each operation here thus starts from the
        // commit that HEAD held before the run, and the branch is the one
        // condition that tells it apart from an operation that the run
        // started. A detached HEAD is no branch, so it never matches the branch
        // of the question.
        //
        // A merge keeps HEAD on its branch, and git writes the branch of a
        // rebase in `head-name`, or `detached HEAD`. So each act is here with
        // each kind of checkout.
        for (update, checkout) in [
            (BaseUpdate::Merge, "other"),
            (BaseUpdate::Rebase, "other"),
            (BaseUpdate::Merge, "--detach"),
            (BaseUpdate::Rebase, "--detach"),
        ] {
            let (act, in_progress, sentence): (&str, fn(&Path) -> bool, &str) = match update {
                BaseUpdate::Rebase => ("rebase", rebase_in_progress, REBASE_LEFT),
                BaseUpdate::Merge => ("merge", merge_in_progress, MERGE_LEFT),
            };
            let case = format!("{act} after checkout {checkout}");
            let workdir = init_repo();
            conflicting_branches(workdir.path());
            git(workdir.path(), &["branch", "other"]);
            let stub = StubShell::new(&format!(
                "{} && {}",
                real_git(&format!("checkout -q {checkout}")),
                real_git(&format!("{act} {BASE}")),
            ));

            let (outcome, rows) = run_through_the_row(&stub, update, workdir.path());

            assert!(
                in_progress(workdir.path()),
                "gsw cannot show that the run started this {case}, so it must still be in \
                 progress: {:?}",
                outcome.output,
            );
            assert!(
                !outcome.success,
                "a run that left an operation in progress must not report success ({case}): {:?}",
                outcome.output,
            );
            assert_eq!(
                last_line_of(&outcome.output).1,
                sentence,
                "the last line must say that gsw did not start the operation and left it \
                 ({case}): {:?}",
                outcome.output,
            );
            assert_eq!(
                rows.lines().last(),
                Some(sentence),
                "the row must end with the sentence of gsw ({case}): {rows:?}",
            );
        }
    }

    #[test]
    fn an_operation_that_started_from_a_different_commit_is_left_as_it_is() {
        // **gsw aborts only an operation that it can show the run started.**
        // The branch is the branch of the question here, but HEAD moved
        // before the operation started. Another pane can commit on the branch
        // between the read before the shell and the start of the shell, and
        // then start an operation from that commit. The command can also
        // commit first. gsw cannot tell those two cases apart, so an
        // operation that did not start from the commit that HEAD held before
        // the run stays as it is.
        //
        // **Only the commit differs.** Each operation is on the branch of the
        // question, so the branch does not tell it apart from an operation
        // that the run started. git writes the commit where a rebase started
        // in `orig-head`, and a stopped merge does not move HEAD. So each act
        // is here.
        for update in BaseUpdate::ALL {
            let (act, in_progress, sentence): (&str, fn(&Path) -> bool, &str) = match update {
                BaseUpdate::Rebase => ("rebase", rebase_in_progress, REBASE_LEFT),
                BaseUpdate::Merge => ("merge", merge_in_progress, MERGE_LEFT),
            };
            let workdir = init_repo();
            conflicting_branches(workdir.path());
            let stub = StubShell::new(&format!(
                "{} && {}",
                real_git("commit -q --allow-empty -m extra"),
                real_git(&format!("{act} {BASE}")),
            ));

            let (outcome, rows) = run_through_the_row(&stub, update, workdir.path());

            assert!(
                in_progress(workdir.path()),
                "the {act} did not start from the HEAD of before the run, so gsw cannot show that \
                 the run started it, and it must still be in progress: {:?}",
                outcome.output,
            );
            assert!(
                !outcome.success,
                "a run that left a {act} in progress must not report success: {:?}",
                outcome.output,
            );
            assert_eq!(
                last_line_of(&outcome.output).1,
                sentence,
                "the last line must say that gsw did not start the {act} and left it: {:?}",
                outcome.output,
            );
            assert_eq!(
                rows.lines().last(),
                Some(sentence),
                "the row must end with the sentence of gsw ({act}): {rows:?}",
            );
        }
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

    #[test]
    fn the_abort_child_sheds_the_git_variables_of_gsw_and_keeps_those_of_the_user() {
        // **This test starts this test binary again, and the hostile
        // environment goes on that child**, for the reason that the test of
        // the run child gives.
        //
        // The abort moves a branch and resets a work tree, so a leaked
        // variable does damage here too. A `gsw` that a pre-commit hook
        // started holds `GIT_DIR`, and an abort that obeyed it would abort a
        // rebase in the repository of the hook. The abort also acts for the
        // user, so it keeps the six variables that the user states.
        //
        // **Two halves, because each half sees what the other half cannot.**
        //
        // - The run: a real run whose command stops a real rebase, in a child
        //   that holds `GIT_DIR=/gsw-decoy/.git`. An abort that obeyed that
        //   variable finds no repository and fails, and the rebase stays. The
        //   run cannot show the six variables of the user, because git aborts
        //   a rebase without them too.
        // - The record: the child finds a recording git first on its `PATH`,
        //   so the abort child itself writes down the environment it got. The
        //   record shows each variable that the abort child held.
        //
        // **The armed control comes first.** The child asserts that it really
        // holds each hostile variable and each variable of the user, that no
        // path of the hostile variables is on this machine, and that the git
        // it starts by name is the recording git. An assertion that a
        // variable is absent passes just as readily where there was nothing
        // to remove.
        if std::env::var_os(HOSTILE_MARKER).is_none() {
            a_child_with_a_recording_git_passes(&test_name(
                module_path!(),
                "the_abort_child_sheds_the_git_variables_of_gsw_and_keeps_those_of_the_user",
            ));
            return;
        }

        for (name, _) in HOSTILE_GIT_ENVIRONMENT {
            assert!(
                std::env::var_os(name).is_some(),
                "the child must really hold {name}, or there is nothing here to remove and the \
                 assertions below are measured against nothing",
            );
        }
        for name in gitscratch::USER_INTENT_GIT_ENVIRONMENT {
            assert_eq!(
                std::env::var(name).ok(),
                Some(user_intent_value(name)),
                "the child must really hold {name}, or there is nothing here to keep",
            );
        }
        let nothing_at = |path: &Path| {
            matches!(
                std::fs::symlink_metadata(path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound
            )
        };
        let decoys: Vec<&Path> = HOSTILE_GIT_ENVIRONMENT
            .iter()
            .map(|(_, value)| Path::new(*value))
            .filter(|path| path.is_absolute())
            .collect();
        for decoy in &decoys {
            assert!(
                nothing_at(decoy),
                "{} must name nothing on this machine, or a write there cannot be seen",
                decoy.display(),
            );
        }

        let workdir = init_repo();
        conflicting_branches(workdir.path());
        assert!(
            !recorded_git_calls().is_empty(),
            "the fixture runs git by name, so the recording git must have recorded it. Without \
             that, the record of the abort below is empty for a reason that is not the abort",
        );
        let before = checkout_of(workdir.path());
        let stub = StubShell::new(&real_git(&format!("rebase {BASE}")));

        let outcome = run_quiet(stub.as_shell(), &default_command(), workdir.path());

        assert!(
            !rebase_in_progress(workdir.path()),
            "the abort did not end the rebase in the work tree of the run. An abort that obeyed \
             GIT_DIR goes to a repository that does not exist, and fails: {:?}",
            outcome.output,
        );
        assert_eq!(
            checkout_of(workdir.path()),
            before,
            "the abort must check the branch out again, at its commit from before the run",
        );
        assert_eq!(
            last_line_of(&outcome.output).1,
            REBASE_ABORTED,
            "gsw must say that it aborted the rebase: {:?}",
            outcome.output,
        );
        for decoy in &decoys {
            assert!(
                nothing_at(decoy),
                "a git child of gsw obeyed a hostile variable and wrote to {}",
                decoy.display(),
            );
        }

        let aborts = recorded_rebase_aborts();
        let calls: Vec<String> = recorded_git_calls()
            .iter()
            .map(|call| call.arguments.join(" "))
            .collect();
        assert_eq!(
            aborts.len(),
            1,
            "the run must abort the rebase once, through the git that the PATH finds: {calls:?}",
        );
        let abort = &aborts[0];
        assert_eq!(
            abort.cwd,
            resolved(workdir.path()),
            "the abort must run in the work tree of the run",
        );
        let carried = shed_git_lines(&abort.environment);
        assert!(
            carried.is_empty(),
            "the abort child carried a git variable out of the environment of gsw. Each of these \
             aims the abort, or configures it, somewhere the user never pointed it: {carried:?}",
        );
        let lost = user_intent_lost(&abort.environment, None);
        assert!(
            lost.is_empty(),
            "the abort child lost a git variable that the user states on purpose, so git aborts \
             with a configuration that the user did not choose: {lost:?}",
        );

        println!("{CHILD_RAN}");
    }

    #[test]
    fn the_abort_child_cannot_open_the_controlling_terminal() {
        // gsw holds the terminal in raw mode while the abort runs, as it does
        // while the command of the user runs. git asks nothing on the terminal
        // for an abort today. But git starts hooks and helpers, and each of
        // them gets the terminal of the abort child. A program that opens
        // `/dev/tty` paints over the frame of gsw and reads the keys that the
        // event thread of gsw waits for. So the abort child gets no terminal,
        // as no child of gsw gets one.
        //
        // **The recording git is the probe.** git itself cannot be a probe.
        // The recording git is the process that gsw starts, so it tries
        // `/dev/tty` before it gives the call to the real git.
        //
        // **The armed control comes first, and it has two parts.** The test
        // process must hold a terminal, or `/dev/tty` is unopenable for every
        // process and the refusal below holds for no reason. The git calls of
        // the fixture must open it, because the fixture does not detach them.
        // That shows that the probe sees a terminal where there is one.
        //
        // The hostile environment is on this child too, because the helper
        // that puts the recording git on a child also puts it there. It
        // changes nothing here: the test above shows that the abort works
        // under it.
        if std::env::var_os(HOSTILE_MARKER).is_none() {
            if !test_process_can_open_the_terminal() {
                eprintln!(
                    "skipped: this test process has no controlling terminal, so /dev/tty is \
                     unopenable for every child regardless - the assertion would hold vacuously",
                );
                return;
            }
            a_child_with_a_recording_git_passes(&test_name(
                module_path!(),
                "the_abort_child_cannot_open_the_controlling_terminal",
            ));
            return;
        }

        assert!(
            test_process_can_open_the_terminal(),
            "the child must hold the terminal of this test, or the refusal below holds for every \
             process",
        );
        let workdir = init_repo();
        conflicting_branches(workdir.path());
        let fixture: Vec<String> = recorded_git_calls()
            .into_iter()
            .map(|call| call.terminal)
            .collect();
        assert!(
            !fixture.is_empty() && fixture.iter().all(|terminal| terminal == TTY_OPENED),
            "the fixture does not detach its git, so each of its calls must open the terminal. \
             Otherwise the probe sees no terminal anywhere: {fixture:?}",
        );
        let stub = StubShell::new(&real_git(&format!("rebase {BASE}")));

        let outcome = run_quiet(stub.as_shell(), &default_command(), workdir.path());

        assert!(
            !rebase_in_progress(workdir.path()),
            "the run must abort the rebase, or there is no abort child to ask: {:?}",
            outcome.output,
        );
        let aborts: Vec<String> = recorded_rebase_aborts()
            .into_iter()
            .map(|call| call.terminal)
            .collect();
        assert_eq!(
            aborts,
            [TTY_REFUSED],
            "the abort child keeps the controlling terminal, so a hook or a helper that git \
             starts can paint over the frame of gsw and take the keys gsw is reading",
        );

        println!("{CHILD_RAN}");
    }
}
