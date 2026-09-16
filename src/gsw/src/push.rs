//! Pushing the current branch from watch mode: what a push will do, how it is
//! described to the user before it runs, and how it is run.
//!
//! `gix` cannot push, so the push itself is a `git` child process. Everything
//! that *decides* — which remote, which arguments, what the confirmation says,
//! and what an outcome means — lives here as pure, terminal-free code so it can
//! be tested without a network or a pty. Only [`run_push`], the blocking half of
//! [`spawn`], starts a process: the push itself, and the read of HEAD that
//! checks the repository is still on the branch the confirmation named.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use colored::{ColoredString, Colorize};

use crate::age::{format_age_detailed, scale_rgb};
use crate::child::detach_from_terminal;
use crate::lines::LineSplitter;
use crate::render::{Snapshot, UpstreamStatus};
use crate::repo::DETACHED_HEAD;
use crate::shell::ShellCommand;
use crate::update::{base_update_prompt_for, BaseUpdate, BaseUpdateCommand};
use crate::watch::{Dimensions, InputMode};
use crate::worktrees::WorktreeList;
use textfit::truncate_right;

/// Most rows a status message is allowed to occupy under the frame.
///
/// A rejected push can produce a dozen lines of hints, and the frame below is
/// what the user is actually watching. Three rows is enough for git's
/// `To <remote>` / `! [rejected] …` / `error: failed to push …` triple, which
/// is the part that says what went wrong — and enough, when a pre-push hook
/// failed instead, for the line that failed and git's verdict on it.
///
/// A ceiling, not a promise: a pane with fewer than four rows cannot spare
/// three and still show a frame, so [`PushUi::overlay`] clips the message
/// further. This is the most the user will ever see, not the least.
const MAX_STATUS_ROWS: usize = 3;

/// Most rows of a running push's own output the window under the frame will
/// ever show.
///
/// A pre-push hook that builds and tests a workspace prints hundreds of lines,
/// and the six that matter are the six that just arrived. Six is also small
/// enough that the frame — the thing watch mode is for — keeps most of a short
/// pane while a push runs.
///
/// A ceiling, not a promise: [`PushUi::overlay`] shows fewer in a pane that
/// cannot spare six, and drops the oldest rather than the newest when it does.
const MAX_PUSH_OUTPUT_ROWS: usize = 6;

/// Most messages from another feature the row holds while a push or a question
/// owns it.
///
/// `G` acts while a push runs, and a push with a pre-push hook takes minutes.
/// Each `G` in those minutes can refuse, and each refusal costs the user one
/// key to clear it. A queue with no bound thus turns one long push into a row
/// the user must press through. Four covers the times a user reaches for the
/// key during a single push, and four is small enough that the clearance is
/// not a job of its own.
///
/// **A full queue drops the newest message and keeps the oldest.** That is the
/// opposite of what [`failure_lines`] does, and the difference is deliberate.
/// There the lines are the output of one command, and git's verdict comes
/// last. Here the messages are separate runs of the same command: the first
/// refusal tells the user what went wrong, and each refusal after it is
/// usually that same refusal again.
const MAX_HELD_MESSAGES: usize = 4;

/// Git's prefix for advice lines. They follow the real error and explain
/// general remedies, so they are the first thing to drop when the message has
/// to fit in [`MAX_STATUS_ROWS`]. A lexical prefix is the right matcher here:
/// this is git's own output convention, not a syntactic property of anything.
const HINT_PREFIX: &str = "hint:";

/// What pressing `p` will actually do, resolved from the snapshot *before* the
/// confirmation appears — so the prompt describes the command that will run
/// rather than a guess the user has to check afterwards.
///
/// Two variants say a push will run — an existing remote branch moves, or a
/// branch appears on the remote that was not there before. That split is the
/// reason this is an enum rather than a struct with a `create: bool`: creating
/// a remote branch is a different act from updating one, and it gets different
/// wording and a different command.
///
/// The other three say a push will not run, and each carries *why*. Totality is
/// deliberate: an `Option` would hand the caller a bare `None` and force it to
/// re-derive the reason it was refused, which is the one piece of knowledge this
/// module exists to own. Every plan can state its own case, so
/// [`prompt_for`] — the only way in — always has something to say.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PushPlan {
    /// HEAD is detached, so there is no branch to push.
    Detached,
    /// No remote gsw can push an untracked branch to.
    NoRemote,
    /// The upstream already carries every local commit, so a push would send
    /// nothing. Kept as a plan rather than folded into `None` so the caller can
    /// say *why* it is not prompting.
    UpToDate {
        /// Short upstream name, e.g. `origin/gsw-push`.
        target: String,
    },
    /// The branch tracks a remote branch that exists and is behind HEAD. Runs a
    /// bare `git push`, which follows the configured upstream — so gsw never
    /// has to re-derive a remote and a refspec git already knows.
    Update {
        /// Short upstream name, e.g. `origin/gsw-push`.
        target: String,
        /// Commits HEAD has that the upstream does not.
        commits: u32,
    },
    /// The branch has no upstream, so the push creates the branch on the remote
    /// and records it as the upstream (`-u`). This is the case the confirmation
    /// must call out: it puts a branch on a shared remote that nobody has seen.
    Create {
        /// Remote to create the branch on.
        remote: String,
        /// Local branch name, which is also the remote branch name.
        branch: String,
    },
}

/// A confirmed push: the branch the confirmation named, and the `git`
/// arguments that carry it out.
///
/// The two are one value because they are one sentence — push *this branch*,
/// *this way* — and the arguments alone do not say which branch. An
/// [`PushPlan::Update`] runs a bare `git push`, which git resolves against
/// whatever HEAD points at when the child process starts. That is not
/// necessarily what HEAD pointed at when the question went on screen: the
/// answer arrives whenever the user presses `y`, and a checkout in another pane
/// fits in between. Carrying the branch alongside the arguments is what lets
/// [`run_push`] confirm the repository is still on that branch, microseconds
/// before git reads it.
///
/// Built only by [`prompt_for`], so a command nobody confirmed cannot be
/// assembled somewhere else and handed to the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PushCommand {
    /// The branch the confirmation named, as [`crate::repo::branch_name`]
    /// reports it.
    branch: String,
    /// Arguments to pass to `git`, not including the program name.
    args: Vec<String>,
}

impl PushCommand {
    /// The command that pushes `branch` by running `git <args>`.
    fn new(branch: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            branch: branch.into(),
            args,
        }
    }

    /// The branch the confirmation named.
    pub(crate) fn branch(&self) -> &str {
        &self.branch
    }

    /// The arguments to pass to `git`, not including the program name.
    pub(crate) fn args(&self) -> &[String] {
        &self.args
    }
}

/// What a confirmation runs: a push of gsw's own, or a command of the user's
/// that brings the branch up to date with the base.
///
/// One value, because one row asks every question of watch mode and one key
/// answers each of them. `y` means "run what the question described", and the
/// question is the only thing that knows what that is — so the answer carries
/// the work itself rather than a name the loop would have to look the work up
/// by. A second value beside this one would be a second thing for `y` to
/// consult, and the two could disagree about which question is on the screen.
///
/// Each variant is built only by the function that composes its own question,
/// so a command nobody confirmed cannot be assembled somewhere else and handed
/// to a runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Confirmed {
    /// A push, as [`prompt_for`] planned it.
    Push(PushCommand),
    /// A rebase onto the base or a merge of the base, as
    /// [`crate::update::base_update_prompt_for`] planned it. The command is the
    /// user's own, and it pushes the branch itself once it has finished.
    BaseUpdate(BaseUpdateCommand),
}

/// What the watch loop does when the user presses a key that runs something.
///
/// This is the whole interface [`prompt_for`] hands back, and
/// [`crate::update::base_update_prompt_for`] hands back the same thing for the
/// keys that rebase onto the base or merge it in. It is deliberately two cases
/// rather than one for each plan: the caller asks a question or shows a
/// message, and never learns which plan produced either. A [`PushPlan`] variant
/// added later — a rejected force push, a protected branch — changes the
/// wording here without touching the loop that displays it.
///
/// [`PushPrompt::Confirm`] carries the command with the question, so the
/// arguments cannot be requested for a command that must never run. The
/// invariant is structural: there is no way to hold a `Confirm` without holding
/// the exact [`Confirmed`] the question described — for a push, the argument
/// list *and* the branch it was written for, which is what the runner re-checks
/// before it pushes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PushPrompt {
    /// Ask before running the command.
    Confirm {
        /// The question, without the key hint, which is the value below.
        question: String,
        /// The keys that answer the question, and what each one does, as
        /// [`confirm_hint`] spells them. It rides with the question because
        /// three keys ask one now, and each of them binds Enter to an act of
        /// its own — so the hint is a fact about the question rather than a
        /// convention the row can hold on its own.
        hint: String,
        /// Whether this question deserves the color of one the user must read
        /// twice.
        ///
        /// Two acts earn it, and for the same reason. A push that creates a
        /// remote branch puts something on a shared remote that nobody has
        /// seen, and a rebase rewrites every commit of the branch and then
        /// force-pushes the result. Neither can be allowed to look like the
        /// routine act beside it, and the color is what carries that in the
        /// half second before the words are read.
        caution: bool,
        /// The command this question described, and what `y` runs.
        command: Confirmed,
        /// What the row says while that command runs, without its age.
        ///
        /// Composed here, with the question, because the sentence names the
        /// act that was confirmed: a press of `R` reports a rebase, and only
        /// the question knows it was one. [`PushUi::overlay`] puts the age
        /// after it.
        running_notice: String,
        /// What to show once this command succeeds. Composed here, with the
        /// question, so the two sentences describe the same act — a push
        /// confirmed as a create reports itself as a create.
        success_message: String,
    },
    /// Run nothing and show this instead. Not an error: the common cause is a
    /// branch that is already fully pushed.
    Refuse {
        /// Why no push is going to happen.
        message: String,
    },
}

/// Decide what pressing `p` does, given the branch state gsw already renders.
///
/// The one way into this module. Callers pass what the snapshot holds and get
/// back either a question with its command or a message — never a plan they
/// have to interpret, and never an argument list they could run against the
/// wrong question.
///
/// `upstream` is the snapshot's tracking status, which is `Some` only when the
/// upstream is configured *and* its remote-tracking ref resolves. That is
/// exactly the signal the wording needs: a branch whose remote ref is missing
/// gets the create wording, because a push really will create it.
pub(crate) fn prompt_for(
    branch: &str,
    remote: Option<&str>,
    upstream: Option<&UpstreamStatus>,
) -> PushPrompt {
    // One match, so a plan's wording and the command that carries it out are
    // written side by side. The variants that never push return here, which is
    // why no later step has to describe a push it can never be asked for.
    match PushPlan::resolve(branch, remote, upstream) {
        // Named as the act it is. A branch that nobody on the remote has seen
        // appearing there is not the same event as an existing branch moving
        // forward, and the sentence has to be the thing that says so — the user
        // reads it in the half second before pressing `y`.
        PushPlan::Create { remote, branch } => {
            let question = format!("Create new remote branch {remote}/{branch}?");
            let success_message = format!("Created {remote}/{branch}");
            PushPrompt::Confirm {
                question,
                hint: confirm_hint(PUSH_VERB),
                caution: true,
                // `-u` records the new remote branch as the upstream, so the
                // push after this one is a plain update.
                command: Confirmed::Push(PushCommand::new(
                    branch.clone(),
                    vec!["push".to_string(), "-u".to_string(), remote, branch],
                )),
                running_notice: RUNNING_NOTICE.to_string(),
                success_message,
            }
        }
        PushPlan::Update { target, commits } => {
            let unit = if commits == 1 { "commit" } else { "commits" };
            PushPrompt::Confirm {
                question: format!("Push {commits} {unit} to {target}?"),
                hint: confirm_hint(PUSH_VERB),
                caution: false,
                // Bare `push`: git reads the remote and the refspec out of the
                // branch config, so a branch tracking something other than the
                // repository's default remote still goes to the right place.
                // Which branch's config it reads is decided by HEAD at exec
                // time, which is why the command carries the branch this
                // question was written for.
                command: Confirmed::Push(PushCommand::new(branch, vec!["push".to_string()])),
                running_notice: RUNNING_NOTICE.to_string(),
                success_message: format!("Pushed {commits} {unit} to {target}"),
            }
        }
        PushPlan::UpToDate { target } => PushPrompt::Refuse {
            message: format!("{target} is already up to date"),
        },
        PushPlan::Detached => PushPrompt::Refuse {
            message: format!("{DETACHED_HEAD} is detached — check out a branch to push"),
        },
        PushPlan::NoRemote => PushPrompt::Refuse {
            message: "no remote to push to".to_string(),
        },
    }
}

/// Run `git push` on a thread of its own and hand the outcome to `on_finish`.
///
/// Off the render thread on purpose. A push is a network round trip, and the
/// watch loop is what keeps the refresh countdown moving, the ages advancing,
/// and a resize repainting. Blocking it for the seconds a push takes would
/// freeze the monitor at exactly the moment the user is watching it.
///
/// `on_finish` runs on that thread. The one production caller sends the outcome
/// down the loop's own channel, so it re-enters the loop the same way every
/// other event does — no shared state, and the outcome is applied between
/// frames rather than during one.
///
/// Takes the whole [`PushCommand`] by value, so the branch the confirmation
/// named crosses onto the thread with the arguments and [`run_push`] can still
/// refuse a repository that moved on in the meantime.
pub(crate) fn spawn<L, F>(command: PushCommand, workdir: PathBuf, on_line: L, on_finish: F)
where
    L: Fn(String) + Send + Sync + 'static,
    F: FnOnce(PushOutcome) + Send + 'static,
{
    std::thread::spawn(move || on_finish(run_push(&command, &workdir, &on_line)));
}

/// What a refused push tells the user to do. Pressing `p` again re-resolves the
/// plan against the branch that is checked out now, so the next question
/// describes the repository as it actually stands — which is the whole remedy.
const RETRY_ADVICE: &str = "press p again";

/// Run `git push` to completion and describe how it went.
///
/// The blocking half of [`spawn`], separated so it can be tested against a real
/// repository without a thread or a channel in the way.
///
/// **The branch is checked first.** A confirmation describes the repository as
/// it stood when `p` was pressed, and the answer arrives whenever the user
/// presses `y` — long enough for a checkout in another pane to land in between.
/// So the branch [`PushCommand`] carries is compared against the one checked out
/// *now*, and a mismatch refuses the push instead of running it against a
/// repository the question never described. The gap between that read and git's
/// own is microseconds rather than seconds; nothing here can close it entirely,
/// short of a lock git does not offer.
///
/// Three things are forced on the child, and all of them matter because gsw is
/// holding the alternate screen in raw mode:
///
/// - **The child is detached from the terminal** ([`detach_from_terminal`]) —
///   its own session on Unix, no inherited console on Windows — so the terminal
///   device cannot be opened by it or by anything it runs. This is the part
///   that actually holds the guarantee, because a terminal prompt usually comes
///   from a *descendant*: OpenSSH opens the terminal directly for a passphrase
///   or an unknown host key (`/dev/tty` through `read_passphrase()`, `CONIN$`
///   on Windows), so a closed stdin and a captured stderr never reach it, and
///   it has no read timeout — the push would hang behind a question gsw never
///   drew while the two processes split the user's keystrokes. With no terminal
///   to open, ssh falls back to `SSH_ASKPASS`, and with no GUI askpass to run
///   it fails immediately and says so. The same is true of every other
///   descendant, credential helpers included, which is why this is done to the
///   process rather than to one transport.
/// - **stdin is closed** and **`GIT_TERMINAL_PROMPT=0`**, which is git's own
///   half of the same rule: git asks for HTTP usernames and passwords itself,
///   and this refuses those before the detachment has to. Disabled, git fails
///   immediately and says why, which lands in the status rows like any other
///   error. Credential helpers and a GUI `SSH_ASKPASS` are untouched — they do
///   not need the terminal, and only prompting *at the terminal* is refused.
/// - **Both streams are captured**, which also suppresses git's progress meter:
///   it renders only to a terminal, so a pipe removes the carriage-return
///   redraws that would otherwise arrive as unreadable status rows.
fn run_push(
    command: &PushCommand,
    workdir: &Path,
    on_line: &(dyn Fn(String) + Sync),
) -> PushOutcome {
    // `None` means git could not be run at all, which the push below reports in
    // git's own terms. Refusing here instead would blame a branch change that
    // did not happen — and a git that cannot start cannot push either.
    if let Some(current) = current_branch(workdir) {
        if current != command.branch() {
            return PushOutcome {
                success: false,
                output: format!(
                    "branch changed from {} to {current} since the confirmation — {RETRY_ADVICE}",
                    command.branch(),
                ),
            };
        }
    }

    let mut child = Command::new("git");
    child
        .args(command.args())
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0");
    detach_from_terminal(&mut child);

    let mut child = match child.spawn() {
        Ok(child) => child,
        // git is missing, or not executable. Rare, and worth saying plainly:
        // every other failure here is git's own words, and this one would
        // otherwise arrive as an empty message.
        Err(error) => {
            return PushOutcome {
                success: false,
                output: format!("cannot run git: {error}"),
            }
        }
    };

    // Taken out of the handle so each pipe is owned by the thread that drains
    // it, which leaves `child` free to be waited on below.
    let child_stdout = child.stdout.take();
    let child_stderr = child.stderr.take();

    // Every line both pipes carried, in the order it was read, as one string
    // with a newline between each line and the one before it.
    //
    // **Arrival order, not stream order.** Grouping the text by stream buries
    // whatever the quieter pipe said last in the middle of the louder pipe's
    // output, and on a failed push that is exactly git's `error: failed to
    // push some refs` — written on stderr after a hook has filled stdout. The
    // user's own terminal merges the two in write order, and this is the same
    // account of the same push.
    //
    // **One growing string, not one `String` per line.** A pre-push hook that
    // builds and tests a workspace prints hundreds of thousands of lines, and
    // a vector of them is a heap allocation each — then one more copy of the
    // whole push to join them at the end, for a record that [`failure_lines`]
    // reads three lines of and a successful push reads none of. Appending in
    // place costs the amortized growth of a single buffer instead, and the
    // text it holds is what `join("\n")` produced, byte for byte: the
    // separator goes *between* lines, so there is no trailing newline and a
    // push that said nothing leaves the empty string behind. Bounding the
    // record is the other way to spend less, and it is [`PushUi`]'s decision
    // to make rather than this function's — see [`PushOutcome`].
    //
    // The flag beside the text is what the text alone cannot say. "The buffer
    // is still empty" is not the same question as "no line has landed yet": a
    // line can *be* empty — an `echo ""` in a hook keeps its row, by
    // [`LineSplitter`]'s rule — and asking the buffer would swallow the
    // newline that belongs after such a first line.
    let collected = std::sync::Mutex::new((String::new(), false));
    let record = |line: String| {
        {
            // The guard lives and dies inside this block, so it is released
            // before the callback below runs. A reader therefore holds the lock
            // for an append onto a string and never across the caller's work.
            let mut collected = collected
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (text, any_line_recorded) = &mut *collected;
            if *any_line_recorded {
                text.push('\n');
            }
            *any_line_recorded = true;
            text.push_str(&line);
        }
        on_line(line);
    };

    // One thread per pipe, and both must run at once. A pipe holds a fixed
    // number of bytes, so a reader that waits its turn lets the other pipe
    // fill, and a child blocked writing into a full pipe never exits — which
    // is the deadlock a single-threaded read of two streams always eventually
    // finds. Scoped threads because `record` is borrowed, not owned: it is
    // `Sync`, so both threads can call it, and the scope is what proves to the
    // compiler that neither outlives the borrow. The scope joins both readers
    // before it returns, which is what makes the wait below safe.
    std::thread::scope(|scope| {
        scope.spawn(|| drain(child_stderr, &record));
        scope.spawn(|| drain(child_stdout, &record));
    });

    // Waited on only after both pipes reach end of file, which they do when
    // the child closes them at exit. Reversing the two would be the same
    // deadlock by another door on a child that outputs more than a pipe holds.
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            return PushOutcome {
                success: false,
                output: format!("cannot wait for git: {error}"),
            }
        }
    };

    // Both readers are joined, so the lock has nothing left to protect and the
    // text is taken out of it whole rather than copied out of it.
    let (mut text, _) = collected
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let success = status.success();
    if !success && text.trim().is_empty() {
        // A failure with nothing to show would render as a blank row, which
        // reads as success. The exit status is all git left us.
        text = format!("git push failed ({status})");
    }

    PushOutcome {
        success,
        output: text,
    }
}

/// Read one of the child's pipes to the end, reporting each line as it lands.
///
/// Every line goes to `report` and nowhere else, so the caller's record of the
/// stream and the window's view of it are the same sequence in the same order.
/// A drain that also returned its own text would be a second account of one
/// pipe, and joining two such accounts is what put git's verdict in the middle
/// of a hook's output rather than at the end of the push.
///
/// `report` **must not panic.** It is called on this thread, inside a
/// [`std::thread::scope`], and a scope whose thread panicked panics in turn
/// when it ends — carrying the panic out of [`run_push`], off the push thread,
/// and past the `on_finish` that tells the monitor a push is over. The one
/// production caller sends on a channel and ignores the result, which cannot
/// panic.
///
/// `stream` is an `Option` because [`std::process::Child`]'s handles are, and a
/// missing pipe is treated as an empty one: it cannot happen for a child
/// configured with [`Stdio::piped`], and inventing an error message for it
/// would put words in git's mouth.
///
/// A read error ends the drain with what was read so far. The child is on the
/// other end of a pipe that is about to close anyway, and the exit status —
/// which is what decides success — is read from the child itself.
fn drain(stream: Option<impl std::io::Read>, report: &(dyn Fn(String) + Sync)) {
    let Some(mut stream) = stream else {
        return;
    };

    let mut splitter = LineSplitter::new();
    let mut buffer = [0_u8; 8192];

    loop {
        match stream.read(&mut buffer) {
            // End of file: the child closed this pipe.
            Ok(0) => break,
            Ok(read) => {
                for line in splitter.feed(&buffer[..read]) {
                    report(line);
                }
            }
            // A signal arrived mid-read. Nothing was lost and nothing is wrong.
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }

    // A hook that exits without a trailing newline still said something.
    if let Some(line) = splitter.finish() {
        report(line);
    }
}

/// The branch checked out in `workdir` right now, or [`DETACHED_HEAD`] when
/// there is none. `None` only when `git` could not be run at all.
///
/// Asked of `git` rather than of `gix`, for two reasons. It is the same
/// question, put to the same program, from the same working directory, a
/// moment before that program resolves HEAD for the push itself — so the answer
/// cannot disagree with git's for a reason gsw would have to model, the way a
/// second implementation of HEAD resolution eventually would. And it keeps the
/// runner taking nothing but a [`PushCommand`] and a path: a `gix::Repository`
/// is not `Send`, so a handle for this could not simply be carried onto the
/// push thread, and the tests here would have to build one instead of pointing
/// at a directory.
///
/// A detached HEAD reports as [`DETACHED_HEAD`], matching
/// [`crate::repo::branch_name`] and the header gsw draws. git refuses `HEAD` as
/// a branch name, so it can never equal a branch a confirmation named — a
/// detached checkout always reads as a change.
///
/// **The child sheds the inherited git environment.** git obeys the environment
/// before it obeys the directory it was pointed at, so a `gsw` started from
/// inside a hook holds a `GIT_DIR` that answers this question about the
/// repository being committed to. That answer is some other branch, or no
/// branch at all, and every confirmation would then be refused as a checkout
/// that never happened. The rule is the `GIT_` prefix and never a list of
/// names, and the six names of [`gitscratch::USER_INTENT_GIT_ENVIRONMENT`] stay
/// because a person sets each of those on purpose — this runs for that person.
pub(crate) fn current_branch(workdir: &Path) -> Option<String> {
    let mut command = Command::new("git");
    gitscratch::shed_inherited_git_environment_keeping_user_intent(&mut command);
    let output = command
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .current_dir(workdir)
        .stdin(Stdio::null())
        .output()
        .ok()?;

    // Detached HEAD is a non-zero exit with nothing on stdout; both spellings
    // of "no branch here" collapse to the sentinel.
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || name.is_empty() {
        return Some(DETACHED_HEAD.to_string());
    }
    Some(name)
}

/// How a finished `git push` came out.
///
/// `output` is everything the child said on both pipes, in the order it was
/// read, kept whole: choosing which of it to show is [`PushUi`]'s job, and a
/// runner that pre-digested it would decide the wording from a place with no
/// idea how many rows are free.
///
/// The order is load-bearing. [`failure_lines`] shows the last lines, and on a
/// failed pre-push hook the last line is git's verdict on stderr — which comes
/// after a hook that wrote to stdout, and only after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PushOutcome {
    /// Whether `git push` exited zero.
    pub success: bool,
    /// Everything git wrote, both streams, in the order they were captured.
    pub output: String,
}

/// Everything watch mode puts under the frame, and the input mode that goes
/// with it.
///
/// The name says `Push` because the push is what owns the row and what every
/// state below describes: a question, a push in flight, and the outcome of
/// one. It is **not** the push's alone. The `G` key runs a command of the
/// user's own, and a command that refuses says why — so
/// [`PushUi::post_error`], [`PushUi::post_notice`] and
/// [`PushUi::post_progress`] are the doors another feature posts through, and
/// they are what keeps the features from painting over each other.
///
/// Watch mode holds one of these and asks it two questions — what mode are we
/// in, and what does the pane show. It never learns whether a prompt or an
/// error is up, so the states below can grow without the render loop growing a
/// branch for each one.
///
/// The list of the worktrees lives here too, and it is not the push's either.
/// It takes the pane, so it owns the row as a question does, and
/// [`PushUi::mode`] stays the one source of what the keys mean. The loop asks
/// one more question for it, [`PushUi::list`], because the open list replaces
/// the frame and does not go under it.
pub(crate) struct PushUi {
    state: State,
    /// Messages from another feature that arrived while the push, a question,
    /// or the open list owned the row, oldest first, each one waiting for the
    /// row to be free.
    ///
    /// A push is the one thing here that takes minutes, and its own outcome is
    /// what the user is waiting to read. So a message that arrives mid-push is
    /// held rather than posted, and [`PushUi::overlay`] posts the oldest one on
    /// the first frame that finds nothing else on the row. Dropping it instead
    /// would make a failure silent, and silence belongs to one case only: a
    /// command that does not exist.
    ///
    /// **A queue rather than one slot, because minutes hold more than one
    /// message.** `G` acts during a push, and the run it starts frees the key
    /// again the moment it ends, so a second `G` refuses with the first
    /// refusal still waiting. One slot made the second message overwrite the
    /// first, which is the silence the rule above forbids. Each message here
    /// reaches the user in turn, and each gets the row to itself for a life of
    /// its own — a key for an error, the clock for a notice.
    ///
    /// The queue holds [`MAX_HELD_MESSAGES`] messages. A full one drops the
    /// newest and keeps the oldest — see that constant for why that end.
    held: VecDeque<HeldMessage>,
    /// Whether the terminal takes 24-bit color, as [`crate::RenderConfig`]
    /// resolved it from the CLI flags and `COLORTERM`. Carried here because the
    /// status message fades, and a fade
    /// is a gradient: the same flag the commit-log ramp is gated on gates this,
    /// so a terminal that would print the escape sequences as text gets the
    /// coarse fallback instead.
    truecolor: bool,
}

/// What the push feature is currently doing. Private: the loop drives this
/// through [`PushUi`]'s methods and reads it only through
/// [`PushUi::mode`]/[`PushUi::overlay`].
enum State {
    /// Nothing on screen and nothing pending.
    Idle,
    /// A message under the frame.
    Status {
        /// Lines to show, already trimmed to [`MAX_STATUS_ROWS`]. How many of
        /// them a given pane has room for is [`PushUi::overlay`]'s call.
        lines: Vec<String>,
        /// How long this message stays, and how it is drawn while it does.
        life: Life,
    },
    /// A confirmation is on screen, waiting for an answer. "On screen" is the
    /// load-bearing half, and two rules keep it true: [`PushUi::request`] does
    /// not enter this state in a pane with no row to draw the question in, and
    /// [`PushUi::overlay`] leaves it when a resize takes that row away. So the
    /// mode below can never promise a question the user was not shown.
    Asking {
        question: String,
        hint: String,
        caution: bool,
        command: Confirmed,
        running_notice: String,
        success_message: String,
    },
    /// The command a question described is running, and this is what it has
    /// said so far.
    Running {
        /// What the row says about the run, without its age. It comes from the
        /// question, so the act that was confirmed is the act that is reported.
        notice: String,
        success_message: String,
        /// When the push started, against the watch loop's injected clock. The
        /// notice reports the age from it, so a hook that takes minutes looks
        /// like a push in progress rather than like a hang.
        started_at: Instant,
        /// The most recent output lines, oldest first, capped at
        /// [`MAX_PUSH_OUTPUT_ROWS`].
        ///
        /// A queue rather than a `Vec` because both ends move: a line arrives
        /// at the back and, once the window is full, one leaves the front. A
        /// `Vec` would pay for a shift of the whole buffer per line of a hook
        /// that prints thousands.
        recent: VecDeque<String>,
    },
    /// The list of the worktrees is open, and takes the pane. It owns the row
    /// as a question does: nothing is painted under the frame, and a message
    /// that arrives waits until the list closes.
    ///
    /// It is a state here, beside the states of the push, so that
    /// [`PushUi::mode`] stays the one source of what the keys mean. A second
    /// source could disagree with the first about one key.
    Listing {
        /// The open list, with its cursor and its scroll.
        list: WorktreeList,
    },
}

/// How long a [`State::Status`] message stays under the frame, and how it is
/// drawn while it does.
///
/// The split is between what gsw said and what git said, and it is one decision
/// rather than two because the two halves are the same fact. Everything gsw
/// composes itself about work that has ended — a push that worked, a push it
/// would not run — is a *report*: the user pressed a key, the answer came back,
/// and a monitor that holds it on screen for the rest of the session is
/// spending a row on news. git's error text is a *remedy*: the user has to read
/// it and act on it, so gsw must not take it away while they are looking at
/// another pane. gsw's news about work still in flight is neither, and
/// [`Life::UntilReplaced`] says how long it stays.
///
/// The age and the fade ride on this enum rather than on a flag beside it,
/// because a message that goes away on its own has to say how old it is — or it
/// is a sentence that quietly stops being true — and a message that stays has
/// no countdown to report. There is no third combination to represent.
enum Life {
    /// Stays until the user presses a key. git's own words about a push that
    /// failed, drawn red, and the one message gsw will not remove by itself.
    UntilDismissed,
    /// Says how long ago it was posted, fades toward black across
    /// [`STATUS_LIFETIME`], and then takes itself off the screen.
    Fading {
        /// When the message was posted, against the watch loop's injected
        /// clock — the same clock [`PushUi::overlay`] is later given.
        posted_at: Instant,
    },
    /// Stays until the next message takes the row. gsw's own words about work
    /// that is still in flight, posted through [`PushUi::post_progress`].
    ///
    /// Neither a key nor the clock removes it, because the words stay true
    /// until the work ends, and the end of the work posts a message of its
    /// own. It shows no age for the same reason. It is drawn as a report at
    /// age zero, because it is gsw's news and that news is current.
    UntilReplaced,
}

impl Life {
    /// How long ago a fading message was posted, as of `now`, or `None` for one
    /// that does not age.
    ///
    /// Saturating on purpose. `now` is read when the overlay is drawn and
    /// `posted_at` when the news arrived, which is earlier in every path
    /// through the loop — but a clock a test drives backwards, or a future
    /// caller that renders with a stale instant, would otherwise underflow
    /// rather than report the zero age it plainly has.
    fn elapsed(&self, now: Instant) -> Option<Duration> {
        match self {
            Self::UntilDismissed | Self::UntilReplaced => None,
            Self::Fading { posted_at } => Some(now.saturating_duration_since(*posted_at)),
        }
    }

    /// What is left of this life for a message that must wait for the row, or
    /// `None` for a message that must not wait.
    ///
    /// The instant goes on purpose, and [`HeldLife`] says why: it is the
    /// instant the message was posted, and a message that waits reaches the
    /// row later than that.
    ///
    /// [`Life::UntilReplaced`] gives `None`. Its words are true only while
    /// the work they describe is in flight, and a message that waits reaches
    /// the row after an unknown time. So a busy row drops such a message.
    fn kind(&self) -> Option<HeldLife> {
        match self {
            Self::UntilDismissed => Some(HeldLife::UntilDismissed),
            Self::Fading { .. } => Some(HeldLife::Fading),
            Self::UntilReplaced => None,
        }
    }
}

/// A message that is waiting for the row, and how long it stays once it has it.
///
/// The two travel together because the frame that frees the row knows nothing
/// about which door the message came through. A queue of lines beside a queue
/// of lives, or beside a flag, is two things that can get one step out of
/// order — and the message would then take the wrong life.
struct HeldMessage {
    /// The line to put on the row.
    line: String,
    /// What takes it off the row again.
    life: HeldLife,
}

/// How long a held message stays once the row frees up.
///
/// [`Life::Fading`] carries the instant a message was posted, and a held
/// message is posted when the row frees up rather than when it arrived. So the
/// queue holds the kind alone, and [`PushUi::post_held`] reads the clock that
/// puts it on the row. A message that waited three minutes for a push then
/// gets its whole life in front of the user, and not the end of one.
enum HeldLife {
    /// Becomes [`Life::Fading`], posted at the instant it reaches the row.
    Fading,
    /// Becomes [`Life::UntilDismissed`], which has no instant to carry.
    UntilDismissed,
}

impl HeldLife {
    /// The life a message of this kind takes when it reaches the row at `now`.
    fn at(&self, now: Instant) -> Life {
        match self {
            Self::Fading => Life::Fading { posted_at: now },
            Self::UntilDismissed => Life::UntilDismissed,
        }
    }
}

/// Where a posted message landed: on the row, or in the queue behind it.
///
/// The doors into the row answer this because the row is not always free,
/// and a caller can have work that only the first answer justifies. The `G`
/// key is that caller: the message it posts asks for a second press, and the
/// offer of that press stands exactly as long as the message on the row does.
/// A message that waits for a push is a message nobody has read, so a key
/// armed by it would take a press the user never gave a reason for.
///
/// An enum rather than a `bool`, because the two answers are two places and
/// not the presence and absence of one thing. `Posted::Held` at
/// [`MAX_HELD_MESSAGES`] is the message that was dropped as well as the one
/// that waits — neither took the row, which is the whole question here, and a
/// third answer would make every caller decide something it has no use for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Posted {
    /// The message is under the frame now.
    OnRow,
    /// A question, a push in flight, or the open list owns the row, so the
    /// message waits in [`PushUi::held`] — or, on a full queue or for a
    /// progress notice, went nowhere at all.
    Held,
}

impl PushUi {
    /// A UI with nothing on screen, drawing on a terminal that takes 24-bit
    /// color when `truecolor` says so.
    pub(crate) fn new(truecolor: bool) -> Self {
        Self {
            state: State::Idle,
            held: VecDeque::new(),
            truecolor,
        }
    }

    /// How long until what this paints changes on its own, or `None` when
    /// nothing on screen moves with the passage of time.
    ///
    /// The watch loop folds this into the same wait window as the decay tick
    /// and the refresh countdown, so a message that ages has a deadline of its
    /// own rather than relying on some other source to wake the loop. That
    /// matters most where no other source exists: `--refresh-interval 0` on a
    /// repository whose newest commit is hours old leaves the loop blocked on
    /// the channel indefinitely, and a message that expires only when something
    /// else happens does not expire.
    ///
    /// A question, a push in flight, an error that never expires, and the open
    /// list all say `None`: none of them changes with the clock, and a wake-up
    /// costs a repaint of the whole pane.
    pub(crate) fn next_tick(&self) -> Option<Duration> {
        match &self.state {
            // A running push ages the same way a fading message does, and for
            // the same reason: the notice reports its own age, and an age only
            // advances on a frame that is drawn. A hook that is quiet for a
            // minute gives the loop no other reason to draw one, so a notice
            // without this would freeze and read as a hang.
            State::Status {
                life: Life::Fading { .. },
                ..
            }
            | State::Running { .. } => Some(STATUS_CADENCE),
            State::Idle | State::Status { .. } | State::Asking { .. } | State::Listing { .. } => {
                None
            }
        }
    }

    /// What keys mean right now.
    ///
    /// [`InputMode::Confirm`] is only ever reported while the question is
    /// actually painted. [`PushUi::request`] holds that up front, by not asking
    /// a question the pane cannot show; [`PushUi::overlay`] holds it afterwards,
    /// for a pane resized down while the question is up — which is why a render
    /// can move this back to [`InputMode::Normal`] with no key pressed.
    pub(crate) fn mode(&self) -> InputMode {
        match self.state {
            State::Asking { .. } => InputMode::Confirm,
            State::Running { .. } => InputMode::Pushing,
            State::Listing { .. } => InputMode::List,
            State::Idle | State::Status { .. } => InputMode::Normal,
        }
    }

    /// Handle `p`: work out what a push would do and either ask or explain.
    ///
    /// Replaces whatever was on screen, so pressing `p` with a stale error up
    /// asks the new question instead of stacking a row under the old one.
    ///
    /// A question is only ever raised in a pane that has a row to draw it in.
    /// `dims` is the pane that is decided against, through
    /// [`Overlay::rows_to_spare`] — the same rule the render path divides the
    /// pane with, so the two cannot reach different answers about the same row.
    /// A pane with nothing to spare is left [`State::Idle`]: nothing on screen,
    /// and [`InputMode::Normal`], so the `y` or Enter behind the `p` is an
    /// ordinary key. There is deliberately no "the pane is too short" notice —
    /// a pane with no row for a one-line question has no row for a one-line
    /// status either, and an explanation that cannot be drawn explains nothing.
    ///
    /// Declining the question *here* is what closes the burst. The watch loop
    /// drains events until the channel has been quiet for a debounce interval,
    /// and classifies each one against the mode as it stands mid-drain, so a
    /// `p` and the Enter behind it — key autorepeat, a paste, a fast
    /// double-tap — are read back to back with no render in between. Any rule
    /// applied at render time is applied after the answer has already been
    /// classified. [`PushUi::overlay`] still drops a question it cannot draw,
    /// but that now covers only the pane resized down while a question is
    /// already up: the render path is the only thing that sees the new size.
    pub(crate) fn request(&mut self, snapshot: &Snapshot, dims: Dimensions, now: Instant) {
        self.ask(
            prompt_for(
                &snapshot.branch,
                snapshot.push_remote.as_deref(),
                snapshot.upstream.as_ref(),
            ),
            dims,
            now,
        );
    }

    /// Put `prompt` on the row: the question with the keys that answer it, or
    /// the refusal with a life of its own.
    ///
    /// The body every door into the row shares, so no two of them can reach
    /// different answers about the same row. It replaces whatever was on
    /// screen, which is why a key pressed with a stale error up asks its
    /// question instead of stacking a row under the old one.
    fn ask(&mut self, prompt: PushPrompt, dims: Dimensions, now: Instant) {
        self.state = match prompt {
            // Nowhere to put the question, so it is not asked. Idle rather than
            // a message: see [`PushUi::request`].
            PushPrompt::Confirm { .. } if Overlay::rows_to_spare(dims) == 0 => State::Idle,
            PushPrompt::Confirm {
                question,
                hint,
                caution,
                command,
                running_notice,
                success_message,
            } => State::Asking {
                question,
                hint,
                caution,
                command,
                running_notice,
                success_message,
            },
            // A refusal describes the repository as it stood when the key was
            // pressed, so it goes stale exactly the way a success does — and
            // costs the frame the same row until it does.
            PushPrompt::Refuse { message } => State::Status {
                lines: vec![message],
                life: Life::Fading { posted_at: now },
            },
        };
    }

    /// Handle `R` or `M`: work out what the user's command would do to the
    /// branch and either ask or explain.
    ///
    /// The door beside [`PushUi::request`], and it obeys the two rules that one
    /// obeys. A pane with no row to draw the question in raises no question and
    /// is left [`State::Idle`], so the `y` or Enter behind the key is an
    /// ordinary key. A refusal posts a fading line, because it describes the
    /// repository as it stood at the press and goes stale exactly as a success
    /// does.
    ///
    /// It takes the command because the command is the user's, and gsw learns
    /// of it from a probe that answers on the loop's own channel. The question
    /// names it, so a key whose command is not there yet has no question to
    /// ask.
    pub(crate) fn request_base_update(
        &mut self,
        snapshot: &Snapshot,
        update: BaseUpdate,
        command: &ShellCommand,
        dims: Dimensions,
        now: Instant,
    ) {
        self.ask(base_update_prompt_for(snapshot, update, command), dims, now);
    }

    /// Handle `y`: start what the question described, returning the
    /// [`Confirmed`] command to run, or `None` when no confirmation was on
    /// screen to accept.
    ///
    /// Moving to [`State::Running`] as it hands the command over is what makes
    /// a second `y` — one that raced the mode change — return `None` rather than
    /// start an overlapping run.
    pub(crate) fn confirm(&mut self, now: Instant) -> Option<Confirmed> {
        let State::Asking {
            command,
            running_notice,
            success_message,
            ..
        } = std::mem::replace(&mut self.state, State::Idle)
        else {
            return None;
        };
        self.state = State::Running {
            notice: running_notice,
            success_message,
            started_at: now,
            recent: VecDeque::new(),
        };
        Some(command)
    }

    /// Handle one line of a running push's output.
    ///
    /// Ignored in every other state. The reader threads are joined before the
    /// outcome is sent, so a line cannot really arrive after the push
    /// finished — but a window that a late line could reopen would paint over
    /// the error the user is reading, and the rule costs nothing to state.
    pub(crate) fn output_line(&mut self, line: String) {
        let State::Running { recent, .. } = &mut self.state else {
            return;
        };
        recent.push_back(line);
        // A hook can print thousands of lines, and every one of them costs
        // memory until the push ends. Trimming on arrival bounds that at the
        // window's own size rather than at the hook's output.
        while recent.len() > MAX_PUSH_OUTPUT_ROWS {
            recent.pop_front();
        }
    }

    /// Drop a message that has outlived [`STATUS_LIFETIME`].
    ///
    /// The counterpart to [`PushUi::dismiss`], and deliberately the only other
    /// way a status leaves the screen: one of them is the user saying they have
    /// read it and the other is the clock saying they have had the chance to.
    /// Everything else on screen — a question, a push in flight, git's error
    /// text — is left exactly where it is.
    fn expire(&mut self, now: Instant) {
        let State::Status { life, .. } = &self.state else {
            return;
        };
        if life
            .elapsed(now)
            .is_some_and(|elapsed| elapsed >= STATUS_LIFETIME)
        {
            self.state = State::Idle;
        }
    }

    /// Put the oldest held message on the row, if there is one and the row is
    /// free.
    ///
    /// Called from [`PushUi::overlay`], beside [`PushUi::expire`], because a
    /// render is the one moment that happens often enough and reliably enough
    /// to act on: the row is freed by a key, by a clock, and by a push that
    /// ended, and a render follows each of them.
    ///
    /// **One message per frame, and no more.** The message it posts owns the
    /// row until a key or the clock takes it away, so the next frame finds the
    /// row busy and leaves the rest of the queue alone. A queue of two thus
    /// reaches the user as two messages in order, and each one gets the whole
    /// life the first one gets. To post them all at once would put the second
    /// message where the user reads the first.
    ///
    /// `now` is the instant the message reaches the row, which is the instant
    /// a fading one starts its life from — see [`HeldLife`]. The one caller is
    /// [`PushUi::overlay`], which is already holding it.
    fn post_held(&mut self, now: Instant) {
        if !matches!(self.state, State::Idle) {
            return;
        }
        if let Some(message) = self.held.pop_front() {
            self.state = State::Status {
                lines: vec![message.line],
                life: message.life.at(now),
            };
        }
    }

    /// Handle `n`: drop the confirmation. The prompt disappearing is the whole
    /// feedback — a "cancelled" notice would itself need dismissing.
    pub(crate) fn cancel(&mut self) {
        if matches!(self.state, State::Asking { .. }) {
            self.state = State::Idle;
        }
    }

    /// Handle a finished push: replace the running notice with the outcome.
    ///
    /// On success the wording comes from the plan that was confirmed, not from
    /// git's output, so a create reports itself as a create. On failure it is
    /// git's own words — a gsw paraphrase of a push error would drop exactly
    /// the detail the user needs.
    ///
    /// The same split decides how long the message stays: `now` starts the
    /// countdown on a success, and a failure gets no countdown at all. See
    /// [`Life`] for why those are one decision.
    pub(crate) fn finished(&mut self, outcome: PushOutcome, now: Instant) {
        let success_message = match std::mem::replace(&mut self.state, State::Idle) {
            State::Running {
                success_message, ..
            } => success_message,
            // A finish with no push running: nothing to report against, so
            // leave the screen as it is rather than inventing a message.
            other => {
                self.state = other;
                return;
            }
        };

        let (lines, life) = if outcome.success {
            (vec![success_message], Life::Fading { posted_at: now })
        } else {
            (failure_lines(&outcome.output), Life::UntilDismissed)
        };
        self.state = State::Status { lines, life };
    }

    /// Put a message from a feature other than the push under the frame.
    ///
    /// The text is another program's words, so it waits for a key the way
    /// git's error text does.
    ///
    /// A question, a push in flight, and the open list each own the row, and
    /// none may be painted over: the question goes with the keys that answer
    /// it, the notice goes with the outcome the push is about to report, and
    /// the list goes with the keys that move its cursor. A message that
    /// arrives then joins the back of [`PushUi::held`], and
    /// [`PushUi::overlay`] posts the front of that queue on each frame that
    /// finds the row free.
    ///
    /// **A queue, because a second message must not erase the first.** A push
    /// takes minutes and `G` acts throughout them, so two refusals in one push
    /// is an ordinary sequence rather than a corner. Both are failures the user
    /// asked for, and both reach the screen in the order they arrived.
    ///
    /// A queue at [`MAX_HELD_MESSAGES`] drops the message that arrives, not
    /// the ones already in it. That constant says why the oldest is the one
    /// worth the row.
    pub(crate) fn post_error(&mut self, line: String) {
        // The answer goes unread here on purpose. git's words wait for a key
        // wherever they land, so a caller of this door has nothing to decide
        // from where the message went — see [`Posted`] for the caller that has.
        let _ = self.post(line, Life::UntilDismissed);
    }

    /// Put gsw's own words under the frame, to be taken off again by the clock.
    ///
    /// The second door into the row, beside [`PushUi::post_error`]. The two
    /// agree about who owns the row: a question, a push in flight, and the open
    /// list are never painted over, so a message that arrives while one of them
    /// is up joins the back of [`PushUi::held`] and waits for the frame that
    /// finds the row free. A full queue drops the message that arrives, here
    /// exactly as there — see [`MAX_HELD_MESSAGES`].
    ///
    /// They differ in one thing, and [`Life`] already says why.
    /// [`PushUi::post_error`] carries another program's words, which are a
    /// remedy: the user has to read them and act on them, so only a key takes
    /// them away. This carries gsw's own words about a key the user pressed,
    /// which are a report: it goes stale the way a push that worked goes
    /// stale, so the clock takes it away.
    ///
    /// `now` is the watch loop's injected clock, and it starts the countdown
    /// only for a message that goes straight onto the row. A message that
    /// waits for a push takes the instant of the frame that posts it
    /// instead — see [`HeldLife`].
    ///
    /// So the answer says which of those two happened, and the caller needs
    /// it. `now` starts a countdown the caller may run a clock of its own
    /// against — the `G` key runs exactly that — and that clock is a lie for a
    /// message that has not reached the row. [`Posted`] says what each answer
    /// obliges the caller to do.
    pub(crate) fn post_notice(&mut self, line: String, now: Instant) -> Posted {
        self.post(line, Life::Fading { posted_at: now })
    }

    /// Put gsw's words about work in flight under the frame, until the next
    /// message takes the row.
    ///
    /// The third door into the row. The `m` key posts through it while a
    /// measurement runs, and the words tell the user that a press of `m` does
    /// nothing now. That stays true until the run ends, so neither a key nor
    /// the clock takes the notice away, and the notice shows no age. The
    /// message that reports the end of the run replaces it, as does any other
    /// message.
    ///
    /// A question, a push in flight, and the open list own the row here, as at
    /// the other two doors. The difference is what happens to the words then:
    /// they go nowhere, and they never wait in [`PushUi::held`]. A held notice
    /// reaches the row after the run it describes has ended, and it then says
    /// that a run is in flight when none is.
    pub(crate) fn post_progress(&mut self, line: String) {
        // The answer goes unread. A notice that did not reach the row went
        // nowhere, and the caller keeps no state that stands on it.
        let _ = self.post(line, Life::UntilReplaced);
    }

    /// Put `line` on the row with `life`, or hold it until the row is free.
    ///
    /// The body the three doors share, so the rule about who owns the row is
    /// written once. A question, a push in flight, and the open list are never
    /// painted over, and a message that arrives while one of them is up joins
    /// the back of [`PushUi::held`]. Two messages go instead: a life with no
    /// [`HeldLife`], which is a progress notice, and a message that finds the
    /// queue at [`MAX_HELD_MESSAGES`].
    ///
    /// A held message keeps the kind of its life and loses the instant. The
    /// instant in `life` is the instant the message arrived, and a held
    /// message reaches the row on a later frame, so [`PushUi::post_held`]
    /// reads the clock again there. [`HeldLife`] says why that is the right
    /// end to measure from.
    ///
    /// The answer reports which of the two arms below ran, so a caller whose
    /// own state stands on the message can see whether anybody has read it
    /// yet — see [`Posted`]. A full queue and a progress notice answer
    /// [`Posted::Held`] with the message dropped, because the question is
    /// whether it took the row and it did not.
    fn post(&mut self, line: String, life: Life) -> Posted {
        match self.state {
            State::Asking { .. } | State::Running { .. } | State::Listing { .. } => {
                // A life with no kind to hold is dropped here, as a message
                // that finds the queue full is. See [`Life::kind`].
                match life.kind() {
                    Some(kind) if self.held.len() < MAX_HELD_MESSAGES => {
                        self.held.push_back(HeldMessage { line, life: kind });
                    }
                    Some(_) | None => {}
                }
                Posted::Held
            }
            State::Idle | State::Status { .. } => {
                self.state = State::Status {
                    lines: vec![line],
                    life,
                };
                Posted::OnRow
            }
        }
    }

    /// Handle a key with no other meaning: clear a status message if one is up.
    /// Leaves a question, a running push, and the open list alone — none of
    /// them is the user's to dismiss by pressing an unrelated key.
    ///
    /// A progress notice stays too. Its words are true until its work ends,
    /// and a key does not end that work. See [`Life::UntilReplaced`].
    pub(crate) fn dismiss(&mut self) {
        match self.state {
            State::Status {
                life: Life::UntilDismissed | Life::Fading { .. },
                ..
            } => self.state = State::Idle,
            State::Status {
                life: Life::UntilReplaced,
                ..
            }
            | State::Idle
            | State::Asking { .. }
            | State::Running { .. }
            | State::Listing { .. } => {}
        }
    }

    /// Take every message off the row and out of the queue: a status line of
    /// any [`Life`], the question, the open list, and every message held for
    /// the row.
    ///
    /// A switch of the worktree calls it, because each of those messages
    /// describes the worktree that the frame showed before the switch. That
    /// includes a progress notice, which no key removes, and an error that
    /// waits for a key. Neither describes the new worktree. The list closes
    /// too. No key switches while it is open, but the return to the home
    /// worktree can find it open, and the frame of home needs the pane.
    ///
    /// It is never called while a push runs. The loop does not switch then,
    /// because the window under the frame belongs to the worktree that
    /// pushes. If it is called then, the running push stays with its window,
    /// and its outcome still arrives. The held messages go all the same.
    pub(crate) fn clear(&mut self) {
        self.held.clear();
        // A running push is work in flight, and not a message. It keeps the
        // row, and its outcome takes the row when it arrives.
        if !matches!(self.state, State::Running { .. }) {
            self.state = State::Idle;
        }
    }

    /// Open the list of the worktrees that Down asked for.
    ///
    /// The list takes the pane, so it owns the row as a question does:
    /// [`PushUi::mode`] gives [`InputMode::List`], [`PushUi::overlay`] paints
    /// nothing under the frame, and a message that arrives waits in
    /// [`PushUi::held`] until the list closes.
    ///
    /// It replaces a status line, as [`PushUi::request`] does. The line
    /// describes the frame that the user stopped reading, so it does not come
    /// back when the list closes.
    ///
    /// It does not open while a question or a push owns the row. The key
    /// table never asks for the list then, and this door keeps the rule if a
    /// caller does: a question keeps the keys that answer it, and a push keeps
    /// its window until its outcome arrives.
    pub(crate) fn open_list(&mut self, list: WorktreeList) {
        match self.state {
            State::Asking { .. } | State::Running { .. } => {}
            State::Idle | State::Status { .. } | State::Listing { .. } => {
                self.state = State::Listing { list };
            }
        }
    }

    /// The open list, to draw it, or `None` when no list is open.
    pub(crate) fn list(&self) -> Option<&WorktreeList> {
        match &self.state {
            State::Listing { list } => Some(list),
            State::Idle | State::Status { .. } | State::Asking { .. } | State::Running { .. } => {
                None
            }
        }
    }

    /// The open list, to move its cursor or to settle its scroll, or `None`
    /// when no list is open.
    pub(crate) fn list_mut(&mut self) -> Option<&mut WorktreeList> {
        match &mut self.state {
            State::Listing { list } => Some(list),
            State::Idle | State::Status { .. } | State::Asking { .. } | State::Running { .. } => {
                None
            }
        }
    }

    /// Close the list and give it back with its cursor, so the caller reads
    /// the worktree that Enter chose. `None`, and no change, when no list is
    /// open.
    ///
    /// The row is free after the close. So the frame that shows the close
    /// also posts the oldest message that waited for the list, because
    /// [`PushUi::overlay`] posts a held message on each frame that finds the
    /// row free.
    pub(crate) fn close_list(&mut self) -> Option<WorktreeList> {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Listing { list } => Some(list),
            // No list is open, so nothing closes.
            other => {
                self.state = other;
                None
            }
        }
    }

    /// What the push feature shows in a pane of `dims`: the lines that fit
    /// there, how many rows the frame must give up to make room for them
    /// (`Overlay::rows`), and how many rows the frame keeps
    /// ([`Overlay::frame_rows`]).
    ///
    /// All three come out of this one call because they are one decision — how
    /// to divide `dims.height` between the frame and the message — and a
    /// decision split across two modules is a decision that can disagree with
    /// itself. A row count larger than the text leaves a blank strip between
    /// the frame and the message; a count smaller than it paints past the
    /// bottom of the pane; a frame height that does not match what is left over
    /// scrolls the screen gsw was measured to fill exactly.
    ///
    /// Two clamps, one contract. gsw's standing rule is that nothing it paints
    /// ever wraps or scrolls the pane it was measured to fill, so each line is
    /// truncated to `dims.width` by display column (UTF-8 safe), and the
    /// overlay as a whole is capped at one row short of `dims.height`. The
    /// frame therefore always keeps a row of its own.
    ///
    /// **Which rows a too-tall message loses is decided by the state, not by
    /// that cap.** The cap drops from the end, and for both of the states that
    /// can outgrow a pane the end is the part worth keeping: a failure's
    /// reason is its last line (see [`failure_lines`]) and a running push's
    /// newest output is its last row. Each arm therefore sizes itself against
    /// [`Overlay::rows_to_spare`] before returning, and the cap below is left
    /// as the backstop it is everywhere else.
    ///
    /// That second clamp is why this takes `&mut self`. In a pane with no row
    /// to spare it cuts a status down to nothing, which costs the user a
    /// message and no more. It would cut a *confirmation* down to nothing too —
    /// and a confirmation is not only text. [`PushUi::mode`] would go on
    /// reporting [`InputMode::Confirm`], so Enter would go on meaning push: the
    /// user presses Enter out of reflex at a frame that did not change, and gsw
    /// pushes to a shared remote having asked nothing. A question the user
    /// cannot see must not be answerable, so a question this pane cannot draw
    /// is cancelled here, exactly as `n` cancels one. The keys go back to
    /// normal in the same breath as the question leaves the screen, because on
    /// this prompt they are the same fact.
    ///
    /// This is the backstop, not the rule. [`PushUi::request`] refuses to raise
    /// a question a pane has no row for, using [`Overlay::rows_to_spare`] as
    /// this does, and that is what covers a `p` pressed in a pane that is
    /// already too short — it has to, because the watch loop can classify a
    /// whole burst of keys with no render between them. What is left for the
    /// cancel here is the pane that *shrinks* with a question already up: it
    /// fitted when it was asked, a resize took its row, and this is the only
    /// place that sees the new size. Nothing is lost but the question either
    /// way: `p` in a pane with a row to spare asks it again.
    ///
    /// A message that has outlived [`STATUS_LIFETIME`] is dropped here rather
    /// than by a key or a timer of its own, for the same reason the question
    /// above is: this is the one place that runs on every frame, so expiring
    /// the message and giving its row back to the frame happen in the same
    /// breath. Which is also why the loop is given [`PushUi::next_tick`] — a
    /// message can only expire on a frame that is drawn, so there has to be a
    /// frame drawn.
    pub(crate) fn overlay(&mut self, dims: Dimensions, now: Instant) -> Overlay {
        self.expire(now);
        self.post_held(now);
        let width = dims.width;
        let lines: Vec<String> = match &self.state {
            // The open list takes the pane, and its frame draws it. Nothing
            // goes under that frame.
            State::Idle | State::Listing { .. } => Vec::new(),
            State::Asking {
                question,
                hint,
                caution,
                ..
            } => {
                let line = truncate_right(&format!("{question}  {hint}"), width);
                // Yellow marks the push that puts something new on a shared
                // remote. The wording says so too — the color is what carries
                // it in the half second before the words are read.
                vec![if *caution {
                    line.yellow().to_string()
                } else {
                    line
                }]
            }
            State::Running {
                notice,
                started_at,
                recent,
                ..
            } => {
                let elapsed = now.saturating_duration_since(*started_at);
                let notice = format!("{notice} ({})", format_age_detailed(elapsed));
                let mut rows = vec![truncate_right(&notice, width)];

                // The window is sized here rather than left to the clamp at
                // the end of this function. That clamp takes rows off the
                // *end* of the list, which for a window built oldest-first
                // under a notice would drop the newest lines — the only ones
                // worth the rows — exactly in the pane with fewest to give.
                // Sizing first puts the loss at the other end, and leaves the
                // clamp as the backstop it is everywhere else.
                let spare = Overlay::rows_to_spare(dims).saturating_sub(rows.len());
                let show = recent.len().min(spare);
                rows.extend(recent.iter().skip(recent.len() - show).map(|line| {
                    // Indented and dimmed, because these rows are somebody
                    // else's words inside gsw's frame. Colored after the
                    // truncation, so the escapes cost the user no columns.
                    truncate_right(&format!("{WINDOW_INDENT}{line}"), width)
                        .dimmed()
                        .to_string()
                }));
                rows
            }
            State::Status { lines, life } => {
                // Which rows go when the pane cannot hold them all, decided
                // here rather than by the clamp at the end of this function.
                // That clamp drops from the end, and the end is where a
                // failure's reason is — see [`failure_lines`]. The running
                // window sizes itself first for the same reason.
                let dropped = lines.len().saturating_sub(Overlay::rows_to_spare(dims));
                // The age goes on the last row, which for every message that
                // has one is the only row: a success and a refusal are one
                // sentence each. The two kinds that never age are git's
                // several-line error text and a progress notice. Numbered
                // before the drop above, so the row that carries it is the
                // message's last and not merely the last one that fitted.
                let last = lines.len().saturating_sub(1);
                lines
                    .iter()
                    .enumerate()
                    .skip(dropped)
                    .map(|(row, line)| match life {
                        // Appended *before* the truncation, so the age is part
                        // of what the pane has to fit rather than something
                        // added to a row already measured against its width.
                        // Saturating for the reason [`Life::elapsed`] gives.
                        Life::Fading { posted_at } => {
                            let elapsed = now.saturating_duration_since(*posted_at);
                            let line = if row == last {
                                format!("{line} ({} ago)", format_age_detailed(elapsed))
                            } else {
                                line.clone()
                            };
                            colorize_status(&truncate_right(&line, width), elapsed, self.truecolor)
                                .to_string()
                        }
                        // gsw's news about work in flight. It does not age, so
                        // it keeps the look of a report at age zero.
                        Life::UntilReplaced => colorize_status(
                            &truncate_right(line, width),
                            Duration::ZERO,
                            self.truecolor,
                        )
                        .to_string(),
                        // git's words, which wait for a key.
                        Life::UntilDismissed => truncate_right(line, width).red().to_string(),
                    })
                    .collect()
            }
        };
        // The frame never gives up its last row. A pane painted entirely by the
        // push feature would leave the user watching an error with nothing
        // under it to say which repository it belongs to — and the frame is
        // what watch mode is for.
        let lines: Vec<String> = lines
            .into_iter()
            .take(Overlay::rows_to_spare(dims))
            .collect();
        // Nothing survived the clamp. For a status that is the end of it, but a
        // question that is not on screen must not still be answerable — the
        // keys and the question go together, so the question goes. `cancel`
        // does nothing in the states where there is no question to drop.
        if lines.is_empty() {
            self.cancel();
        }
        // The frame gets what the overlay did not take. The clamp at one covers
        // only a degenerate zero-row pane: the take above already leaves a row
        // for the frame in every pane that has one, so this floor never fights
        // the line above it for a row a real terminal reported.
        let frame_rows = dims.height.saturating_sub(lines.len()).max(1);
        Overlay { lines, frame_rows }
    }
}

/// What the push feature paints under the frame, sized for one particular pane
/// — and, with it, how tall the frame above it must be rendered.
///
/// Built only by [`PushUi::overlay`], which is what makes the row count and the
/// text impossible to disagree about: they are the same `Vec` — one measured,
/// the other joined.
///
/// The frame's height is carried here for the same reason. The pane is divided
/// once, and both halves of that division are read off the same value, so the
/// caller cannot re-derive one of them and land somewhere else. It used to
/// subtract `Overlay::rows` from the pane height itself, in another module —
/// two expressions that had to agree by inspection, about arithmetic that has
/// already produced two review findings.
pub(crate) struct Overlay {
    /// Painted lines, each already truncated to the pane's width, and at most
    /// one fewer of them than the pane has rows.
    lines: Vec<String>,
    /// Rows left for the frame, which is the pane's height less the lines
    /// above, floored at one.
    frame_rows: usize,
}

impl Overlay {
    /// How many rows an overlay may take in a pane of `dims`: every row but the
    /// one the frame keeps.
    ///
    /// The single place that rule is written down. [`PushUi::overlay`] clamps
    /// its lines to it, and [`PushUi::request`] asks it whether a question can
    /// be shown at all before raising one — two decisions about the same row,
    /// and exactly the pair that must not drift. Spelled out in both places,
    /// one of them would eventually go on offering a question the other refuses
    /// to draw, which is the defect this whole type exists to make
    /// unrepresentable.
    fn rows_to_spare(dims: Dimensions) -> usize {
        dims.height.saturating_sub(1)
    }

    /// How many rows the frame must give up. Always at least one short of the
    /// pane, so the frame keeps a row whatever the overlay wanted to say.
    ///
    /// Test-only, and that is the point of the refactor that made it so. The
    /// watch loop used to read this and subtract it from the pane height
    /// itself; it now asks for [`Overlay::frame_rows`] and gets the answer this
    /// type already worked out. Nothing outside the tests needs the count on
    /// its own any more, and a production caller that took it would be holding
    /// half of a division it could complete differently. The tests still
    /// measure it, because it is the number both halves of the split are made
    /// of.
    #[cfg(test)]
    pub(crate) fn rows(&self) -> usize {
        self.lines.len()
    }

    /// How many rows the frame is rendered with under this overlay.
    ///
    /// The frame is laid out to fill exactly the height it is given, so this is
    /// the number that keeps the two together inside the pane: appending the
    /// overlay to a full-height frame would push the frame's bottom row off the
    /// screen. Never zero — a pane with nothing but a message in it is not
    /// watch mode, so the frame keeps a row even when the pane reports none to
    /// share.
    pub(crate) fn frame_rows(&self) -> usize {
        self.frame_rows
    }

    /// The text to paint under the frame: exactly `Overlay::rows` lines, and
    /// empty when there are none.
    pub(crate) fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// The word every message about a push of gsw's own uses for it.
const PUSH_VERB: &str = "push";

/// The key hint shown with a confirmation, for an act that `verb` names.
///
/// Spelled out rather than the usual `[y/N]`. That convention's capital letter
/// means "this is what Enter gives you", and Enter *confirms* here — so `[y/N]`
/// would promise that the key people reach for by reflex is the safe one, on
/// the prompts in gsw that write to a shared remote.
///
/// The same reasoning is why a question this hint cannot be drawn with is never
/// raised (see [`PushUi::request`], and [`PushUi::overlay`] for the pane that
/// shrinks under one). What makes Enter safe to bind to a push is that the user
/// is looking at the sentence saying so. Off the screen, the binding keeps the
/// risk and loses the sentence.
///
/// One function rather than one constant for each key, because three keys now
/// ask a question and three spellings of one convention are three things that
/// drift apart. The verb is the act of the question the hint goes under, so the
/// promise names what Enter does here rather than what it does under some other
/// key.
pub(crate) fn confirm_hint(verb: &str) -> String {
    format!("[y/Enter = {verb}, n/Esc = cancel]")
}

/// What the window's rows are indented by.
///
/// The indent and the dim together say these rows are the child process
/// speaking rather than gsw, which matters because the notice above them and
/// the frame below them are both gsw's own words.
const WINDOW_INDENT: &str = "  ";

/// What a running push says while the network round trip is in flight.
///
/// The notice of a push alone. Every question carries its own now, because a
/// rebase that runs for minutes must say on the row which act is running.
pub(crate) const RUNNING_NOTICE: &str = "Pushing…";

/// How long a status message gsw wrote itself stays under the frame.
///
/// It is also the length of the fade, so the message reaches black exactly as
/// it is removed and nothing ever blinks out at full brightness.
///
/// The `G` key's own state in [`crate::watch`] reads it as well. The message
/// that asks for a second press of that key is the armed state of the key, so
/// the arming and the message it stands for must end at the same moment — one
/// number, read in both places, rather than two numbers that agree until
/// somebody changes one of them.
pub(crate) const STATUS_LIFETIME: Duration = Duration::from_secs(60);

/// How often such a message has to be repainted for its age text and its fade
/// to move.
///
/// One second, because that is the resolution of the age text — `5s ago`, then
/// `6s ago` — and a fade redrawn more often than the words beside it would cost
/// repaints nobody can read. The same cadence, for the same reason, as the
/// decay tick that advances the commit ages in the frame above.
const STATUS_CADENCE: Duration = Duration::from_secs(1);

/// The color an ageing status message is drawn in at age zero, on a terminal
/// that takes 24-bit color.
///
/// A light neutral gray rather than white: it is what an unstyled row already
/// looks like on the dark terminals gsw draws for, so the message starts where
/// it used to start and only then begins to leave.
const STATUS_RGB: (u8, u8, u8) = (208, 208, 208);

/// Fraction of [`STATUS_LIFETIME`] an ageing message keeps full brightness for
/// when there is no truecolor to fade along.
///
/// Without a gradient the fade has exactly two steps, so the step goes at the
/// half-way mark: full brightness while the news is current, dim for the rest,
/// gone at the end. Coarse, and honest about it — the alternative is a message
/// that hangs at full brightness and then vanishes.
const COARSE_FADE_AT: f32 = 0.5;

/// Color one row of an ageing status message, `elapsed` after it was posted.
///
/// The fade runs the whole length of [`STATUS_LIFETIME`] and ends at black, so
/// the row reaches the background exactly as it is removed — a message on its
/// way out looks like one, and nothing ever blinks out at full brightness. It
/// is the same shape as the commit-log gradient above it, with one difference
/// that follows from what the two are for: the log fades to a floor, because a
/// commit that has stopped being fresh is still a commit worth reading, and
/// this fades past it, because a status message that has stopped being fresh is
/// leaving.
///
/// Returns a [`ColoredString`] rather than a `String` so tests can read the
/// color off the value. `colored` decides whether to emit escapes at all from
/// process-global state, which other tests in this binary toggle.
fn colorize_status(line: &str, elapsed: Duration, truecolor: bool) -> ColoredString {
    // Cast is exact for the values involved: `STATUS_LIFETIME` is a small
    // constant and `elapsed` is clamped to it by the division below.
    #[allow(
        clippy::cast_precision_loss,
        reason = "seconds counts here are far below f32's exact-integer range"
    )]
    let spent = (elapsed.as_secs_f32() / STATUS_LIFETIME.as_secs_f32()).clamp(0.0, 1.0);

    if truecolor {
        let (r, g, b) = scale_rgb(STATUS_RGB, 1.0 - spent);
        return line.truecolor(r, g, b);
    }
    // No gradient to fade along: one step, at the half-way mark. `normal()`
    // leaves the row exactly as it was drawn before this feature — `colored`
    // emits nothing at all for a string with no color and no style.
    if spent >= COARSE_FADE_AT {
        line.dimmed()
    } else {
        line.normal()
    }
}

/// What a failed push says when git said nothing gsw could show.
const SILENT_FAILURE: &str = "git push failed";

/// Pick the lines of a failed push's output worth the rows they cost.
///
/// **The last of them, not the first.** A push git refuses on its own writes
/// exactly three non-hint lines — `To <remote>`, `! [rejected] …`,
/// `error: failed to push …` — so for that failure the head and the tail are
/// the same three and the choice does not arise. It arises the moment a
/// repository has a pre-push hook: the hook prints its whole run, fails, and
/// git adds its verdict after, so the reason sits at the end behind a banner
/// that would otherwise take every row.
///
/// Hints are dropped first either way. They follow the real error, so under a
/// tail rule they are the lines that would crowd it out. Only if dropping them
/// leaves nothing are they let back in: a message the user cannot act on beats
/// a blank row that reads as success.
fn failure_lines(output: &str) -> Vec<String> {
    let meaningful = |line: &&str| !line.trim().is_empty();
    let last = |lines: Vec<String>| -> Vec<String> {
        let dropped = lines.len().saturating_sub(MAX_STATUS_ROWS);
        lines.into_iter().skip(dropped).collect()
    };

    let mut lines = last(
        output
            .lines()
            .filter(meaningful)
            .filter(|line| !line.trim_start().starts_with(HINT_PREFIX))
            .map(|line| line.trim_end().to_string())
            .collect(),
    );

    if lines.is_empty() {
        lines = last(
            output
                .lines()
                .filter(meaningful)
                .map(|line| line.trim_end().to_string())
                .collect(),
        );
    }
    if lines.is_empty() {
        lines.push(SILENT_FAILURE.to_string());
    }
    lines
}

impl PushPlan {
    /// Resolve what a push would do, including the reasons it would do nothing.
    ///
    /// `remote` is only consulted for [`PushPlan::Create`]. An
    /// [`PushPlan::Update`] runs a bare `git push` and lets git read the remote
    /// out of the branch config, so a branch tracking a remote other than the
    /// repository's default still pushes to the right place. *Which* branch's
    /// config git reads is decided by HEAD when the child runs, so the branch
    /// resolved here travels with the arguments in a [`PushCommand`] and
    /// [`run_push`] refuses the push if the checkout has moved by then.
    fn resolve(branch: &str, remote: Option<&str>, upstream: Option<&UpstreamStatus>) -> Self {
        // Checked before the upstream, so a tracking status left over from
        // before the checkout cannot make a detached HEAD look pushable.
        if branch == DETACHED_HEAD {
            return Self::Detached;
        }

        match upstream {
            // Level with the upstream, or behind it only: `git push` would
            // report "Everything up-to-date". Say so without the round trip.
            Some(up) if up.ahead == 0 => Self::UpToDate {
                target: up.name.clone(),
            },
            // Ahead — including ahead *and* behind. A diverged branch is very
            // likely rejected as a non-fast-forward, and that rejection is what
            // the user needs to read. gsw does not pre-empt git's decision.
            Some(up) => Self::Update {
                target: up.name.clone(),
                commits: up.ahead,
            },
            None => match remote {
                Some(remote) => Self::Create {
                    remote: remote.to_string(),
                    branch: branch.to_string(),
                },
                None => Self::NoRemote,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upstream(name: &str, ahead: u32, behind: u32) -> UpstreamStatus {
        UpstreamStatus {
            name: name.to_string(),
            ahead,
            behind,
        }
    }

    /// The question a prompt asks, or the reason it refuses — whichever this
    /// prompt carries. Keeps each assertion to the one string under test.
    fn text(prompt: &PushPrompt) -> &str {
        match prompt {
            PushPrompt::Confirm { question, .. } => question,
            PushPrompt::Refuse { message } => message,
        }
    }

    /// The push a confirmable prompt carries. Panics on a refusal, so a test
    /// that expected a push and got a message fails on the line that asked, and
    /// panics on a prompt that carries another act, which no caller here plans.
    fn command(prompt: &PushPrompt) -> &PushCommand {
        match prompt {
            PushPrompt::Confirm {
                command: Confirmed::Push(command),
                ..
            } => command,
            PushPrompt::Confirm { command, .. } => {
                panic!("expected a push, got {command:?}")
            }
            PushPrompt::Refuse { message } => {
                panic!("expected a confirmable prompt, got a refusal: {message}")
            }
        }
    }

    #[test]
    fn a_branch_with_no_upstream_creates_it_on_the_remote() {
        // The common case in this workflow: a fresh worktree branch that has
        // never been pushed. The push must create the remote branch and record
        // it as the upstream, so the header's tracking segment appears and the
        // next push is a plain update.
        assert_eq!(
            PushPlan::resolve("gsw-push", Some("origin"), None),
            PushPlan::Create {
                remote: "origin".to_string(),
                branch: "gsw-push".to_string(),
            },
        );
        assert_eq!(
            command(&prompt_for("gsw-push", Some("origin"), None)).args(),
            ["push", "-u", "origin", "gsw-push"],
        );
    }

    #[test]
    fn a_tracked_branch_that_is_ahead_updates_the_remote_branch() {
        // The upstream exists, so git already knows the remote and the refspec.
        // A bare `git push` uses them, which keeps gsw from re-deriving a
        // refspec git would only override.
        let up = upstream("origin/gsw-push", 3, 0);
        assert_eq!(
            PushPlan::resolve("gsw-push", Some("origin"), Some(&up)),
            PushPlan::Update {
                target: "origin/gsw-push".to_string(),
                commits: 3,
            },
        );
        assert_eq!(
            command(&prompt_for("gsw-push", Some("origin"), Some(&up))).args(),
            ["push"]
        );
    }

    #[test]
    fn a_tracked_branch_that_is_level_has_nothing_to_push() {
        // Level with the upstream: a push would send nothing, so the plan says
        // so and the caller shows a status line instead of a prompt.
        let up = upstream("origin/gsw-push", 0, 0);
        assert_eq!(
            PushPlan::resolve("gsw-push", Some("origin"), Some(&up)),
            PushPlan::UpToDate {
                target: "origin/gsw-push".to_string(),
            },
        );
    }

    #[test]
    fn a_tracked_branch_that_is_only_behind_has_nothing_to_push() {
        // Behind but not ahead: `git push` would report "Everything up-to-date".
        // Prompting for that wastes a network round trip and reads as a failure
        // when nothing failed.
        let up = upstream("origin/gsw-push", 0, 7);
        assert_eq!(
            PushPlan::resolve("gsw-push", Some("origin"), Some(&up)),
            PushPlan::UpToDate {
                target: "origin/gsw-push".to_string(),
            },
        );
    }

    #[test]
    fn a_tracked_branch_that_is_ahead_and_behind_still_pushes() {
        // Diverged. The push will very likely be rejected as a non-fast-forward,
        // and that rejection is exactly what the user needs to see — gsw must
        // not pre-empt git's decision by refusing to try.
        let up = upstream("origin/gsw-push", 2, 5);
        assert_eq!(
            PushPlan::resolve("gsw-push", Some("origin"), Some(&up)),
            PushPlan::Update {
                target: "origin/gsw-push".to_string(),
                commits: 2,
            },
        );
    }

    #[test]
    fn a_branch_with_no_upstream_and_no_remote_cannot_be_pushed() {
        // A repository with no remote at all (or none gsw can pick). There is
        // nowhere to push, so there is nothing to confirm.
        assert_eq!(
            PushPlan::resolve("gsw-push", None, None),
            PushPlan::NoRemote,
        );
    }

    #[test]
    fn a_detached_head_cannot_be_pushed() {
        // `repo::branch_name` reports `HEAD` when HEAD is detached, and git
        // refuses `HEAD` as a branch name, so this sentinel can never collide
        // with a real branch. Pushing it would create a remote branch literally
        // named `HEAD`.
        assert_eq!(
            PushPlan::resolve(DETACHED_HEAD, Some("origin"), None),
            PushPlan::Detached,
        );
    }

    #[test]
    fn a_detached_head_cannot_be_pushed_even_with_a_stale_upstream() {
        // Belt and braces: a detached HEAD must be refused before the upstream
        // is consulted, so a leftover tracking status cannot make it pushable.
        let up = upstream("origin/main", 4, 0);
        assert_eq!(
            PushPlan::resolve(DETACHED_HEAD, Some("origin"), Some(&up)),
            PushPlan::Detached,
        );
    }

    #[test]
    fn the_create_plan_uses_the_remote_it_was_given() {
        // A repository whose only remote is not named `origin`. The plan must
        // carry that name through to the command rather than assuming `origin`.
        assert_eq!(
            command(&prompt_for("gsw-push", Some("fork"), None)).args(),
            ["push", "-u", "fork", "gsw-push"],
        );
    }

    #[test]
    fn creating_a_remote_branch_says_so_and_is_marked() {
        // The point of the whole variant: a branch that does not exist on the
        // remote must not be confirmed with the same sentence as a routine
        // update. The wording names the act, and the flag lets the display
        // layer color it apart.
        let prompt = prompt_for("gsw-push", Some("origin"), None);
        assert_eq!(text(&prompt), "Create new remote branch origin/gsw-push?");
        assert!(
            matches!(prompt, PushPrompt::Confirm { caution: true, .. },),
            "a create must be flagged so the display layer can set it apart",
        );
    }

    #[test]
    fn updating_an_existing_remote_branch_counts_the_commits() {
        // The routine case. It names the target and how much is going, and it
        // is NOT flagged as a create — nothing new appears on the remote.
        let up = upstream("origin/gsw-push", 3, 0);
        let prompt = prompt_for("gsw-push", Some("origin"), Some(&up));
        assert_eq!(text(&prompt), "Push 3 commits to origin/gsw-push?");
        assert!(
            matches!(prompt, PushPrompt::Confirm { caution: false, .. },),
            "updating an existing branch must not be flagged as a create",
        );
    }

    #[test]
    fn one_commit_is_singular() {
        // "Push 1 commits" reads as a bug in the tool and undermines trust in
        // the number right next to it.
        let up = upstream("origin/gsw-push", 1, 0);
        assert_eq!(
            text(&prompt_for("gsw-push", Some("origin"), Some(&up))),
            "Push 1 commit to origin/gsw-push?",
        );
    }

    #[test]
    fn the_confirmation_carries_the_command_it_describes() {
        // The question, the argument list, and the branch the question named
        // travel together, so what runs on `y` is what the sentence promised —
        // and the runner can still tell whether the repository moved under it.
        let prompt = prompt_for("gsw-push", Some("origin"), None);
        assert_eq!(
            command(&prompt).args(),
            ["push", "-u", "origin", "gsw-push"]
        );
        assert_eq!(command(&prompt).branch(), "gsw-push");
    }

    #[test]
    fn a_bare_push_still_names_the_branch_it_was_confirmed_for() {
        // The argument list for an update says nothing about which branch it
        // pushes — git resolves that from HEAD when it runs. The command has to
        // carry the branch anyway, or nothing downstream can tell that the
        // checkout changed between the question and the answer.
        let up = upstream("origin/gsw-push", 3, 0);
        let prompt = prompt_for("gsw-push", Some("origin"), Some(&up));
        assert_eq!(command(&prompt).args(), ["push"]);
        assert_eq!(command(&prompt).branch(), "gsw-push");
    }

    #[test]
    fn an_up_to_date_branch_is_refused_by_name() {
        // Not an error, and not a prompt either: pressing `p` on a fully-pushed
        // branch must say why nothing happened, naming the branch it checked.
        assert_eq!(
            prompt_for(
                "gsw-push",
                Some("origin"),
                Some(&upstream("origin/gsw-push", 0, 0))
            ),
            PushPrompt::Refuse {
                message: "origin/gsw-push is already up to date".to_string(),
            },
        );
    }

    #[test]
    fn a_detached_head_is_refused_with_the_way_out() {
        // The message names the fix, because "cannot push" alone leaves the
        // user guessing at what gsw objected to.
        assert_eq!(
            prompt_for(DETACHED_HEAD, Some("origin"), None),
            PushPrompt::Refuse {
                message: "HEAD is detached — check out a branch to push".to_string(),
            },
        );
    }

    #[test]
    fn a_repository_with_no_remote_is_refused() {
        assert_eq!(
            prompt_for("gsw-push", None, None),
            PushPrompt::Refuse {
                message: "no remote to push to".to_string(),
            },
        );
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    use crate::render::Snapshot;
    use crate::worktrees::{WorktreeEntry, WorktreePath};
    use testcolor::{max_red_channel, TRUECOLOR_FG};

    /// A snapshot on `gsw-push` with `origin` available and the given tracking
    /// status. Only the four fields the push feature reads matter here.
    fn snapshot(upstream: Option<UpstreamStatus>) -> Snapshot {
        Snapshot {
            branch: "gsw-push".to_string(),
            base: "main".to_string(),
            commits_ahead: 0,
            commits_behind: 0,
            files: Vec::new(),
            log: Vec::new(),
            upstream,
            operation: None,
            push_remote: Some("origin".to_string()),
            worktree: None,
        }
    }

    fn tracked(ahead: u32) -> Option<UpstreamStatus> {
        Some(UpstreamStatus {
            name: "origin/gsw-push".to_string(),
            ahead,
            behind: 0,
        })
    }

    /// The display width of `line` in terminal columns, ignoring ANSI escapes.
    ///
    /// The escapes have to be stripped rather than assumed absent.
    /// `colored` decides whether to emit them from process-global state that
    /// other tests in this binary toggle, so a raw `UnicodeWidthStr::width`
    /// counts escape bytes as columns in some runs and not others — the
    /// assertion would pass or fail depending on test order. Columns on screen
    /// are also the thing under test: the overlay colors *after* truncating,
    /// so the escapes cost the user nothing.
    ///
    /// Stripped by `testcolor`, which is the workspace's one stripper. This
    /// used to skip from an escape to the next ASCII letter, which is right
    /// for the CSI sequences `colored` emits and wrong for the rest — and the
    /// window under a running push paints whatever a hook wrote, which is not
    /// a set of sequences anyone here chooses.
    fn visible_width(line: &str) -> usize {
        unicode_width::UnicodeWidthStr::width(testcolor::strip_ansi(line).as_str())
    }

    /// The instant every test starts from.
    ///
    /// `Instant` has no constructor, so the origin is read from the real clock
    /// once and every later moment is derived by adding to it. Nothing here
    /// waits for real time to pass: a test that wants a minute-old message adds
    /// a minute to this and hands the result to [`PushUi::overlay`].
    ///
    /// Read fresh on each call rather than cached in a `static`, so no test can
    /// leave a mutated origin behind for the next one — the value is only ever
    /// compared against instants derived from the same call.
    fn t0() -> Instant {
        Instant::now()
    }

    /// A pane `width` columns wide with more rows than any overlay can want, so
    /// a test about wording or row count is not also a test about clipping.
    fn tall_pane(width: usize) -> Dimensions {
        Dimensions {
            width,
            height: MAX_STATUS_ROWS + 10,
        }
    }

    /// A UI with the confirmation already on screen for an untracked branch.
    fn asking() -> PushUi {
        let mut ui = PushUi::new(false);
        ui.request(&snapshot(None), tall_pane(80), t0());
        ui
    }

    /// How far behind the base every question about a base update here is
    /// asked. Any count above zero does: the count is the reason the key acts
    /// at all.
    const BEHIND: u32 = 5;

    /// A snapshot of `gsw-push`, [`BEHIND`] commits behind `main`, which is
    /// what a rebase or a merge is asked about.
    fn behind_the_base() -> Snapshot {
        Snapshot {
            commits_behind: BEHIND,
            ..snapshot(None)
        }
    }

    /// The command `update` runs here, which is the one it falls back on.
    fn base_update_command(update: BaseUpdate) -> ShellCommand {
        ShellCommand::new(None, update.default_command()).expect("a name")
    }

    /// A UI with the question of `update` already on screen, asked at `now` in
    /// a pane of `dims`.
    fn asking_base_update_in(update: BaseUpdate, dims: Dimensions, now: Instant) -> PushUi {
        let mut ui = PushUi::new(false);
        ui.request_base_update(
            &behind_the_base(),
            update,
            &base_update_command(update),
            dims,
            now,
        );
        ui
    }

    /// A UI with the question of `update` already on screen, asked at `now` in
    /// a pane with room for it.
    fn asking_base_update(update: BaseUpdate, now: Instant) -> PushUi {
        asking_base_update_in(update, tall_pane(80), now)
    }

    /// A UI with a push already running, confirmed at `now`.
    fn pushing(now: Instant) -> PushUi {
        let mut ui = PushUi::new(false);
        ui.request(&snapshot(None), tall_pane(80), now);
        ui.confirm(now)
            .expect("the confirmation must hand over a command");
        ui
    }

    /// What `ui` paints, as the glyphs a user reads.
    ///
    /// The escapes are forced on and then taken back out, so the assertion
    /// covers the painted output rather than a plain render no terminal would
    /// produce. `colored` decides at format time, from process-global state
    /// that other tests in this binary toggle, so reading `text()` raw would
    /// compare different bytes depending on whether the run had a terminal.
    fn painted(ui: &mut PushUi, dims: Dimensions, now: Instant) -> String {
        testcolor::strip_ansi(&testcolor::with_forced_ansi(|| {
            ui.overlay(dims, now).text()
        }))
    }

    /// What `ui` paints in a pane wide enough for a question, with the escapes
    /// left in, so a test can read the color off the row.
    ///
    /// The escapes are forced on for the same reason [`painted`] forces them
    /// on: `colored` decides at format time from process-global state that
    /// other tests in this binary toggle, so a raw render carries no color at
    /// all in some runs. The one door to that override is `testcolor`, and
    /// clippy bans every other spelling of it.
    fn escapes(ui: &mut PushUi, now: Instant) -> String {
        testcolor::with_forced_ansi(|| ui.overlay(tall_pane(120), now).text())
    }

    #[test]
    fn a_message_from_another_feature_goes_under_the_frame_and_waits_for_a_key() {
        // The `G` key runs somebody else's command, and a command that refuses
        // says why. That refusal is the whole reason the key did nothing.
        let now = t0();
        let mut ui = PushUi::new(false);
        ui.post_error("branch main names no issue".to_string());

        let text = painted(&mut ui, tall_pane(80), now);
        assert!(
            text.contains("branch main names no issue"),
            "the message must reach the screen, got {text:?}",
        );
        assert_eq!(
            ui.next_tick(),
            None,
            "a message that waits for a key does not age",
        );

        ui.dismiss();
        let text = painted(&mut ui, tall_pane(80), now);
        assert_eq!(text, "", "a key must take it off the screen");
    }

    #[test]
    fn a_message_from_another_feature_leaves_a_running_push_on_the_screen() {
        // `G` acts while a push runs, so the two features can reach the one
        // row at once. The push owns it: taking its notice away would lose the
        // outcome the push is about to report.
        let now = t0();
        let mut ui = pushing(now);
        ui.post_error("branch main names no issue".to_string());

        let text = painted(&mut ui, tall_pane(80), now);
        assert!(
            text.contains(RUNNING_NOTICE),
            "the push must keep the rows it is using, got {text:?}",
        );
        assert!(
            !text.contains("names no issue"),
            "the held message must wait its turn, got {text:?}",
        );
        assert_eq!(ui.mode(), InputMode::Pushing, "the push is still running");
    }

    #[test]
    fn a_message_from_another_feature_arrives_once_the_push_is_done_with_the_row() {
        // Held is not dropped. Silence belongs to one case only, and this is
        // not it.
        let now = t0();
        let mut ui = pushing(now);
        ui.post_error("branch main names no issue".to_string());
        ui.finished(
            PushOutcome {
                success: true,
                output: String::new(),
            },
            now,
        );

        // The push's own message ages off the screen first, and the held one
        // takes the row it leaves.
        let later = now + STATUS_LIFETIME;
        let text = painted(&mut ui, tall_pane(80), later);
        assert!(
            text.contains("branch main names no issue"),
            "the held message must reach the screen, got {text:?}",
        );
    }

    #[test]
    fn a_message_from_another_feature_leaves_the_question_on_the_screen() {
        // A question and the keys that answer it go together. A message that
        // took the question away would leave the mode answering nothing.
        let now = t0();
        let mut ui = asking();
        ui.post_error("branch main names no issue".to_string());

        let text = painted(&mut ui, tall_pane(80), now);
        assert_eq!(
            ui.mode(),
            InputMode::Confirm,
            "the question must still be on screen",
        );
        assert!(
            !text.contains("names no issue"),
            "the held message must wait its turn, got {text:?}",
        );
    }

    /// Every message `ui` puts on the row from `now` on, in the order a user
    /// reads them.
    ///
    /// Each pass paints one frame, records what the row carries, and presses a
    /// key — which is what a user does with a message that waits for one. The
    /// pass stops at the first blank frame. The count above it is a backstop:
    /// a queue that never empties must fail a test rather than hold the run
    /// open.
    fn drained(ui: &mut PushUi, now: Instant) -> Vec<String> {
        let mut seen = Vec::new();
        for _ in 0..MAX_HELD_MESSAGES + 4 {
            let text = painted(ui, tall_pane(80), now);
            if text.is_empty() {
                break;
            }
            seen.push(text);
            ui.dismiss();
        }
        seen
    }

    #[test]
    fn two_messages_held_during_a_push_both_reach_the_screen_in_order() {
        // A run started by `G` can end while the push is still going, and the
        // key is free again the moment it does. So a second `G` refuses with
        // the first refusal still waiting. The user asked for both runs, and
        // both owe an answer.
        let now = t0();
        let mut ui = pushing(now);
        ui.post_error("the first refusal".to_string());
        ui.post_error("the second refusal".to_string());
        ui.finished(
            PushOutcome {
                success: true,
                output: String::new(),
            },
            now,
        );

        // The push's own message ages off the screen first, and the oldest
        // held message takes the row it leaves.
        let later = now + STATUS_LIFETIME;
        let text = painted(&mut ui, tall_pane(80), later);
        assert!(
            text.contains("the first refusal"),
            "the first message must come first, got {text:?}",
        );
        assert!(
            !text.contains("the second refusal"),
            "the second message must wait its turn, got {text:?}",
        );

        // A key is the user's word that the first message was read. The second
        // takes the row it leaves, and waits for a key of its own.
        ui.dismiss();
        let text = painted(&mut ui, tall_pane(80), later);
        assert!(
            text.contains("the second refusal"),
            "the second message must follow the first, got {text:?}",
        );
    }

    #[test]
    fn a_held_message_is_not_lost_to_a_second_one() {
        // The narrow statement of the defect: one slot held one message, so a
        // second refusal wrote over the first and the user never saw it.
        let now = t0();
        let mut ui = pushing(now);
        ui.post_error("the first refusal".to_string());
        ui.post_error("the second refusal".to_string());
        ui.finished(
            PushOutcome {
                success: true,
                output: String::new(),
            },
            now,
        );

        let seen = drained(&mut ui, now + STATUS_LIFETIME);
        assert!(
            seen.iter().any(|text| text.contains("the first refusal")),
            "the first message must reach the screen, got {seen:?}",
        );
    }

    #[test]
    fn a_full_queue_of_held_messages_drops_the_newest() {
        // The bound is what stops a push of several minutes from filling the
        // row with keys to press. Which end it drops is the point: the first
        // refusal says what went wrong, and the ones after it repeat it.
        let now = t0();
        let mut ui = pushing(now);
        for index in 0..MAX_HELD_MESSAGES + 1 {
            ui.post_error(format!("refusal number {index}"));
        }
        ui.finished(
            PushOutcome {
                success: true,
                output: String::new(),
            },
            now,
        );

        let seen = drained(&mut ui, now + STATUS_LIFETIME);
        assert!(
            seen.iter().any(|text| text.contains("refusal number 0")),
            "the oldest message must survive a full queue, got {seen:?}",
        );
        let newest = format!("refusal number {MAX_HELD_MESSAGES}");
        assert!(
            !seen.iter().any(|text| text.contains(&newest)),
            "the newest message is the one a full queue drops, got {seen:?}",
        );
    }

    /// What a notice says. The wording belongs to the key that posts one, and
    /// the tests below are about how long it stays.
    const NOTICE: &str = "remote shell — press G again";

    #[test]
    fn a_notice_takes_itself_off_the_screen() {
        // gsw's own words about a key the user pressed. They are a report, and
        // a report that stays until somebody types at the monitor is a row
        // spent for the rest of the session.
        let now = t0();
        let mut ui = PushUi::new(false);
        ui.post_notice(NOTICE.to_string(), now);

        let text = painted(&mut ui, tall_pane(80), now);
        assert!(
            text.contains(NOTICE),
            "the notice must reach the screen, got {text:?}",
        );
        assert!(
            text.contains("(0s ago)"),
            "a message the clock removes says how old it is, got {text:?}",
        );
        assert_eq!(
            ui.next_tick(),
            Some(STATUS_CADENCE),
            "a message that ages must wake the loop to age",
        );

        let text = painted(&mut ui, tall_pane(80), now + STATUS_LIFETIME);
        assert_eq!(text, "", "the clock must take the notice away");
    }

    #[test]
    fn a_message_from_another_feature_still_waits_for_a_key_a_lifetime_later() {
        // The other door is unchanged by the one above it. Another program's
        // words are a remedy, and a remedy that leaves on its own while the
        // user reads another pane is worse than a row spent.
        let now = t0();
        let mut ui = PushUi::new(false);
        ui.post_error("branch main names no issue".to_string());

        let later = now + STATUS_LIFETIME;
        let text = painted(&mut ui, tall_pane(80), later);
        assert!(
            text.contains("branch main names no issue"),
            "an error must outlive the lifetime a notice has, got {text:?}",
        );
        assert!(
            !text.contains("ago"),
            "a message that never expires has no countdown to report, got {text:?}",
        );

        ui.dismiss();
        let text = painted(&mut ui, tall_pane(80), later);
        assert_eq!(text, "", "a key is still what clears it");
    }

    #[test]
    fn a_notice_that_arrives_during_a_push_waits_for_the_row() {
        // `G` acts while a push runs, and the push owns the row: its notice
        // goes with the outcome it is about to report.
        let now = t0();
        let mut ui = pushing(now);
        ui.post_notice(NOTICE.to_string(), now);

        let text = painted(&mut ui, tall_pane(80), now);
        assert!(
            text.contains(RUNNING_NOTICE),
            "the push must keep the rows it is using, got {text:?}",
        );
        assert!(
            !text.contains(NOTICE),
            "the held notice must wait its turn, got {text:?}",
        );
        assert_eq!(ui.mode(), InputMode::Pushing, "the push is still running");
    }

    #[test]
    fn a_notice_that_waited_for_the_row_gets_its_whole_life_on_it() {
        // A push with a pre-push hook takes minutes, and a notice posted at
        // the start of one reaches the screen at the end. Its life starts
        // where the user can read it: a notice that carried the instant it
        // arrived would appear already expired and go on the next frame.
        let now = t0();
        let mut ui = pushing(now);
        ui.post_notice(NOTICE.to_string(), now);

        // The push runs for longer than a notice lives, and its own message
        // then takes the row for a lifetime of its own.
        let push_ended = now + STATUS_LIFETIME * 2;
        ui.finished(
            PushOutcome {
                success: true,
                output: String::new(),
            },
            push_ended,
        );
        let text = painted(&mut ui, tall_pane(80), push_ended);
        assert!(
            !text.contains(NOTICE),
            "the push's own outcome comes first, got {text:?}",
        );

        // The push's message ages off, and the notice takes the row it leaves.
        let arrived = push_ended + STATUS_LIFETIME;
        let text = painted(&mut ui, tall_pane(80), arrived);
        assert!(
            text.contains(NOTICE),
            "the held notice must reach the screen, got {text:?}",
        );
        assert!(
            text.contains("(0s ago)"),
            "the life of a held notice starts on the row, got {text:?}",
        );

        let text = painted(&mut ui, tall_pane(80), arrived + STATUS_CADENCE);
        assert!(
            text.contains(NOTICE),
            "the notice must still be there a moment later, got {text:?}",
        );

        let text = painted(&mut ui, tall_pane(80), arrived + STATUS_LIFETIME);
        assert_eq!(
            text, "",
            "the clock must take the notice away a lifetime after it arrived",
        );
    }

    #[test]
    fn an_error_that_waited_for_the_row_still_waits_for_a_key() {
        // The queue carries which life a message takes, and it must carry the
        // other one unchanged. An error that waited for a push is still a
        // remedy when it reaches the row.
        let now = t0();
        let mut ui = pushing(now);
        ui.post_error("branch main names no issue".to_string());
        ui.finished(
            PushOutcome {
                success: true,
                output: String::new(),
            },
            now,
        );

        // The push's own message ages off, and the held error takes the row.
        let arrived = now + STATUS_LIFETIME;
        let text = painted(&mut ui, tall_pane(80), arrived);
        assert!(
            text.contains("branch main names no issue"),
            "the held error must reach the screen, got {text:?}",
        );

        let text = painted(&mut ui, tall_pane(80), arrived + STATUS_LIFETIME * 3);
        assert!(
            text.contains("branch main names no issue"),
            "a held error must not expire once it is on the row, got {text:?}",
        );

        ui.dismiss();
        let text = painted(&mut ui, tall_pane(80), arrived);
        assert_eq!(text, "", "a key is what clears it");
    }

    /// Every kind of thing a switch of the worktree must take off the row, with
    /// a name for a failed assertion. Each one describes the worktree that the
    /// frame showed before the switch.
    fn rows_a_switch_takes_away(now: Instant) -> Vec<(&'static str, PushUi)> {
        let mut notice = PushUi::new(false);
        let _ = notice.post_notice(NOTICE.to_string(), now);

        let mut error = PushUi::new(false);
        error.post_error("branch main names no issue".to_string());

        let mut progress = PushUi::new(false);
        progress.post_progress("Running grind and grime against main…".to_string());

        // A push that ended leaves its outcome on the row, and a message that
        // arrived during the push waits behind it.
        let mut outcome_and_held = pushing(now);
        outcome_and_held.post_error("held behind the push".to_string());
        outcome_and_held.finished(
            PushOutcome {
                success: true,
                output: String::new(),
            },
            now,
        );

        let mut question_and_held = asking();
        question_and_held.post_error("held behind the question".to_string());

        vec![
            ("a notice that fades", notice),
            ("an error that waits for a key", error),
            ("a progress notice", progress),
            (
                "a push outcome with a message held behind it",
                outcome_and_held,
            ),
            ("the question", asking()),
            (
                "a question with a message held behind it",
                question_and_held,
            ),
        ]
    }

    #[test]
    fn clear_takes_every_message_off_the_row_and_out_of_the_queue() {
        // A switch of the worktree calls `clear`. Every message under the
        // frame describes the worktree that the frame showed before, so the
        // row must be empty after it, and no held message may take the row
        // later. A question goes with the keys that answer it.
        let now = t0();
        for (what, mut ui) in rows_a_switch_takes_away(now) {
            assert_ne!(
                painted(&mut ui, tall_pane(80), now),
                "",
                "{what}: the row must carry something before the clear",
            );

            ui.clear();

            // The mode is the one source of what the keys mean, so a question
            // that left the mode left its keys too.
            assert_eq!(ui.mode(), InputMode::Normal, "{what}: no question stays");
            assert_eq!(
                painted(&mut ui, tall_pane(80), now),
                "",
                "{what}: the row must be empty after the clear",
            );
            assert_eq!(
                painted(&mut ui, tall_pane(80), now + STATUS_LIFETIME),
                "",
                "{what}: no held message may reach the row later",
            );
            assert_eq!(ui.next_tick(), None, "{what}: nothing is left to age");
        }
    }

    #[test]
    fn clear_leaves_a_running_push_alone_and_empties_the_queue() {
        // The loop never switches while a push runs, so it never calls `clear`
        // then. A call then leaves the push on the row with its window, and
        // the outcome of the push still arrives. The held messages go all the
        // same, because each one describes the worktree the frame showed
        // before.
        let now = t0();
        let mut ui = pushing(now);
        ui.output_line("Compiling gsw v0.1.0".to_string());
        ui.post_error("held behind the push".to_string());

        ui.clear();

        assert_eq!(ui.mode(), InputMode::Pushing, "the push is still running");
        let text = painted(&mut ui, tall_pane(80), now);
        assert!(
            text.contains(RUNNING_NOTICE),
            "the push must keep its row, got {text:?}",
        );
        assert!(
            text.contains("Compiling gsw v0.1.0"),
            "the push must keep its window, got {text:?}",
        );

        ui.finished(
            PushOutcome {
                success: false,
                output: "error: failed to push some refs\n".to_string(),
            },
            now,
        );
        let text = painted(&mut ui, tall_pane(80), now);
        assert!(
            text.contains("error: failed to push some refs"),
            "the outcome of the push must still arrive, got {text:?}",
        );

        ui.dismiss();
        let text = painted(&mut ui, tall_pane(80), now);
        assert_eq!(text, "", "no held message may reach the row, got {text:?}");
    }

    /// A list of three worktrees, sorted by path, with the cursor and the
    /// home worktree on the middle row, `bravo`.
    fn three_worktrees() -> WorktreeList {
        let entries: Vec<WorktreeEntry> = ["alpha", "bravo", "charlie"]
            .into_iter()
            .map(|name| WorktreeEntry {
                path: WorktreePath::fake(format!("/code/{name}")),
                label: name.to_string(),
            })
            .collect();
        let middle = entries[1].path.clone();
        WorktreeList::open(entries, &middle, middle.clone()).expect("a list with rows opens")
    }

    /// The label of the row under the cursor of the open list of `ui`, or
    /// `None` when no list is open.
    fn cursor_of(ui: &PushUi) -> Option<&str> {
        ui.list().map(|list| list.selected().label.as_str())
    }

    #[test]
    fn an_open_list_takes_the_keys_and_paints_nothing_under_the_frame() {
        // The list takes the pane, so the keys mean what they mean in the
        // list, and the row under the frame carries nothing. Nothing on the
        // list ages, so the loop has no reason to wake for it. A key with no
        // meaning leaves the list open, as it leaves a question on the row.
        let now = t0();
        let pane = tall_pane(80);
        let mut ui = PushUi::new(false);
        ui.open_list(three_worktrees());

        assert_eq!(ui.mode(), InputMode::List, "the list takes the keys");
        assert_eq!(cursor_of(&ui), Some("bravo"), "the cursor starts on bravo");
        let overlay = ui.overlay(pane, now);
        assert_eq!(
            overlay.text(),
            "",
            "the list paints nothing under the frame"
        );
        assert_eq!(
            overlay.frame_rows(),
            pane.height,
            "the frame of the list takes the whole pane",
        );
        assert_eq!(ui.next_tick(), None, "nothing on the list ages");

        ui.dismiss();
        assert_eq!(
            ui.mode(),
            InputMode::List,
            "a key with no meaning leaves the list open",
        );

        ui.list_mut().expect("the list is open").down();
        assert_eq!(cursor_of(&ui), Some("charlie"), "the cursor moves");

        let closed = ui.close_list().expect("a close gives the open list back");
        assert_eq!(
            closed.selected().label,
            "charlie",
            "the list comes back with its cursor",
        );
        assert_eq!(ui.mode(), InputMode::Normal, "the keys go back to normal");
        assert_eq!(cursor_of(&ui), None, "no list stays open");
        assert!(ui.close_list().is_none(), "a second close finds no list");
    }

    #[test]
    fn a_message_posted_while_the_list_is_open_waits_for_the_list_to_close() {
        // The list owns the row, as a question does. A message that arrives
        // through either door waits in the queue, and each one reaches the
        // row in turn once the list closes. A progress notice goes nowhere:
        // its words are true only while its work is in flight, and the work
        // can end while the list is open.
        let now = t0();
        let mut ui = PushUi::new(false);
        ui.open_list(three_worktrees());
        ui.post_error("the first message".to_string());
        let notice = ui.post_notice("the second message".to_string(), now);
        ui.post_progress("work in flight".to_string());

        assert_eq!(
            notice,
            Posted::Held,
            "a notice that finds the list open waits"
        );
        assert_eq!(ui.mode(), InputMode::List, "no message closes the list");
        assert_eq!(
            painted(&mut ui, tall_pane(80), now),
            "",
            "no message reaches the row under the list",
        );

        let _ = ui.close_list();
        assert_eq!(
            drained(&mut ui, now),
            ["the first message", "the second message (0s ago)"],
            "each held message reaches the row in turn, and the progress notice never does",
        );
    }

    #[test]
    fn opening_the_list_replaces_a_status_line() {
        // Down opens the list over the line on the row, as `p` asks its
        // question over it. The line describes the frame that the user
        // stopped reading, so it does not come back when the list closes.
        let now = t0();
        let mut notice = PushUi::new(false);
        let _ = notice.post_notice("a notice that fades".to_string(), now);
        let mut error = PushUi::new(false);
        error.post_error("an error that waits for a key".to_string());
        let mut progress = PushUi::new(false);
        progress.post_progress("a progress notice".to_string());

        for (what, mut ui) in [
            ("a notice that fades", notice),
            ("an error that waits for a key", error),
            ("a progress notice", progress),
        ] {
            ui.open_list(three_worktrees());
            assert_eq!(ui.mode(), InputMode::List, "{what}: the list must open");
            let _ = ui.close_list();
            assert_eq!(
                painted(&mut ui, tall_pane(80), now),
                "",
                "{what}: the line must not come back when the list closes",
            );
        }
    }

    #[test]
    fn clear_closes_the_list_and_empties_the_queue() {
        // The return to the home worktree calls `clear` with the list open.
        // The list shows the worktrees as the frame of the old worktree saw
        // them, so it closes, and no message held behind it takes the row.
        let now = t0();
        let mut ui = PushUi::new(false);
        ui.open_list(three_worktrees());
        ui.post_error("held behind the list".to_string());
        assert_eq!(
            ui.mode(),
            InputMode::List,
            "the fixture must start with the list open"
        );

        ui.clear();

        assert_eq!(ui.mode(), InputMode::Normal, "the list must close");
        assert_eq!(cursor_of(&ui), None, "no list stays open");
        assert_eq!(
            painted(&mut ui, tall_pane(80), now),
            "",
            "no held message may reach the row",
        );
    }

    #[test]
    fn the_list_does_not_open_while_a_question_or_a_push_owns_the_row() {
        // The key table never asks for the list then, but the door must not
        // break what owns the row. A question keeps the keys that answer it,
        // and a push keeps its window until its outcome arrives.
        let now = t0();
        let mut ui = asking();
        ui.open_list(three_worktrees());
        assert_eq!(
            ui.mode(),
            InputMode::Confirm,
            "the question must keep the keys"
        );
        assert_eq!(cursor_of(&ui), None, "no list may open over the question");
        assert!(ui.confirm(now).is_some(), "the question must still answer");

        let mut ui = pushing(now);
        ui.output_line("Compiling gsw v0.1.0".to_string());
        ui.open_list(three_worktrees());
        assert_eq!(ui.mode(), InputMode::Pushing, "the push must keep the row");
        assert_eq!(cursor_of(&ui), None, "no list may open over the push");
        let text = painted(&mut ui, tall_pane(80), now);
        assert!(
            text.contains(RUNNING_NOTICE) && text.contains("Compiling gsw v0.1.0"),
            "the push must keep its notice and its window, got {text:?}",
        );
    }

    #[test]
    fn a_line_reported_while_pushing_appears_under_the_notice() {
        // The whole feature: a long pre-push hook leaves the user watching a
        // frozen "Pushing…" with no way to tell work from a hang.
        let now = t0();
        let mut ui = pushing(now);
        ui.output_line("Compiling gsw v0.1.0".to_string());

        let text = painted(&mut ui, tall_pane(80), now);
        assert!(
            text.contains(RUNNING_NOTICE),
            "the notice must stay above the window, got {text:?}",
        );
        assert!(
            text.contains("Compiling gsw v0.1.0"),
            "the hook's line must reach the screen, got {text:?}",
        );
    }

    #[test]
    fn the_window_keeps_the_newest_lines_and_drops_the_oldest() {
        // A hook that builds a workspace prints hundreds of lines. The window
        // is six rows, and the six worth having are the six that just arrived.
        let now = t0();
        let mut ui = pushing(now);
        for step in 0..MAX_PUSH_OUTPUT_ROWS + 4 {
            ui.output_line(format!("line {step}"));
        }

        let text = painted(&mut ui, tall_pane(80), now);
        assert_eq!(
            text.lines().count(),
            MAX_PUSH_OUTPUT_ROWS + 1,
            "the notice plus a full window, got {text:?}",
        );
        assert!(
            text.contains("line 9") && text.contains("line 4"),
            "the newest six must be on screen, got {text:?}",
        );
        assert!(
            !text.contains("line 0") && !text.contains("line 3"),
            "the lines the window outgrew must be gone, got {text:?}",
        );
    }

    #[test]
    fn a_short_pane_keeps_the_notice_and_drops_the_oldest_window_rows() {
        // The overlay's clamp takes rows off the end of the list, so a window
        // built oldest-first with the notice on top loses its newest lines
        // exactly when it has fewest to spare. Sizing the window before the
        // clamp is what puts the loss at the other end.
        let now = t0();
        let mut ui = pushing(now);
        for step in 0..MAX_PUSH_OUTPUT_ROWS {
            ui.output_line(format!("line {step}"));
        }

        // Four rows, one of which the frame always keeps.
        let dims = Dimensions {
            width: 80,
            height: 4,
        };
        let text = painted(&mut ui, dims, now);
        assert_eq!(
            text.lines().count(),
            3,
            "the overlay gets every row but the frame's, got {text:?}",
        );
        assert!(
            text.contains(RUNNING_NOTICE),
            "the row that says a push is running must survive, got {text:?}",
        );
        assert!(
            text.contains("line 5"),
            "the newest line must survive, got {text:?}",
        );
        assert!(
            !text.contains("line 0"),
            "the oldest lines are what a short pane loses, got {text:?}",
        );
    }

    #[test]
    fn the_notice_says_how_long_the_push_has_been_running() {
        // A hook that takes minutes is the case this feature exists for, and a
        // notice that never changes is indistinguishable from a hang.
        let now = t0();
        let mut ui = pushing(now);

        let text = painted(&mut ui, tall_pane(80), now + Duration::from_secs(72));
        assert!(
            text.contains("1m12s"),
            "the notice must report its own age, got {text:?}",
        );
    }

    #[test]
    fn a_running_push_keeps_the_loop_waking() {
        // The age above only advances on a frame that is drawn, and a hook
        // that is quiet for a minute gives the loop no other reason to draw
        // one.
        let ui = pushing(t0());
        assert_eq!(ui.next_tick(), Some(STATUS_CADENCE));
    }

    #[test]
    fn a_window_row_is_truncated_to_the_pane_width() {
        // gsw's standing rule: nothing it paints wraps the pane it was
        // measured to fill. A hook's line is the one text here nobody chose
        // the length of.
        let now = t0();
        let mut ui = pushing(now);
        ui.output_line("x".repeat(200));

        let overlay = testcolor::with_forced_ansi(|| ui.overlay(tall_pane(40), now).text());
        for line in overlay.lines() {
            assert!(
                visible_width(line) <= 40,
                "a row is {} columns wide in a 40-column pane: {line:?}",
                visible_width(line),
            );
        }
    }

    #[test]
    fn a_line_reported_when_no_push_is_running_changes_nothing() {
        let mut ui = PushUi::new(false);
        ui.output_line("stray".to_string());

        assert_eq!(ui.overlay(tall_pane(80), t0()).rows(), 0);
        assert_eq!(ui.mode(), InputMode::Normal);
    }

    #[test]
    fn the_window_closes_when_the_push_finishes() {
        // The outcome replaces the window. Leaving the hook's last rows under
        // a success message would spend the frame's rows on news twice over.
        let now = t0();
        let mut ui = pushing(now);
        ui.output_line("Compiling gsw v0.1.0".to_string());

        ui.finished(
            PushOutcome {
                success: true,
                output: String::new(),
            },
            now,
        );

        let text = painted(&mut ui, tall_pane(80), now);
        assert!(
            !text.contains("Compiling gsw v0.1.0"),
            "the window must close with the push, got {text:?}",
        );
    }

    #[test]
    fn a_fresh_ui_shows_nothing_and_leaves_the_keys_alone() {
        let mut ui = PushUi::new(false);
        assert_eq!(ui.mode(), InputMode::Normal);
        assert_eq!(ui.overlay(tall_pane(80), t0()).rows(), 0);
        assert_eq!(ui.overlay(tall_pane(80), t0()).text(), "");
    }

    #[test]
    fn requesting_a_push_asks_the_question_and_takes_the_keys() {
        // `p` on a pushable branch must put the question on screen AND switch
        // the key table, or `y` would be read as an ordinary key.
        let mut ui = asking();
        assert_eq!(ui.mode(), InputMode::Confirm);
        assert_eq!(ui.overlay(tall_pane(80), t0()).rows(), 1);
        let overlay = ui.overlay(tall_pane(80), t0()).text();
        assert!(
            overlay.contains("Create new remote branch origin/gsw-push?"),
            "the question must be on screen, got {overlay:?}",
        );
        // Spelled out rather than the usual `[y/N]`, because a capital `N` is
        // the convention for "Enter means no" and Enter confirms here. A hint
        // that lies about the riskiest key on the prompt is worse than a long
        // one.
        assert!(
            overlay.contains(&confirm_hint(PUSH_VERB)),
            "the overlay owns the key hint, got {overlay:?}",
        );
    }

    #[test]
    fn requesting_a_base_update_asks_the_question_and_takes_the_keys() {
        // `R` on a branch behind the base must put the question on screen AND
        // switch the key table, or `y` would be read as an ordinary key.
        let mut ui = asking_base_update(BaseUpdate::Rebase, t0());
        assert_eq!(ui.mode(), InputMode::Confirm);
        let overlay = painted(&mut ui, tall_pane(120), t0());
        assert!(
            overlay.contains("Rebase gsw-push onto main (5 commits behind), then push with grp?"),
            "the question must be on screen, got {overlay:?}",
        );
        assert!(
            overlay.contains(&confirm_hint("rebase")),
            "the keys that answer it go with it, got {overlay:?}",
        );
    }

    #[test]
    fn requesting_a_base_update_that_is_refused_explains_instead_of_asking() {
        // The refusal is a message, not a question: the keys must stay normal,
        // so `y` does not answer a prompt that is not there.
        let mut ui = PushUi::new(false);
        ui.request_base_update(
            &snapshot(None),
            BaseUpdate::Rebase,
            &base_update_command(BaseUpdate::Rebase),
            tall_pane(80),
            t0(),
        );
        assert_eq!(ui.mode(), InputMode::Normal);
        let overlay = painted(&mut ui, tall_pane(80), t0());
        assert!(
            overlay.contains("gsw-push already contains main"),
            "the reason must reach the row, got {overlay:?}",
        );
    }

    #[test]
    fn confirming_a_base_update_hands_back_the_command_it_described_once() {
        // The act, the branch, the base, and the command travel together, so
        // what runs on `y` is what the sentence promised — and the runner can
        // still tell whether the repository moved under it. The second `y` is
        // one that raced the mode change, and it must start nothing.
        let mut ui = asking_base_update(BaseUpdate::Rebase, t0());
        let Some(Confirmed::BaseUpdate(command)) = ui.confirm(t0()) else {
            panic!("a question about a rebase must confirm a rebase");
        };
        assert_eq!(command.update(), BaseUpdate::Rebase);
        assert_eq!(command.branch(), "gsw-push");
        assert_eq!(command.base(), "main");
        assert_eq!(command.command().name(), "grp");
        assert_eq!(ui.mode(), InputMode::Pushing);
        assert_eq!(
            ui.confirm(t0()),
            None,
            "a second y must not start a second run",
        );
    }

    #[test]
    fn a_running_base_update_takes_its_notice_from_the_question_and_counts_the_time() {
        // The run carries no deadline, because a pre-push hook of this
        // workspace builds and tests every crate in it and takes minutes. So
        // the notice names the act that is running and counts the time, and a
        // run that hangs shows on the screen as a run that hangs. `Pushing…`
        // here would name neither the act nor the command.
        let now = t0();
        let mut ui = asking_base_update(BaseUpdate::Rebase, now);
        ui.confirm(now).expect("the question must confirm");
        assert_eq!(
            painted(&mut ui, tall_pane(120), now + Duration::from_secs(72)),
            "Rebasing gsw-push onto main with grp… (1m12s)",
        );
    }

    #[test]
    fn a_running_merge_says_that_it_is_merging() {
        let now = t0();
        let mut ui = asking_base_update(BaseUpdate::Merge, now);
        ui.confirm(now).expect("the question must confirm");
        assert_eq!(
            painted(&mut ui, tall_pane(120), now + Duration::from_secs(4)),
            "Merging main into gsw-push with gmp… (4s)",
        );
    }

    #[test]
    fn the_rebase_question_wears_the_color_of_caution_and_the_merge_question_does_not() {
        // A rebase rewrites every commit of the branch and `grp` force-pushes
        // the result, so a branch that somebody else has pulled is a branch
        // they must repair. That is the question the color of the create is
        // for. A merge writes one commit and pushes it, which is the routine
        // act the count in the header is about.
        let now = t0();
        let rebase = escapes(&mut asking_base_update(BaseUpdate::Rebase, now), now);
        let glyphs = testcolor::strip_ansi(&rebase);
        assert_eq!(
            rebase,
            testcolor::with_forced_ansi(|| glyphs.yellow().to_string()),
            "the rebase question must be drawn in the color of the question that \
             creates a remote branch",
        );

        let merge = escapes(&mut asking_base_update(BaseUpdate::Merge, now), now);
        assert_eq!(
            merge,
            testcolor::strip_ansi(&merge),
            "the merge question is routine, so it takes the color of the row",
        );
    }

    #[test]
    fn a_pane_with_no_row_to_spare_raises_no_base_update_question() {
        // The rule of the row, which both doors into it obey: a question the
        // user cannot see must not be answerable, or Enter pressed out of
        // reflex at an unchanged frame force-pushes a rewritten branch. The
        // same rule holds `p`, and it is written once.
        let mut ui = asking_base_update_in(
            BaseUpdate::Rebase,
            Dimensions {
                width: 80,
                height: 1,
            },
            t0(),
        );
        assert_eq!(
            ui.mode(),
            InputMode::Normal,
            "the keys must stay ordinary where no question was shown",
        );
        assert_eq!(
            ui.confirm(t0()),
            None,
            "there must be nothing for a y to answer",
        );
    }

    #[test]
    fn requesting_a_push_with_nothing_to_push_explains_instead_of_asking() {
        // The refusal is a message, not a question: the keys must stay normal,
        // so `y` does not answer a prompt that is not there.
        let mut ui = PushUi::new(false);
        ui.request(&snapshot(tracked(0)), tall_pane(80), t0());
        assert_eq!(ui.mode(), InputMode::Normal);
        assert_eq!(ui.overlay(tall_pane(80), t0()).rows(), 1);
        assert!(ui
            .overlay(tall_pane(80), t0())
            .text()
            .contains("origin/gsw-push is already up to date"));
        assert!(
            !ui.overlay(tall_pane(80), t0())
                .text()
                .contains(&confirm_hint(PUSH_VERB)),
            "a refusal must not offer keys that do nothing",
        );
    }

    #[test]
    fn confirming_hands_back_the_command_and_switches_to_pushing() {
        let mut ui = asking();
        let Some(Confirmed::Push(command)) = ui.confirm(t0()) else {
            panic!("a question about a push must confirm a push");
        };
        assert_eq!(command.args(), ["push", "-u", "origin", "gsw-push"]);
        assert_eq!(
            command.branch(),
            "gsw-push",
            "the branch the question named must reach the runner",
        );
        assert_eq!(ui.mode(), InputMode::Pushing);
        assert_eq!(
            ui.overlay(tall_pane(80), t0()).rows(),
            1,
            "the running push stays on screen"
        );
    }

    #[test]
    fn confirming_with_no_question_up_runs_nothing() {
        // Belt and braces against a stray PushConfirmed: with no confirmation
        // on screen there is no command to run, and inventing one would push
        // without asking.
        let mut ui = PushUi::new(false);
        assert_eq!(ui.confirm(t0()), None);
        assert_eq!(ui.mode(), InputMode::Normal);
    }

    #[test]
    fn confirming_twice_runs_the_push_once() {
        // The second `y` arrives after the mode has already moved to Pushing.
        // It must not produce a second command.
        let mut ui = asking();
        assert!(ui.confirm(t0()).is_some());
        assert_eq!(
            ui.confirm(t0()),
            None,
            "a second confirm must not push again"
        );
    }

    #[test]
    fn cancelling_clears_the_question_without_a_notice() {
        let mut ui = asking();
        ui.cancel();
        assert_eq!(ui.mode(), InputMode::Normal);
        assert_eq!(
            ui.overlay(tall_pane(80), t0()).rows(),
            0,
            "a cancelled prompt leaves nothing behind"
        );
    }

    #[test]
    fn a_successful_push_reports_what_it_did() {
        // The wording comes from the plan, so a create reports itself as a
        // create rather than as a generic success.
        let mut ui = asking();
        ui.confirm(t0());
        ui.finished(
            PushOutcome {
                success: true,
                output: "To /tmp/origin\n * [new branch] gsw-push -> gsw-push\n".to_string(),
            },
            t0(),
        );
        assert_eq!(ui.mode(), InputMode::Normal);
        assert!(ui
            .overlay(tall_pane(80), t0())
            .text()
            .contains("Created origin/gsw-push"));
    }

    #[test]
    fn a_successful_update_counts_what_it_pushed() {
        let mut ui = PushUi::new(false);
        ui.request(&snapshot(tracked(3)), tall_pane(80), t0());
        ui.confirm(t0());
        ui.finished(
            PushOutcome {
                success: true,
                output: String::new(),
            },
            t0(),
        );
        assert!(ui
            .overlay(tall_pane(80), t0())
            .text()
            .contains("Pushed 3 commits to origin/gsw-push"));
    }

    #[test]
    fn a_failed_push_shows_what_git_said() {
        // The whole point of the feature's error path: git's own words, not a
        // gsw paraphrase.
        let mut ui = asking();
        ui.confirm(t0());
        ui.finished(
            PushOutcome {
                success: false,
                output: "To /tmp/origin\n ! [rejected] gsw-push -> gsw-push (fetch first)\n\
                     error: failed to push some refs to '/tmp/origin'\n"
                    .to_string(),
            },
            t0(),
        );
        assert_eq!(ui.mode(), InputMode::Normal);
        let overlay = ui.overlay(tall_pane(120), t0()).text();
        assert!(overlay.contains("! [rejected]"), "got {overlay:?}");
        assert!(
            overlay.contains("error: failed to push some refs"),
            "got {overlay:?}"
        );
    }

    #[test]
    fn a_failed_push_drops_the_hints_before_the_error() {
        // git follows a rejection with several `hint:` lines. They must not
        // crowd out the error itself when only three rows are free.
        let mut ui = asking();
        ui.confirm(t0());
        ui.finished(
            PushOutcome {
                success: false,
                output: "To /tmp/origin\n\
                     hint: Updates were rejected because the tip is behind\n\
                     hint: its remote counterpart. Integrate the changes\n\
                     hint: before pushing again.\n\
                     ! [rejected] gsw-push -> gsw-push (fetch first)\n\
                     error: failed to push some refs\n"
                    .to_string(),
            },
            t0(),
        );
        let overlay = ui.overlay(tall_pane(120), t0()).text();
        assert!(
            !overlay.contains("hint:"),
            "hints must not survive, got {overlay:?}"
        );
        assert!(overlay.contains("! [rejected]"), "got {overlay:?}");
        assert!(
            overlay.contains("error: failed to push some refs"),
            "got {overlay:?}"
        );
    }

    #[test]
    fn a_failed_push_shows_the_last_lines_rather_than_the_first() {
        // A pre-push hook that runs a test suite prints its whole run before
        // it fails, and git adds its own verdict after that. The reason is
        // therefore at the end, and three rows spent on the hook's opening
        // banner say nothing at all — which is what the head rule gave every
        // repository that has a hook.
        //
        // A plain rejection is unaffected: git writes exactly three non-hint
        // lines there, so the head and the tail are the same three.
        let mut ui = asking();
        ui.confirm(t0());
        ui.finished(
            PushOutcome {
                success: false,
                output: "Running clippy\n\
                     Compiling gsw v0.1.0\n\
                     Compiling repo-guards v0.1.0\n\
                     test push::window ... FAILED\n\
                     error: test failed\n\
                     error: failed to push some refs\n"
                    .to_string(),
            },
            t0(),
        );

        let overlay = ui.overlay(tall_pane(120), t0()).text();
        assert!(
            overlay.contains("error: failed to push some refs"),
            "git's own verdict is the last line and must survive, got {overlay:?}",
        );
        assert!(
            overlay.contains("test push::window ... FAILED"),
            "the failing test is what names the reason, got {overlay:?}",
        );
        assert!(
            !overlay.contains("Compiling"),
            "the opening banner is what the rows come from, got {overlay:?}",
        );
    }

    #[test]
    fn a_failed_push_never_takes_more_than_three_rows() {
        // The frame below is what the user is watching. A wall of git output
        // must not push it off the screen.
        let mut ui = asking();
        ui.confirm(t0());
        let output = (1..=20)
            .map(|n| format!("error: line {n}\n"))
            .collect::<String>();
        ui.finished(
            PushOutcome {
                success: false,
                output,
            },
            t0(),
        );
        assert_eq!(ui.overlay(tall_pane(80), t0()).rows(), MAX_STATUS_ROWS);
        assert_eq!(
            ui.overlay(tall_pane(80), t0()).text().lines().count(),
            MAX_STATUS_ROWS
        );
    }

    #[test]
    fn a_message_taller_than_the_pane_keeps_the_rows_the_frame_can_spare() {
        // Three rows of error in a three-row pane: the frame is laid out to
        // fill the pane exactly, so every row the overlay takes is a row the
        // frame gave up, and the last one is not the frame's to give.
        let mut ui = asking();
        ui.confirm(t0());
        ui.finished(
            PushOutcome {
                success: false,
                output: "To /tmp/origin\n\
                     ! [rejected] gsw-push -> gsw-push (fetch first)\n\
                     error: failed to push some refs\n"
                    .to_string(),
            },
            t0(),
        );
        let overlay = ui.overlay(
            Dimensions {
                width: 80,
                height: 3,
            },
            t0(),
        );
        assert_eq!(overlay.rows(), 2, "the frame keeps the third row");
        // The reason a push failed is the last thing said about it, so the
        // head is what goes. `To /tmp/origin` names a remote the frame above
        // already shows, and it is the line the clip can most afford to lose.
        let text = overlay.text();
        assert!(
            text.contains("error: failed to push some refs"),
            "the verdict must survive the clip, got {text:?}",
        );
        assert!(
            !text.contains("To /tmp/origin"),
            "the first line is the one to drop, got {text:?}",
        );
    }

    #[test]
    fn a_one_row_pane_is_all_frame() {
        // Nothing is left to overlay onto, and a pane showing only a question
        // with no frame under it is not watch mode.
        let mut ui = asking();
        let overlay = ui.overlay(
            Dimensions {
                width: 80,
                height: 1,
            },
            t0(),
        );
        assert_eq!(overlay.rows(), 0);
        assert_eq!(overlay.text(), "");
    }

    #[test]
    fn a_question_the_pane_cannot_show_cannot_be_answered() {
        // What the user sees in a one-row pane after pressing `p` is a frame
        // that did not change, because there is no row left to draw the
        // question in. If the keys still meant "push", the Enter they press out
        // of reflex would push to a shared remote having asked nothing. So the
        // question goes when its row does, and the keys go with it.
        let mut ui = asking();
        assert_eq!(ui.mode(), InputMode::Confirm, "the question was raised");
        let overlay = ui.overlay(
            Dimensions {
                width: 80,
                height: 1,
            },
            t0(),
        );
        assert_eq!(overlay.text(), "", "the pane had no row to ask in");
        assert_eq!(
            ui.mode(),
            InputMode::Normal,
            "a question that was never drawn must not leave the keys meaning push",
        );
        assert_eq!(
            ui.confirm(t0()),
            None,
            "Enter must not start a push nobody was asked about",
        );
    }

    #[test]
    fn a_pane_with_no_room_for_the_question_is_never_asked_it() {
        // The cancel above runs at render time, and there is no render between
        // the `p` and the `y` of one burst of keys. So the pane has to be
        // consulted where the question is raised: a `p` in a pane with no row
        // to spare leaves the keys alone, and there is no question for a later
        // key to answer.
        let mut ui = PushUi::new(false);
        ui.request(
            &snapshot(None),
            Dimensions {
                width: 80,
                height: 1,
            },
            t0(),
        );
        assert_eq!(
            ui.mode(),
            InputMode::Normal,
            "a question the pane cannot hold must not switch the key table",
        );
        assert_eq!(
            ui.confirm(t0()),
            None,
            "there must be no question waiting for a `y` that never saw one",
        );
    }

    #[test]
    fn the_row_count_always_matches_the_text() {
        // The whole reason these are one call: a count that disagrees with the
        // text either leaves a blank strip under the frame or paints past the
        // bottom of the pane.
        let mut ui = asking();
        ui.confirm(t0());
        ui.finished(
            PushOutcome {
                success: false,
                output: (1..=20)
                    .map(|n| format!("error: line {n}\n"))
                    .collect::<String>(),
            },
            t0(),
        );
        for height in 0..8 {
            let overlay = ui.overlay(Dimensions { width: 80, height }, t0());
            let painted = if overlay.text().is_empty() {
                0
            } else {
                overlay.text().lines().count()
            };
            assert_eq!(painted, overlay.rows(), "disagreed in a {height}-row pane");
            assert!(
                overlay.rows() < height.max(1),
                "the frame lost its last row in a {height}-row pane",
            );
        }
    }

    /// A rejection as git writes one: several lines, more than a short pane can
    /// hold. The status states are only worth sweeping against a message that
    /// wants more rows than it can have.
    const REJECTION: &str = "To /tmp/origin\n\
                             ! [rejected] gsw-push -> gsw-push (fetch first)\n\
                             error: failed to push some refs to '/tmp/origin'\n";

    /// Wide enough that no line is truncated, so the sweep below measures rows
    /// and nothing else.
    const SWEEP_WIDTH: usize = 80;

    /// The tallest pane the sweep tries. Comfortably past [`MAX_STATUS_ROWS`],
    /// so the sweep covers panes that are short, panes that are exactly full,
    /// and panes with room to spare.
    const SWEEP_MAX_HEIGHT: usize = 8;

    /// A UI reporting the outcome of a push it asked about and ran.
    fn reporting(outcome: PushOutcome) -> PushUi {
        let mut ui = asking();
        ui.confirm(t0());
        ui.finished(outcome, t0());
        ui
    }

    /// One `PushUi` in each state the push feature can hold, plus the two ways
    /// it comes back to rest, each with a name the sweep can report.
    ///
    /// Built through `request`, `confirm`, `finished`, `cancel` and `dismiss` —
    /// the same calls the watch loop makes — rather than by assembling a
    /// `State` directly. A state built by hand could be one the loop can never
    /// reach, and an invariant that holds only for unreachable states holds
    /// nothing.
    fn every_state() -> Vec<(&'static str, PushUi)> {
        let cancelled = {
            let mut ui = asking();
            ui.cancel();
            ui
        };
        let dismissed = {
            let mut ui = reporting(PushOutcome {
                success: false,
                output: REJECTION.to_string(),
            });
            ui.dismiss();
            ui
        };
        let refusing = {
            let mut ui = PushUi::new(false);
            ui.request(&snapshot(tracked(0)), tall_pane(80), t0());
            ui
        };
        let asking_to_update = {
            let mut ui = PushUi::new(false);
            ui.request(&snapshot(tracked(3)), tall_pane(80), t0());
            ui
        };
        let running = {
            let mut ui = asking();
            ui.confirm(t0());
            ui
        };
        let listing = {
            let mut ui = PushUi::new(false);
            ui.open_list(three_worktrees());
            ui
        };
        vec![
            ("idle", PushUi::new(false)),
            ("asking to create a remote branch", asking()),
            ("asking to update a remote branch", asking_to_update),
            ("running a push", running),
            (
                "reporting a successful push",
                reporting(PushOutcome {
                    success: true,
                    output: String::new(),
                }),
            ),
            (
                "reporting a failed push",
                reporting(PushOutcome {
                    success: false,
                    output: REJECTION.to_string(),
                }),
            ),
            ("refusing a branch with nothing to push", refusing),
            ("a cancelled question", cancelled),
            ("a dismissed status", dismissed),
            ("an open list of the worktrees", listing),
        ]
    }

    #[test]
    fn the_row_split_holds_in_every_state_and_every_small_pane() {
        // This arithmetic has now produced two separate review findings — an
        // overlay that took more rows than the pane had, and a question that
        // vanished on a one-row pane while the keys still meant "push". Both
        // hid in a state and a pane size nobody spot-checked. So every state
        // the feature can reach is swept against every pane from zero rows to
        // eight, and all four rules are checked on each pair, rather than one
        // rule being sampled at one size.
        //
        // Every broken pair is collected and reported together. A guard for a
        // recurring class of defect must say how far the damage goes, not stop
        // at the first pair and hide the rest behind a fix.
        let mut broken: Vec<String> = Vec::new();

        // Every pane gets its own freshly built states, rather than one
        // `PushUi` being carried across the whole range of heights. `overlay`
        // can change the state it was asked about — a question the pane cannot
        // draw is cancelled there — so a carried instance would be idle by the
        // second pane, and each state would be swept exactly once instead of
        // nine times.
        for height in 0..=SWEEP_MAX_HEIGHT {
            for (name, mut ui) in every_state() {
                let overlay = ui.overlay(
                    Dimensions {
                        width: SWEEP_WIDTH,
                        height,
                    },
                    t0(),
                );

                // The count and the body must be the same rows. A count larger
                // than the text leaves a blank strip under the frame; a count
                // smaller than it paints past the bottom of the pane.
                let text = overlay.text();
                let painted = if text.is_empty() {
                    0
                } else {
                    text.lines().count()
                };
                if painted != overlay.rows() {
                    broken.push(format!(
                        "{name} in a {height}-row pane: rows() says {} but the text has \
                         {painted} lines",
                        overlay.rows(),
                    ));
                }

                // The frame never loses its last row. A pane holding only a
                // message says nothing about which repository it belongs to,
                // and the repository is what watch mode is for.
                if overlay.frame_rows() == 0 {
                    broken.push(format!(
                        "{name} in a {height}-row pane: the frame was left no rows at all",
                    ));
                }

                // The two together must fit. The frame fills exactly the height
                // it is given, so one row too many scrolls the alternate screen
                // — the failure the last review found.
                if overlay.rows() + overlay.frame_rows() > height.max(1) {
                    broken.push(format!(
                        "{name} in a {height}-row pane: {} overlay rows and {} frame rows \
                         overflow it",
                        overlay.rows(),
                        overlay.frame_rows(),
                    ));
                }

                // Whenever the keys mean "push", the question has to be on
                // screen. A confirmation the user cannot read is a push they
                // did not agree to.
                if ui.mode() == InputMode::Confirm && overlay.rows() == 0 {
                    broken.push(format!(
                        "{name} in a {height}-row pane: the keys mean push, but the question \
                         is not on screen",
                    ));
                }
            }
        }

        assert!(
            broken.is_empty(),
            "{} state/pane pairs broke the row split:\n{}",
            broken.len(),
            broken.join("\n"),
        );
    }

    #[test]
    fn a_failed_push_that_said_nothing_still_says_something() {
        // A push that fails with no output at all must not leave a blank row
        // that reads as success.
        let mut ui = asking();
        ui.confirm(t0());
        ui.finished(
            PushOutcome {
                success: false,
                output: "   \n\n".to_string(),
            },
            t0(),
        );
        assert_eq!(ui.overlay(tall_pane(80), t0()).rows(), 1);
        assert!(
            !ui.overlay(tall_pane(80), t0()).text().trim().is_empty(),
            "a failure must always say that it failed",
        );
    }

    #[test]
    fn a_status_stays_until_a_key_arrives() {
        // Tim's requirement: an error must survive every decay tick and
        // repaint, and go away only when the user has pressed something.
        let mut ui = asking();
        ui.confirm(t0());
        ui.finished(
            PushOutcome {
                success: false,
                output: "error: failed to push some refs\n".to_string(),
            },
            t0(),
        );
        assert_eq!(ui.overlay(tall_pane(80), t0()).rows(), 1);
        ui.dismiss();
        assert_eq!(
            ui.overlay(tall_pane(80), t0()).rows(),
            0,
            "a key press clears the message"
        );
        assert_eq!(ui.overlay(tall_pane(80), t0()).text(), "");
    }

    #[test]
    fn dismissing_leaves_a_question_and_a_running_push_alone() {
        // `dismiss` is what an unrelated key does. It must not answer a
        // question or hide a push that is still running.
        let mut ui = asking();
        ui.dismiss();
        assert_eq!(ui.mode(), InputMode::Confirm, "a stray key must not cancel");

        ui.confirm(t0());
        ui.dismiss();
        assert_eq!(
            ui.mode(),
            InputMode::Pushing,
            "a stray key must not hide a running push",
        );
    }

    #[test]
    fn a_running_push_says_so() {
        let mut ui = asking();
        ui.confirm(t0());
        let overlay = ui.overlay(tall_pane(80), t0()).text();
        assert!(
            overlay.to_lowercase().contains("push"),
            "the running notice must name what is happening, got {overlay:?}",
        );
    }

    #[test]
    fn the_overlay_never_exceeds_the_width_it_is_given() {
        // gsw's standing contract: nothing it prints wraps. A long remote name
        // or a long git error must be truncated, not folded onto a new row.
        let mut ui = PushUi::new(false);
        let mut snap = snapshot(None);
        snap.branch = "a-branch-name-long-enough-to-need-truncating-on-a-narrow-pane".to_string();
        ui.request(&snap, tall_pane(80), t0());
        for width in [10, 20, 40] {
            let overlay = ui.overlay(tall_pane(width), t0()).text();
            for line in overlay.lines() {
                assert!(
                    visible_width(line) <= width,
                    "line {line:?} exceeds width {width}",
                );
            }
        }
    }

    #[test]
    fn the_overlay_truncates_multibyte_text_without_panicking() {
        // Branch names can hold multi-byte characters, and byte-slicing one at
        // a narrow width is a panic in the middle of the alternate screen.
        let mut ui = PushUi::new(false);
        let mut snap = snapshot(None);
        snap.branch = "日本語のブランチ名-🎉-café".to_string();
        ui.request(&snap, tall_pane(80), t0());
        for width in 1..40 {
            let overlay = ui.overlay(tall_pane(width), t0()).text();
            for line in overlay.lines() {
                assert!(
                    visible_width(line) <= width,
                    "line {line:?} exceeds width {width}",
                );
            }
        }
    }

    #[test]
    fn a_new_request_replaces_a_stale_status() {
        // Pressing `p` while an old error is on screen must ask the new
        // question, not stack a second row under the first.
        let mut ui = asking();
        ui.confirm(t0());
        ui.finished(
            PushOutcome {
                success: false,
                output: "error: failed to push some refs\n".to_string(),
            },
            t0(),
        );
        ui.request(&snapshot(None), tall_pane(80), t0());
        assert_eq!(ui.mode(), InputMode::Confirm);
        assert_eq!(ui.overlay(tall_pane(80), t0()).rows(), 1);
        assert!(!ui.overlay(tall_pane(120), t0()).text().contains("error:"));
    }

    /// A UI holding the message a finished update push leaves behind, posted at
    /// `at`. The state every test below about ageing starts from.
    fn pushed_at(at: Instant) -> PushUi {
        pushed_with(false, at)
    }

    /// The same message, on a UI built for a terminal whose color depth is
    /// `truecolor`.
    ///
    /// The color depth is a parameter because it is the value under test. A
    /// `PushUi` keeps the depth [`PushUi::new`] receives, and gives it to the
    /// fade at the one point in [`PushUi::overlay`] that colors a row. A test
    /// that proves the depth arrives there must therefore choose it here: this
    /// function is the only route from the test suite to `PushUi::new(true)`.
    /// [`pushed_at`] keeps the 8-color default, which is what every other test
    /// about ageing reads.
    fn pushed_with(truecolor: bool, at: Instant) -> PushUi {
        let mut ui = PushUi::new(truecolor);
        ui.request(&snapshot(tracked(3)), tall_pane(80), at);
        ui.confirm(at);
        ui.finished(
            PushOutcome {
                success: true,
                output: String::new(),
            },
            at,
        );
        ui
    }

    #[test]
    fn a_successful_push_says_how_long_ago_it_happened() {
        // A monitor's rows all say when. "Pushed 3 commits" on its own stops
        // being news the moment the user looks away, and nothing on screen
        // tells them whether they are reading something from five seconds ago
        // or from before lunch.
        let start = t0();
        let mut ui = pushed_at(start);

        let fresh = ui.overlay(tall_pane(80), start).text();
        assert!(
            fresh.contains("Pushed 3 commits to origin/gsw-push"),
            "the message itself must survive the age being added, got {fresh:?}",
        );
        assert!(
            fresh.contains("(0s ago)"),
            "a message just posted must say so, got {fresh:?}",
        );

        let later = ui
            .overlay(tall_pane(80), start + Duration::from_secs(5))
            .text();
        assert!(
            later.contains("Pushed 3 commits to origin/gsw-push"),
            "got {later:?}",
        );
        assert!(
            later.contains("(5s ago)"),
            "the age must advance with the clock, got {later:?}",
        );
    }

    #[test]
    fn a_successful_push_takes_itself_off_the_screen() {
        // The complaint this feature answers: the message stayed until a key
        // was pressed, which on a monitor nobody is typing at means forever.
        let start = t0();
        let mut ui = pushed_at(start);

        assert_eq!(
            ui.overlay(tall_pane(80), start + STATUS_LIFETIME - STATUS_CADENCE)
                .rows(),
            1,
            "the message must last its whole lifetime",
        );

        let expired = ui.overlay(tall_pane(80), start + STATUS_LIFETIME);
        assert_eq!(expired.rows(), 0, "the message must remove itself");
        assert_eq!(expired.text(), "");
        assert_eq!(
            expired.frame_rows(),
            tall_pane(80).height,
            "the row it was using must go back to the frame",
        );
        assert_eq!(ui.mode(), InputMode::Normal);
    }

    #[test]
    fn a_refused_push_also_says_when_and_also_goes_away() {
        // A refusal describes the repository as it stood when `p` was pressed,
        // so it goes stale exactly the way a success does — and it costs the
        // frame the same row until it does.
        let start = t0();
        let mut ui = PushUi::new(false);
        ui.request(&snapshot(tracked(0)), tall_pane(80), start);

        let text = ui
            .overlay(tall_pane(80), start + Duration::from_secs(7))
            .text();
        assert!(
            text.contains("origin/gsw-push is already up to date"),
            "got {text:?}",
        );
        assert!(text.contains("(7s ago)"), "got {text:?}");

        assert_eq!(
            ui.overlay(tall_pane(80), start + STATUS_LIFETIME).rows(),
            0,
            "a refusal must expire like any other message gsw wrote",
        );
    }

    #[test]
    fn a_failed_push_stays_however_long_it_takes() {
        // The one message gsw must not remove by itself. git's error text is
        // what the user has to read and act on, and a remedy that expires
        // while they are looking at another pane is worse than a row spent.
        let start = t0();
        let mut ui = asking();
        ui.confirm(t0());
        ui.finished(
            PushOutcome {
                success: false,
                output: "error: failed to push some refs\n".to_string(),
            },
            start,
        );

        let hours_later = ui.overlay(tall_pane(80), start + Duration::from_secs(3 * 60 * 60));
        assert_eq!(hours_later.rows(), 1, "an error must not expire");
        assert!(
            hours_later
                .text()
                .contains("error: failed to push some refs"),
            "got {:?}",
            hours_later.text(),
        );
        assert!(
            !hours_later.text().contains("ago"),
            "an error that never expires has no countdown to report, got {:?}",
            hours_later.text(),
        );

        ui.dismiss();
        assert_eq!(
            ui.overlay(tall_pane(80), start).rows(),
            0,
            "a key press is still what clears it",
        );
    }

    #[test]
    fn an_ageing_message_darkens_all_the_way_to_black() {
        // The fade is the other half of the age text: a message on its way out
        // should look like it, and be gone rather than dark by the end.
        //
        // Asserted on the typed color rather than on the escape bytes, because
        // whether `colored` emits any is process-global state other tests in
        // this binary toggle.
        let brightness = |elapsed: Duration| match colorize_status(TEXT, elapsed, true).fgcolor {
            Some(colored::Color::TrueColor { r, g, b }) => {
                u32::from(r) + u32::from(g) + u32::from(b)
            }
            other => panic!("a fading row must carry a truecolor foreground, got {other:?}"),
        };

        assert!(
            brightness(Duration::ZERO) > 0,
            "a message just posted is drawn at full brightness",
        );
        assert_eq!(
            brightness(STATUS_LIFETIME),
            0,
            "the fade must reach black exactly as the message is removed",
        );

        // Monotone the whole way down, so no repaint ever brightens a message
        // that is on its way out.
        let mut previous = brightness(Duration::ZERO);
        for second in 1..=STATUS_LIFETIME.as_secs() {
            let now = brightness(Duration::from_secs(second));
            assert!(
                now <= previous,
                "the fade brightened at {second}s: {previous} then {now}",
            );
            previous = now;
        }
    }

    #[test]
    fn an_ageing_message_dims_once_where_there_is_no_truecolor() {
        // The 8-color fallback the commit-log gradient already has. Two steps
        // is all there is to spend, so the message is drawn plain while the
        // news is current and dim for the rest of its life.
        use colored::Styles;
        let fresh = colorize_status(TEXT, Duration::ZERO, false);
        assert!(
            !fresh.style.contains(Styles::Dimmed) && fresh.fgcolor.is_none(),
            "a message just posted is drawn exactly as it was before",
        );
        assert!(
            colorize_status(TEXT, STATUS_LIFETIME - STATUS_CADENCE, false)
                .style
                .contains(Styles::Dimmed),
            "an old message must be dimmed even with no gradient to fade along",
        );
    }

    /// Stand-in row for the styling tests, which are about the color a status
    /// row is drawn in and not about what it says.
    const TEXT: &str = "Pushed 3 commits to origin/gsw-push (5s ago)";

    #[test]
    fn the_status_message_fades_in_24_bit_color_only_where_the_terminal_takes_it() {
        // The two tests above read a typed `ColoredString` from
        // `colorize_status`, and the comment on the first one says why: the
        // `colored` crate decides from process-global state whether it writes
        // escape bytes at all. That reason holds for those tests, which supply
        // the color depth themselves. It does not hold for this one. Here the
        // color depth is the subject — the question is whether the value
        // `PushUi::new` received arrives at the one call in `PushUi::overlay`
        // that colors a row — and the painted bytes are the only place that
        // answer appears. `testcolor::with_forced_ansi` makes those bytes
        // stable: it holds the one lock on that global state for both halves of
        // the comparison.
        //
        // Both halves are necessary. The first half alone passes if any other
        // part of the row writes a 24-bit color. The second half is what shows
        // that the color depth is the thing that decides.
        //
        // The age is half of `STATUS_LIFETIME` for two reasons. The message is
        // still on screen at that age, well before it expires and leaves an
        // empty overlay that satisfies the second half for the wrong reason.
        // And the age is at `COARSE_FADE_AT`, so the 8-color half paints a dim
        // row and writes an escape sequence of its own. The second half
        // therefore proves that the row carries no 24-bit color, not merely
        // that it carries no escapes.
        let start = t0();
        let age = STATUS_LIFETIME / 2;

        let (deep, coarse) = testcolor::with_forced_ansi(|| {
            let deep = pushed_with(true, start)
                .overlay(tall_pane(80), start + age)
                .text();
            let coarse = pushed_with(false, start)
                .overlay(tall_pane(80), start + age)
                .text();
            (deep, coarse)
        });

        assert!(
            deep.contains(TRUECOLOR_FG),
            "a truecolor terminal must get the 24-bit fade, got {deep:?}",
        );
        assert!(
            !coarse.contains(TRUECOLOR_FG),
            "an 8-color terminal must get no 24-bit color, got {coarse:?}",
        );
    }

    #[test]
    fn the_status_message_is_painted_darker_the_later_the_overlay_is_drawn() {
        // The age the fade uses comes out of the UI, and `PushUi::overlay` is
        // what takes it out. The two fade tests above miss a call that passes a
        // constant age instead, because they call `colorize_status` directly
        // and hand it the age themselves. This test reads the brightness of a
        // row the overlay painted, at two ages, which is what the watch loop
        // does on every repaint.
        //
        // The exception recorded in the test above applies here for the same
        // reason: the brightness is in the escape bytes, so the row must carry
        // real ones, and `testcolor::with_forced_ansi` is what makes it do so.
        //
        // One cadence before the end of `STATUS_LIFETIME` is the oldest age at
        // which the message is still on screen, so the two ages are the largest
        // difference in brightness the overlay can show.
        let start = t0();
        let old = STATUS_LIFETIME - STATUS_CADENCE;

        let (fresh, aged) = testcolor::with_forced_ansi(|| {
            let fresh = pushed_with(true, start)
                .overlay(tall_pane(80), start)
                .text();
            let aged = pushed_with(true, start)
                .overlay(tall_pane(80), start + old)
                .text();
            (fresh, aged)
        });

        let fresh_max = max_red_channel(&fresh);
        let aged_max = max_red_channel(&aged);
        assert!(
            aged_max < fresh_max,
            "an older message must be painted darker: fresh={fresh_max} aged={aged_max}",
        );
    }

    #[test]
    fn an_ageing_message_wakes_the_loop_every_second() {
        // The loop sleeps until the soonest deadline any source imposes, and
        // with `--refresh-interval 0` on a repository whose newest commit is
        // hours old there is no other source at all. Without a deadline of its
        // own the message would age only when something else happened to
        // happen, and expire only when the user pressed a key — which is what
        // it is here to stop doing.
        let start = t0();
        let mut ui = pushed_at(start);
        ui.overlay(tall_pane(80), start);
        assert_eq!(ui.next_tick(), Some(STATUS_CADENCE));

        // And it stops asking once there is nothing left to move.
        ui.overlay(tall_pane(80), start + STATUS_LIFETIME);
        assert_eq!(
            ui.next_tick(),
            None,
            "an expired message must not go on waking the loop",
        );
    }

    #[test]
    fn nothing_that_does_not_age_asks_the_loop_to_wake() {
        // A wake-up costs a repaint of the whole pane, so only a message that
        // actually changes with the clock may ask for one.
        let start = t0();

        assert_eq!(PushUi::new(false).next_tick(), None, "an idle UI");
        assert_eq!(
            asking().next_tick(),
            None,
            "a question waiting for an answer"
        );

        // A push in flight is deliberately absent from this list. It used to
        // be here, and it belonged here while the running notice was the fixed
        // words "Pushing…": nothing about it changed with the clock. The
        // notice now reports how long the push has been running, so it does,
        // and `a_running_push_keeps_the_loop_waking` states the other half.

        let mut failed = asking();
        failed.confirm(t0());
        failed.finished(
            PushOutcome {
                success: false,
                output: "error: failed to push some refs\n".to_string(),
            },
            start,
        );
        assert_eq!(failed.next_tick(), None, "an error that never expires");
    }

    #[test]
    fn the_age_is_part_of_what_the_pane_has_to_fit() {
        // The age is appended before the row is truncated, not after, or a
        // narrow pane gets a row that wraps — and a wrapped row scrolls the
        // alternate screen gsw was measured to fill exactly.
        let start = t0();
        let mut ui = pushed_at(start);
        for width in 1..60 {
            let text = ui
                .overlay(
                    Dimensions {
                        width,
                        height: MAX_STATUS_ROWS + 10,
                    },
                    start + Duration::from_secs(42),
                )
                .text();
            for line in text.lines() {
                assert!(
                    visible_width(line) <= width,
                    "line {line:?} exceeds width {width}",
                );
            }
        }
    }

    /// What a progress notice says. The words belong to the `m` key, and the
    /// tests below are about how long the notice stays and how it looks.
    const PROGRESS: &str = "Running grind and grime against main…";

    #[test]
    fn a_progress_notice_goes_on_a_free_row_and_does_not_age() {
        // The notice says that a press of `m` does nothing now. That stays
        // true until the run ends, so the notice has no age to report and no
        // reason to wake the loop.
        let now = t0();
        let mut ui = PushUi::new(false);
        ui.post_progress(PROGRESS.to_string());

        assert_eq!(
            painted(&mut ui, tall_pane(80), now),
            PROGRESS,
            "the notice must reach the row, with no age after it",
        );
        assert_eq!(
            ui.next_tick(),
            None,
            "a notice that does not age must not wake the loop",
        );

        // A status on the row is news, so the row is free for the notice.
        let mut ui = PushUi::new(false);
        let _ = ui.post_notice(NOTICE.to_string(), now);
        ui.post_progress(PROGRESS.to_string());
        assert_eq!(
            painted(&mut ui, tall_pane(80), now),
            PROGRESS,
            "the notice must replace a status that is on the row",
        );
    }

    #[test]
    fn a_key_with_no_meaning_leaves_a_progress_notice_on_the_row() {
        // A key that took the notice away would say that the run had ended,
        // and a press of `m` would still do nothing.
        let now = t0();
        let mut ui = PushUi::new(false);
        ui.post_progress(PROGRESS.to_string());

        ui.dismiss();
        assert_eq!(
            painted(&mut ui, tall_pane(80), now),
            PROGRESS,
            "a key must not take the notice away",
        );
    }

    #[test]
    fn the_clock_leaves_a_progress_notice_on_the_row() {
        // A rebase replay of a long branch can take longer than a status
        // lives, and the run is still in flight for all of that time.
        let now = t0();
        let mut ui = PushUi::new(false);
        ui.post_progress(PROGRESS.to_string());

        assert_eq!(
            painted(&mut ui, tall_pane(80), now + STATUS_LIFETIME * 3),
            PROGRESS,
            "the clock must not take the notice away, and the notice shows no age",
        );
        assert_eq!(ui.next_tick(), None, "the notice still does not age");
    }

    #[test]
    fn a_progress_notice_is_drawn_like_a_fresh_status_for_as_long_as_it_stays() {
        // The notice is gsw's own words about news that is still true. So it
        // looks like a status at age zero, and not like git's words, which are
        // red. It does not fade, because a fade says that the words are
        // leaving.
        //
        // The color is in the escape bytes, so the rows must carry real ones.
        // `testcolor::with_forced_ansi` makes them do so, as in the fade tests
        // above. One cadence before the end of `STATUS_LIFETIME` is where a
        // fading status is darkest and still on screen.
        let start = t0();
        let late = start + STATUS_LIFETIME - STATUS_CADENCE;

        let (deep_fresh, deep_late, coarse_late) = testcolor::with_forced_ansi(|| {
            let mut deep = PushUi::new(true);
            deep.post_progress(PROGRESS.to_string());
            let deep_fresh = deep.overlay(tall_pane(80), start).text();
            let deep_late = deep.overlay(tall_pane(80), late).text();

            let mut coarse = PushUi::new(false);
            coarse.post_progress(PROGRESS.to_string());
            let coarse_late = coarse.overlay(tall_pane(80), late).text();
            (deep_fresh, deep_late, coarse_late)
        });

        assert_eq!(testcolor::strip_ansi(&deep_fresh), PROGRESS);
        assert_eq!(
            max_red_channel(&deep_fresh),
            STATUS_RGB.0,
            "the notice must be drawn in the color of a status at age zero, got {deep_fresh:?}",
        );
        assert_eq!(
            max_red_channel(&deep_late),
            STATUS_RGB.0,
            "the notice must not fade, got {deep_late:?}",
        );
        assert_eq!(
            coarse_late, PROGRESS,
            "with no truecolor the notice stays plain, and it never dims",
        );
    }

    #[test]
    fn a_progress_notice_never_waits_for_a_busy_row() {
        // A notice that reached the row after its run ended would say that a
        // run is in flight when none is. So a push in flight or a question
        // that owns the row takes the notice away for good.
        let now = t0();

        let mut ui = pushing(now);
        ui.post_progress(PROGRESS.to_string());
        ui.finished(
            PushOutcome {
                success: false,
                output: "error: failed to push some refs\n".to_string(),
            },
            now,
        );
        let seen = drained(&mut ui, now);
        assert!(
            !seen.iter().any(|text| text.contains(PROGRESS)),
            "a notice posted during a push must never reach the row, got {seen:?}",
        );

        let mut ui = asking();
        ui.post_progress(PROGRESS.to_string());
        ui.cancel();
        assert_eq!(
            painted(&mut ui, tall_pane(80), now),
            "",
            "a notice posted during a question must never reach the row",
        );

        // The control. The same door on the row that is free now puts the
        // notice there, so the two checks above are about a busy row, and not
        // about a door that does nothing.
        ui.post_progress(PROGRESS.to_string());
        assert_eq!(painted(&mut ui, tall_pane(80), now), PROGRESS);
    }

    #[test]
    fn the_next_message_replaces_a_progress_notice() {
        // Each of these messages is newer news than the notice. The result of
        // the run is one of them, and a key that asks a question is another.
        /// One way to post a newer message onto the row at an instant.
        type Replace = fn(&mut PushUi, Instant);

        let now = t0();
        let push_hint = confirm_hint(PUSH_VERB);
        let replacements: [(&str, Replace, &str); 4] = [
            (
                "a notice",
                |ui, now| {
                    let _ = ui.post_notice(NOTICE.to_string(), now);
                },
                NOTICE,
            ),
            (
                "an error",
                |ui, _| ui.post_error("branch main names no issue".to_string()),
                "branch main names no issue",
            ),
            (
                "another progress notice",
                |ui, _| ui.post_progress("Running grind and grime against master…".to_string()),
                "against master…",
            ),
            (
                "a press of p",
                |ui, now| ui.request(&snapshot(None), tall_pane(80), now),
                &push_hint,
            ),
        ];

        for (what, replace, shows) in replacements {
            let mut ui = PushUi::new(false);
            ui.post_progress(PROGRESS.to_string());
            assert_eq!(
                painted(&mut ui, tall_pane(80), now),
                PROGRESS,
                "the notice must be on the row before {what}",
            );

            replace(&mut ui, now);
            let text = painted(&mut ui, tall_pane(80), now);
            assert!(
                text.contains(shows),
                "{what} must take the row, got {text:?}"
            );
            assert!(
                !text.contains(PROGRESS),
                "{what} must replace the progress notice, got {text:?}",
            );
        }
    }
}

#[cfg(test)]
mod run_tests {
    use super::*;
    use crate::testrepo::{git, init_repo_with_upstream};

    /// A clone with a `feature` branch holding one commit, ready to push.
    ///
    /// `feature` rather than `main` because the origin fixture is a normal
    /// checkout, and git refuses to push to the branch a non-bare repository
    /// has checked out.
    fn clone_with_feature_branch() -> (tempfile::TempDir, tempfile::TempDir) {
        let (origin, clone) = init_repo_with_upstream();
        let p = clone.path();
        git(p, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(p.join("feature.txt"), "work\n").expect("write feature.txt");
        git(p, &["add", "feature.txt"]);
        git(p, &["commit", "-q", "-m", "feature work"]);
        (origin, clone)
    }

    /// The command a confirmation shown on `feature` would have carried.
    ///
    /// Every fixture here is checked out on `feature`, so this is what the
    /// confirmation named — and the runner refuses anything whose branch no
    /// longer matches the checkout.
    fn confirmed(args: &[&str]) -> PushCommand {
        PushCommand::new("feature", args.iter().map(|s| (*s).to_string()).collect())
    }

    /// [`run_push`] for a test that does not care what arrived while it ran,
    /// which is every test here but the streaming one.
    fn run_quiet(command: &PushCommand, workdir: &Path) -> PushOutcome {
        run_push(command, workdir, &|_| {})
    }

    /// Whether `origin` has a `feature` branch, read from the origin itself.
    fn origin_has_feature(origin: &Path) -> bool {
        Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", "refs/heads/feature"])
            .current_dir(origin)
            .output()
            .expect("invoke git")
            .status
            .success()
    }

    #[test]
    fn creating_a_remote_branch_really_creates_it() {
        // The end-to-end create: the branch must exist on the remote
        // afterwards, and the local branch must now track it — which is what
        // makes the next `p` a plain update.
        let (origin, clone) = clone_with_feature_branch();
        assert!(
            !origin_has_feature(origin.path()),
            "the fixture must start without the branch",
        );

        let outcome = run_quiet(
            &confirmed(&["push", "-u", "origin", "feature"]),
            clone.path(),
        );

        assert!(outcome.success, "push failed: {}", outcome.output);
        assert!(
            origin_has_feature(origin.path()),
            "the branch must exist on the remote after the push",
        );

        let upstream = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "feature@{upstream}"])
            .current_dir(clone.path())
            .output()
            .expect("invoke git");
        assert_eq!(
            String::from_utf8_lossy(&upstream.stdout).trim(),
            "origin/feature",
            "-u must record the upstream, or the next push asks the same question",
        );
    }

    #[test]
    fn updating_a_tracked_branch_sends_the_new_commit() {
        // The plain `git push` path: no remote or refspec is passed, so this
        // also proves git really does read them out of the branch config.
        let (origin, clone) = clone_with_feature_branch();
        let p = clone.path();
        git(p, &["push", "-q", "-u", "origin", "feature"]);
        std::fs::write(p.join("feature.txt"), "more work\n").expect("write feature.txt");
        git(p, &["commit", "-q", "-am", "more work"]);

        let outcome = run_quiet(&confirmed(&["push"]), p);
        assert!(outcome.success, "push failed: {}", outcome.output);

        let subject = Command::new("git")
            .args(["log", "-1", "--format=%s", "refs/heads/feature"])
            .current_dir(origin.path())
            .output()
            .expect("invoke git");
        assert_eq!(
            String::from_utf8_lossy(&subject.stdout).trim(),
            "more work",
            "the remote branch must carry the new commit",
        );
    }

    #[test]
    fn a_rejected_push_reports_the_rejection() {
        // A diverged branch: the commit is amended after being pushed, so the
        // remote holds one the local branch no longer has. git rejects it, and
        // that rejection must survive into the outcome rather than being
        // flattened into a bare failure.
        let (_origin, clone) = clone_with_feature_branch();
        let p = clone.path();
        git(p, &["push", "-q", "-u", "origin", "feature"]);
        git(p, &["commit", "-q", "--amend", "-m", "rewritten"]);

        let outcome = run_quiet(&confirmed(&["push"]), p);
        assert!(!outcome.success, "a diverged push must fail");
        assert!(
            outcome.output.contains("rejected"),
            "git's rejection must reach the outcome, got {:?}",
            outcome.output,
        );
    }

    #[test]
    fn a_push_to_a_remote_that_does_not_exist_reports_it() {
        let (_origin, clone) = clone_with_feature_branch();
        let outcome = run_quiet(
            &confirmed(&["push", "no-such-remote", "feature"]),
            clone.path(),
        );
        assert!(!outcome.success, "pushing to a missing remote must fail");
        assert!(
            !outcome.output.trim().is_empty(),
            "a failure must carry something to show the user",
        );
    }

    #[test]
    fn a_failure_always_carries_something_to_show() {
        // Belt and braces on the whole error path: whatever git does, an
        // unsuccessful outcome never arrives with an empty message, because a
        // blank status row reads as success.
        let (_origin, clone) = clone_with_feature_branch();
        let outcome = run_quiet(&confirmed(&["push", "--no-such-flag"]), clone.path());
        assert!(!outcome.success);
        assert!(!failure_lines(&outcome.output).is_empty());
        assert!(!outcome.output.trim().is_empty());
    }

    #[test]
    fn git_cannot_prompt_at_the_terminal() {
        // gsw holds the alternate screen in raw mode. A git that can prompt
        // would read the same keystrokes the event reader is reading, behind a
        // question gsw did not draw. It must fail fast instead of waiting.
        let (_origin, clone) = clone_with_feature_branch();
        let outcome = run_quiet(
            &confirmed(&["push", "https://user@127.0.0.1:1/nope.git", "feature"]),
            clone.path(),
        );
        assert!(
            !outcome.success,
            "an unreachable authenticated remote must fail"
        );
        assert!(
            !outcome.output.trim().is_empty(),
            "the failure must say something, got {:?}",
            outcome.output,
        );
    }

    /// What the push child's *descendants* can reach, which is where a terminal
    /// prompt actually comes from: the ssh transport, a credential helper, anything
    /// git execs. Unix-only because `/dev/tty` is: Windows denies the same access
    /// by denying the child a console, and nothing here runs on Windows to check
    /// it — see [`detach_from_terminal`]'s Windows arm, which says so plainly.
    #[cfg(unix)]
    mod terminal_tests {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// What the fake ssh below writes when it *could* open the controlling
        /// terminal — the failure this test exists to catch.
        const TTY_OPENED: &str = "opened";

        /// What the fake ssh writes when `/dev/tty` was unopenable, which is the
        /// only outcome that keeps a transport from painting a prompt over gsw's
        /// frame and racing it for the user's keystrokes.
        const TTY_REFUSED: &str = "refused";

        /// Whether the *test process* can open the controlling terminal.
        ///
        /// The assertion below is only worth making when there is a terminal for
        /// the child to be denied. A `cargo test` started from a script, a CI
        /// runner, or this repository's own pre-commit hook has no controlling
        /// terminal at all, and `/dev/tty` is then unopenable for every process in
        /// the tree whether or not the push child is detached — so the test would
        /// pass while exercising nothing. It skips rather than bank a vacancy as a
        /// green.
        fn test_process_can_open_the_terminal() -> bool {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/tty")
                .is_ok()
        }

        #[test]
        fn the_push_child_cannot_open_the_controlling_terminal() {
            // `GIT_TERMINAL_PROMPT=0` governs git's *own* prompts. The ssh
            // transport is a separate program, and OpenSSH's `read_passphrase()`
            // opens `/dev/tty` directly — a closed stdin and a captured stderr
            // never reach it. So a passphrase-protected key with no agent, or an
            // unknown host key, would paint a prompt gsw cannot see over the
            // alternate screen and read the keystrokes the event thread is waiting
            // for, with no timeout to end it. Nothing in the process tree may be
            // able to open the terminal.
            if !test_process_can_open_the_terminal() {
                eprintln!(
                    "skipped: this test process has no controlling terminal, so /dev/tty is \
                 unopenable for every child regardless — the assertion would hold vacuously",
                );
                return;
            }

            let (_origin, clone) = clone_with_feature_branch();
            let p = clone.path();

            // Its own tempdir: two copies of this test run at once under a parallel
            // `cargo test`, and a shared script or record path would have them
            // overwrite each other's answer.
            let probe = tempfile::tempdir().expect("tempdir");
            let record = probe.path().join("tty-probe");
            let fake_ssh = probe.path().join("fake-ssh");
            // The subshell is deliberate: a failed `exec` redirection exits the
            // shell it runs in, so the probe has to be a shell of its own for the
            // `else` branch to be reachable.
            std::fs::write(
                &fake_ssh,
                format!(
                    "#!/bin/sh\n\
                 if ( exec 3<>/dev/tty ) 2>/dev/null; then\n\
                 \tprintf '{TTY_OPENED}' > '{record}'\n\
                 else\n\
                 \tprintf '{TTY_REFUSED}' > '{record}'\n\
                 fi\n\
                 exit 1\n",
                    record = record.display(),
                ),
            )
            .expect("write the fake ssh");
            std::fs::set_permissions(
                &fake_ssh,
                std::fs::Permissions::from_mode(FAKE_SSH_EXECUTABLE_MODE),
            )
            .expect("make the fake ssh executable");

            // `core.sshCommand` rather than `GIT_SSH_COMMAND`: the environment is
            // process-global and this binary runs many git commands concurrently,
            // so an env var would reach every other test's push. The config entry
            // reaches this repository only. Nothing leaves the machine either —
            // the fake ssh replaces the transport before any socket is opened.
            git(
                p,
                &[
                    "config",
                    "core.sshCommand",
                    fake_ssh.to_str().expect("utf-8 tempdir path"),
                ],
            );
            git(
                p,
                &["remote", "add", "tty-probe", "ssh://127.0.0.1/nope.git"],
            );

            let outcome = run_quiet(&confirmed(&["push", "tty-probe", "feature"]), p);
            assert!(
                !outcome.success,
                "the fake ssh connects to nothing, so the push must fail: {}",
                outcome.output,
            );

            let recorded = std::fs::read_to_string(&record)
                .expect("git never ran the ssh transport, so the probe proved nothing");
            assert_eq!(
                recorded.trim(),
                TTY_REFUSED,
                "the push child's own children can still open the controlling terminal, \
             so ssh can prompt over gsw's frame and steal its keystrokes",
            );
        }

        /// `rwxr-xr-x` — git has to be able to execute the fake ssh it is pointed
        /// at, and a file written by `std::fs::write` is not executable.
        const FAKE_SSH_EXECUTABLE_MODE: u32 = 0o755;
    }

    /// The commit `refs/heads/<branch>` points at in the repository at `dir`,
    /// or `""` when there is no such branch. Compared before and after a push
    /// to say whether anything was actually sent.
    fn tip(dir: &Path, branch: &str) -> String {
        let output = Command::new("git")
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .current_dir(dir)
            .output()
            .expect("invoke git");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn an_update_whose_branch_changed_since_the_confirmation_is_not_pushed() {
        // The window the confirmation opens: `p` resolves the command while
        // `feature` is checked out, `y` arrives seconds later, and a checkout in
        // another pane lands in between. A bare `git push` names no branch, so
        // git would resolve it against the *new* HEAD and send a branch the
        // question never mentioned. Nothing may be pushed in that case.
        let (origin, clone) = clone_with_feature_branch();
        let p = clone.path();
        git(p, &["push", "-q", "-u", "origin", "feature"]);
        // A second branch that also tracks the origin, so a mis-resolved bare
        // push would succeed rather than being stopped by something else.
        git(p, &["checkout", "-q", "-b", "other", "main"]);
        std::fs::write(p.join("other.txt"), "other work\n").expect("write other.txt");
        git(p, &["add", "other.txt"]);
        git(p, &["commit", "-q", "-m", "other work"]);
        git(p, &["push", "-q", "-u", "origin", "other"]);
        std::fs::write(p.join("other.txt"), "more other work\n").expect("write other.txt");
        git(p, &["commit", "-q", "-am", "more other work"]);

        // Confirmed on `feature`, which is what the question named…
        let command = PushCommand::new("feature", vec!["push".to_string()]);
        // …but `other` is what is checked out when the answer arrives.
        let before_other = tip(origin.path(), "other");
        let before_feature = tip(origin.path(), "feature");

        let outcome = run_quiet(&command, p);

        assert!(
            !outcome.success,
            "a push whose branch changed must not report success: {}",
            outcome.output,
        );
        assert_eq!(
            tip(origin.path(), "other"),
            before_other,
            "the branch that was checked out at exec time must not be pushed",
        );
        assert_eq!(
            tip(origin.path(), "feature"),
            before_feature,
            "nothing at all may be pushed once the checkout no longer matches",
        );
        assert!(
            outcome.output.contains("branch changed"),
            "the outcome must say the branch changed, got {:?}",
            outcome.output,
        );
        assert!(
            outcome.output.contains("press p again"),
            "the outcome must say how to retry, got {:?}",
            outcome.output,
        );
    }

    #[test]
    fn a_create_whose_branch_changed_since_the_confirmation_is_not_pushed() {
        // A create names its branch in the arguments, so it would still push the
        // right ref — but the check is one rule, not a per-variant exception: a
        // confirmation is an answer about the repository as it stood, and gsw
        // asks again rather than acting on a repository that moved.
        let (origin, clone) = clone_with_feature_branch();
        let p = clone.path();
        let command = PushCommand::new(
            "feature",
            ["push", "-u", "origin", "feature"]
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        );
        git(p, &["checkout", "-q", "main"]);

        let outcome = run_quiet(&command, p);

        assert!(
            !outcome.success,
            "a create whose branch changed must not report success: {}",
            outcome.output,
        );
        assert!(
            !origin_has_feature(origin.path()),
            "the confirmed branch must not reach the remote after the checkout changed",
        );
        assert!(
            outcome.output.contains("branch changed"),
            "the outcome must say the branch changed, got {:?}",
            outcome.output,
        );
        assert!(
            outcome.output.contains("press p again"),
            "the outcome must say how to retry, got {:?}",
            outcome.output,
        );
    }

    #[test]
    fn a_detached_head_after_the_confirmation_is_not_pushed() {
        // The other way the checkout can move: `git rebase`, `git bisect`, or a
        // plain `git checkout <sha>` in another pane leaves no branch at all.
        // `HEAD` is not a name git accepts for a branch, so it can never match
        // the one the confirmation named.
        let (origin, clone) = clone_with_feature_branch();
        let p = clone.path();
        let command = confirmed(&["push", "-u", "origin", "feature"]);
        git(p, &["checkout", "-q", "--detach"]);

        let outcome = run_quiet(&command, p);

        assert!(!outcome.success, "got {:?}", outcome.output);
        assert!(
            !origin_has_feature(origin.path()),
            "a detached checkout must stop the push like any other change",
        );
        assert!(
            outcome.output.contains(DETACHED_HEAD),
            "the outcome must name what HEAD is now, got {:?}",
            outcome.output,
        );
    }

    /// The runner reports a hook's output while the hook is still running.
    ///
    /// Unix-only for the hook's execute bit, which is what makes git run it at
    /// all. The rule under test is not Unix-specific — a pipe read returns what
    /// the writer flushed on every platform — but a test that cannot make an
    /// executable hook cannot ask the question.
    #[cfg(unix)]
    mod streaming_tests {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{Arc, Mutex};

        /// What the hook writes before it waits to hear that the line arrived.
        const FIRST_LINE: &str = "hook-said-this-first";

        /// What the hook writes only once it has heard, so its presence proves
        /// the runner reported the first line while the child was still alive.
        const SECOND_LINE: &str = "hook-said-this-second";

        /// Install `body` as the repository's pre-push hook.
        ///
        /// `core.hooksPath` is stated rather than inherited: a developer with
        /// one set globally would otherwise run their own hooks here, and the
        /// test would pass or fail on a machine's configuration.
        fn write_hook(workdir: &Path, body: &str) {
            git(workdir, &["config", "core.hooksPath", ".git/hooks"]);
            let hook = workdir.join(".git").join("hooks").join("pre-push");
            std::fs::create_dir_all(hook.parent().expect("the hook has a parent"))
                .expect("create the hooks directory");
            std::fs::write(&hook, format!("#!/bin/sh\n{body}\n")).expect("write the pre-push hook");
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
                .expect("make the hook executable");
        }

        /// Longest the hook waits to be told, in units of its own poll.
        /// Bounded so a runner that reports nothing until the child exits
        /// fails this test in a few seconds instead of deadlocking it: the
        /// runner would be waiting for a hook that is waiting for the runner.
        const GATE_POLLS: u32 = 100;

        #[test]
        fn the_runner_reports_a_line_while_the_push_is_still_running() {
            // This is the whole feature. A pre-push hook that runs a test suite
            // holds the push for minutes, and output that arrives only when the
            // child exits is output that arrives when nobody needs it any more.
            let (_origin, clone) = clone_with_feature_branch();
            let p = clone.path();

            // Inside the git dir rather than the worktree, so the gate cannot
            // appear as an untracked file in the repository under test.
            let gate = p.join(".git").join("gate");
            write_hook(
                p,
                &format!(
                    r#"echo "{FIRST_LINE}"
i=0
while [ $i -lt {GATE_POLLS} ]; do
    [ -f "{gate}" ] && break
    sleep 0.2
    i=$((i + 1))
done
if [ ! -f "{gate}" ]; then
    echo "the runner reported no line while the hook was running" >&2
    exit 1
fi
echo "{SECOND_LINE}"
"#,
                    gate = gate.display(),
                ),
            );

            let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let recorded = Arc::clone(&seen);
            let gate_path = gate;
            let outcome = run_push(
                &confirmed(&["push", "-u", "origin", "feature"]),
                p,
                &move |line: String| {
                    if line == FIRST_LINE {
                        std::fs::write(&gate_path, b"open").expect("open the gate");
                    }
                    recorded
                        .lock()
                        .expect("the record lock is never poisoned")
                        .push(line);
                },
            );

            // The hook itself makes the assertion: it exits non-zero when it
            // was never told, so a runner that buffers to the end fails here
            // with the hook's own words.
            assert!(outcome.success, "push failed: {}", outcome.output);

            let seen = seen.lock().expect("the record lock is never poisoned");
            assert!(
                seen.iter().any(|line| line == FIRST_LINE),
                "the first line must reach the reporter, got {seen:?}",
            );
            assert!(
                seen.iter().any(|line| line == SECOND_LINE),
                "the line written after the gate opened must reach it too, got {seen:?}",
            );
        }

        #[test]
        fn a_failed_push_shows_the_last_thing_said_across_both_pipes() {
            // The runner reads two pipes. Joining their text by stream puts
            // the whole of one after the whole of the other, so the last three
            // lines are the tail of one pipe alone and whatever the other said
            // last is nowhere in them.
            //
            // That is the ordinary failure. A hook prints its progress on
            // stdout and fails, and git writes `error: failed to push some
            // refs` on stderr after everything — so grouping by stream buries
            // the one line that says the push did not happen, behind a hook's
            // banner that says nothing about it.
            //
            // The gate makes the order a fact rather than a race: the hook
            // does not exit until the runner has reported its last stdout
            // line, so git's stderr cannot be read before them.
            let (_origin, clone) = clone_with_feature_branch();
            let p = clone.path();
            let gate = p.join(".git").join("gate");
            write_hook(
                p,
                &format!(
                    r#"for i in 1 2 3 4; do echo "hook stdout $i"; done
i=0
while [ $i -lt {GATE_POLLS} ]; do
    [ -f "{gate}" ] && break
    sleep 0.2
    i=$((i + 1))
done
exit 1"#,
                    gate = gate.display(),
                ),
            );

            let gate_path = gate;
            let outcome = run_push(
                &confirmed(&["push", "-u", "origin", "feature"]),
                p,
                &move |line: String| {
                    if line == "hook stdout 4" {
                        std::fs::write(&gate_path, b"open").expect("open the gate");
                    }
                },
            );

            assert!(
                !outcome.success,
                "the hook exits non-zero, so the push must fail",
            );
            let shown = failure_lines(&outcome.output);
            assert!(
                shown.iter().any(|line| line.contains("error:")),
                "git's verdict is the last thing said and must survive, got {shown:?}",
            );
        }

        /// What [`spawn`] delivered, in the order the channel carried it.
        enum Report {
            Line(String),
            Done(PushOutcome),
        }

        #[test]
        fn spawn_reports_every_line_before_the_outcome() {
            // The watch loop applies events in the order they arrive, and the
            // outcome closes the window. A line that landed after it would
            // reopen the window over the message the user is meant to read.
            //
            // This passes as written, because `run_push` joins both reader
            // threads before it returns and `spawn` reports the outcome after
            // that. The test is here to keep it true: the two are separated by
            // a thread boundary, and nothing else states the order.
            let (_origin, clone) = clone_with_feature_branch();
            let p = clone.path();
            write_hook(p, "for i in 1 2 3; do echo \"line-$i\"; done");

            let (tx, rx) = std::sync::mpsc::channel();
            let line_tx = tx.clone();
            spawn(
                confirmed(&["push", "-u", "origin", "feature"]),
                p.to_path_buf(),
                move |line| {
                    let _ = line_tx.send(Report::Line(line));
                },
                move |outcome| {
                    let _ = tx.send(Report::Done(outcome));
                },
            );

            // Stops at the outcome, so a line that arrived after it is a line
            // this never records — which is what the assertions below catch.
            let mut lines = Vec::new();
            loop {
                match rx
                    .recv_timeout(Duration::from_secs(60))
                    .expect("the callbacks must fire")
                {
                    Report::Line(line) => lines.push(line),
                    Report::Done(outcome) => {
                        assert!(outcome.success, "push failed: {}", outcome.output);
                        break;
                    }
                }
            }

            for expected in ["line-1", "line-2", "line-3"] {
                assert!(
                    lines.iter().any(|line| line == expected),
                    "{expected} must arrive before the outcome, got {lines:?}",
                );
            }
        }
    }

    #[test]
    fn spawn_delivers_the_outcome_off_the_calling_thread() {
        // The loop learns a push finished only through this callback, so a
        // push that completes without calling it would leave the monitor stuck
        // in the pushing mode forever.
        let (origin, clone) = clone_with_feature_branch();
        let (tx, rx) = std::sync::mpsc::channel();

        spawn(
            confirmed(&["push", "-u", "origin", "feature"]),
            clone.path().to_path_buf(),
            // This test is about the outcome hop, not the line hop.
            |_line| {},
            move |outcome| {
                let _ = tx.send(outcome);
            },
        );

        let outcome = rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the callback must fire when the push finishes");
        assert!(outcome.success, "push failed: {}", outcome.output);
        assert!(origin_has_feature(origin.path()));
    }
}
