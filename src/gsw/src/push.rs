//! Pushing the current branch from watch mode: what a push will do, how it is
//! described to the user before it runs, and how it is run.
//!
//! `gix` cannot push, so the push itself is a `git` child process. Everything
//! that *decides* — which remote, which arguments, what the confirmation says,
//! and what an outcome means — lives here as pure, terminal-free code so it can
//! be tested without a network or a pty. Only [`run_push`], the blocking half of
//! [`spawn`], starts a process: the push itself, and the read of HEAD that
//! checks the repository is still on the branch the confirmation named.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use colored::Colorize;

use crate::render::{truncate_right, Snapshot, UpstreamStatus};
use crate::repo::DETACHED_HEAD;
use crate::watch::{Dimensions, InputMode};

/// Most rows a status message is allowed to occupy under the frame.
///
/// A rejected push can produce a dozen lines of hints, and the frame below is
/// what the user is actually watching. Three rows is enough for git's
/// `To <remote>` / `! [rejected] …` / `error: failed to push …` triple, which
/// is the part that says what went wrong.
///
/// A ceiling, not a promise: a pane with fewer than four rows cannot spare
/// three and still show a frame, so [`PushUi::overlay`] clips the message
/// further. This is the most the user will ever see, not the least.
const MAX_STATUS_ROWS: usize = 3;

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

/// What the watch loop does when the user presses `p`.
///
/// This is the whole interface [`prompt_for`] hands back, and it is deliberately
/// two cases rather than five: the caller asks a question or shows a message,
/// and never learns which plan produced either. A [`PushPlan`] variant added
/// later — a rejected force push, a protected branch — changes the wording here
/// without touching the loop that displays it.
///
/// [`PushPrompt::Confirm`] carries the command with the question, so the
/// arguments cannot be requested for a push that must never run. The invariant
/// is structural: there is no way to hold a `Confirm` without holding the exact
/// [`PushCommand`] the confirmation described — the argument list *and* the
/// branch it was written for, which is what the runner re-checks before it
/// pushes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PushPrompt {
    /// Ask before running the push.
    Confirm {
        /// The question, without the key hint — the display layer owns the
        /// `[y/N]` convention.
        question: String,
        /// Whether this push creates a branch on the remote. The display layer
        /// colors this case differently, so a create can never be mistaken for
        /// a routine update at a glance.
        creates_remote_branch: bool,
        /// The branch and the arguments this question described.
        command: PushCommand,
        /// What to show once this push succeeds. Composed here, with the
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
                creates_remote_branch: true,
                // `-u` records the new remote branch as the upstream, so the
                // push after this one is a plain update.
                command: PushCommand::new(
                    branch.clone(),
                    vec!["push".to_string(), "-u".to_string(), remote, branch],
                ),
                success_message,
            }
        }
        PushPlan::Update { target, commits } => {
            let unit = if commits == 1 { "commit" } else { "commits" };
            PushPrompt::Confirm {
                question: format!("Push {commits} {unit} to {target}?"),
                creates_remote_branch: false,
                // Bare `push`: git reads the remote and the refspec out of the
                // branch config, so a branch tracking something other than the
                // repository's default remote still goes to the right place.
                // Which branch's config it reads is decided by HEAD at exec
                // time, which is why the command carries the branch this
                // question was written for.
                command: PushCommand::new(branch, vec!["push".to_string()]),
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
pub(crate) fn spawn<F>(command: PushCommand, workdir: PathBuf, on_finish: F)
where
    F: FnOnce(PushOutcome) + Send + 'static,
{
    std::thread::spawn(move || on_finish(run_push(&command, &workdir)));
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
/// - **The child gets no controlling terminal** ([`detach_from_terminal`]), so
///   `/dev/tty` cannot be opened by it or by anything it runs. This is the part
///   that actually holds the guarantee, because a terminal prompt usually comes
///   from a *descendant*: OpenSSH's `read_passphrase()` opens `/dev/tty`
///   directly for a passphrase or an unknown host key, so a closed stdin and a
///   captured stderr never reach it, and it has no read timeout — the push
///   would hang behind a question gsw never drew while the two processes split
///   the user's keystrokes. With no terminal to open, ssh falls back to
///   `SSH_ASKPASS`, and with no `DISPLAY` it fails immediately and says so. The
///   same is true of every other descendant, credential helpers included, which
///   is why this is done to the process rather than to one transport.
/// - **stdin is closed** and **`GIT_TERMINAL_PROMPT=0`**, which is git's own
///   half of the same rule: git asks for HTTP usernames and passwords itself,
///   and this refuses those before the detachment has to. Disabled, git fails
///   immediately and says why, which lands in the status rows like any other
///   error. Credential helpers and a GUI `SSH_ASKPASS` are untouched — they do
///   not need the terminal, and only prompting *at the terminal* is refused.
/// - **Both streams are captured**, which also suppresses git's progress meter:
///   it renders only to a terminal, so a pipe removes the carriage-return
///   redraws that would otherwise arrive as unreadable status rows.
fn run_push(command: &PushCommand, workdir: &Path) -> PushOutcome {
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
        .env("GIT_TERMINAL_PROMPT", "0");
    detach_from_terminal(&mut child);
    let result = child.output();

    let output = match result {
        Ok(output) => output,
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

    // stderr first: `git push` reports what it did — `To <remote>`, the ref
    // updates, and every rejection — on stderr, and writes to stdout only under
    // flags gsw does not pass. Leading with it puts the useful lines at the
    // head, which is the part a status message has room for — at most
    // [`MAX_STATUS_ROWS`], and fewer on a short pane.
    let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    // Carriage returns are how a progress meter redraws in place. Capturing
    // both streams already suppresses it, so this is for anything else that
    // emits CRLF: left in, a stray `\r` would send the cursor to column zero
    // mid-row and scramble the frame under it.
    let mut text = text.replace('\r', "");

    let success = output.status.success();
    if !success && text.trim().is_empty() {
        // A failure with nothing to show would render as a blank row, which
        // reads as success. The exit status is all git left us.
        text = format!("git push failed ({})", output.status);
    }

    PushOutcome {
        success,
        output: text,
    }
}

/// Arrange for `command`'s child to run with no controlling terminal, so
/// nothing in its process tree can open `/dev/tty`.
///
/// The Unix half asks for a new session before the exec. A session leader has
/// no controlling terminal until it deliberately acquires one, and no program
/// git runs does that — so `open("/dev/tty")` returns `ENXIO` for the child,
/// for ssh, and for every credential helper below them. That is what a prompt
/// needs: it is the one path to the terminal that a closed stdin and captured
/// output streams do not cover, because it bypasses the descriptors entirely.
/// Refused it, OpenSSH sets `use_askpass` and either runs `SSH_ASKPASS` (a GUI
/// prompt, which is fine — it does not touch the pane gsw is drawing on) or
/// gives up at once with a message that reaches the status rows.
///
/// The Windows half is a no-op, and honestly so: there is no `/dev/tty` and no
/// session to leave. `GIT_TERMINAL_PROMPT=0` and the closed stdin are the whole
/// defense there.
#[cfg(unix)]
fn detach_from_terminal(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: the closure runs in the forked child, between `fork` and `exec`,
    // where only async-signal-safe functions may be called. `setsid` is one
    // (POSIX.1-2017, "Signal Concepts"); it is a bare syscall that allocates
    // nothing and takes no lock the parent's other threads could be holding.
    unsafe {
        command.pre_exec(|| {
            // The return value is deliberately dropped. `setsid` fails with
            // EPERM when the caller is already a process group leader, which
            // means a new session was not available — harmless, and not worth
            // failing a push over: the terminal defenses below it still stand,
            // and returning an error here would abort the exec and report a
            // push failure to a user who has done nothing wrong. There is no
            // other failure mode.
            let _ = libc::setsid();
            Ok(())
        });
    }
}

/// See the Unix half above: Windows has no controlling terminal to leave and no
/// `/dev/tty` to deny, so there is nothing to arrange.
#[cfg(not(unix))]
fn detach_from_terminal(_command: &mut Command) {}

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
fn current_branch(workdir: &Path) -> Option<String> {
    let output = Command::new("git")
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
/// `output` is git's own stdout and stderr, kept whole: choosing which of it to
/// show is [`PushUi`]'s job, and a runner that pre-digested it would decide the
/// wording from a place with no idea how many rows are free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PushOutcome {
    /// Whether `git push` exited zero.
    pub success: bool,
    /// Everything git wrote, both streams, in the order they were captured.
    pub output: String,
}

/// Everything the push feature puts on screen, and the input mode that goes
/// with it.
///
/// Watch mode holds one of these and asks it two questions — what mode are we
/// in, and what does the pane show. It never learns whether a prompt or an
/// error is up, so the states below can grow without the render loop growing a
/// branch for each one.
pub(crate) struct PushUi {
    state: State,
}

/// What the push feature is currently doing. Private: the loop drives this
/// through [`PushUi`]'s methods and reads it only through
/// [`PushUi::mode`]/[`PushUi::overlay`].
enum State {
    /// Nothing on screen and nothing pending.
    Idle,
    /// A message under the frame, staying until the user presses a key.
    Status {
        /// Lines to show, already trimmed to [`MAX_STATUS_ROWS`]. How many of
        /// them a given pane has room for is [`PushUi::overlay`]'s call.
        lines: Vec<String>,
        /// Whether this reports a failure, which the display colors red.
        failed: bool,
    },
    /// A confirmation is on screen, waiting for an answer.
    Asking {
        question: String,
        creates_remote_branch: bool,
        command: PushCommand,
        success_message: String,
    },
    /// `git push` is running.
    Running { success_message: String },
}

impl PushUi {
    /// A UI with nothing on screen.
    pub(crate) fn new() -> Self {
        Self { state: State::Idle }
    }

    /// What keys mean right now.
    pub(crate) fn mode(&self) -> InputMode {
        match self.state {
            State::Asking { .. } => InputMode::Confirm,
            State::Running { .. } => InputMode::Pushing,
            State::Idle | State::Status { .. } => InputMode::Normal,
        }
    }

    /// Handle `p`: work out what a push would do and either ask or explain.
    ///
    /// Replaces whatever was on screen, so pressing `p` with a stale error up
    /// asks the new question instead of stacking a row under the old one.
    pub(crate) fn request(&mut self, snapshot: &Snapshot) {
        self.state = match prompt_for(
            &snapshot.branch,
            snapshot.push_remote.as_deref(),
            snapshot.upstream.as_ref(),
        ) {
            PushPrompt::Confirm {
                question,
                creates_remote_branch,
                command,
                success_message,
            } => State::Asking {
                question,
                creates_remote_branch,
                command,
                success_message,
            },
            PushPrompt::Refuse { message } => State::Status {
                lines: vec![message],
                failed: false,
            },
        };
    }

    /// Handle `y`: start the push, returning the [`PushCommand`] to run, or
    /// `None` when no confirmation was on screen to accept.
    ///
    /// Moving to [`State::Running`] as it hands the command over is what makes
    /// a second `y` — one that raced the mode change — return `None` rather than
    /// start an overlapping push.
    pub(crate) fn confirm(&mut self) -> Option<PushCommand> {
        let State::Asking {
            command,
            success_message,
            ..
        } = std::mem::replace(&mut self.state, State::Idle)
        else {
            return None;
        };
        self.state = State::Running { success_message };
        Some(command)
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
    pub(crate) fn finished(&mut self, outcome: PushOutcome) {
        let success_message = match std::mem::replace(&mut self.state, State::Idle) {
            State::Running { success_message } => success_message,
            // A finish with no push running: nothing to report against, so
            // leave the screen as it is rather than inventing a message.
            other => {
                self.state = other;
                return;
            }
        };

        let lines = if outcome.success {
            vec![success_message]
        } else {
            failure_lines(&outcome.output)
        };
        self.state = State::Status {
            lines,
            failed: !outcome.success,
        };
    }

    /// Handle a key with no other meaning: clear a status message if one is up.
    /// Leaves a question or a running push alone — neither is the user's to
    /// dismiss by pressing an unrelated key.
    pub(crate) fn dismiss(&mut self) {
        if matches!(self.state, State::Status { .. }) {
            self.state = State::Idle;
        }
    }

    /// What the push feature shows in a pane of `dims`: the lines that fit
    /// there, and — as [`Overlay::rows`] — how many rows the frame must give up
    /// to make room for them.
    ///
    /// Both come out of this one call because a row count and a body computed
    /// apart can disagree, and either direction of disagreement is a bug: a
    /// count larger than the text leaves a blank strip between the frame and
    /// the message, a count smaller than it paints past the bottom of the pane.
    ///
    /// Two clamps, one contract. gsw's standing rule is that nothing it paints
    /// ever wraps or scrolls the pane it was measured to fill, so each line is
    /// truncated to `dims.width` by display column (UTF-8 safe), and the
    /// overlay as a whole is capped at one row short of `dims.height`. The
    /// frame therefore always keeps a row of its own, and a message too tall
    /// for what is left loses its last lines — [`failure_lines`] puts git's
    /// most useful output first, so the head is the part worth keeping.
    pub(crate) fn overlay(&self, dims: Dimensions) -> Overlay {
        let width = dims.width;
        let lines: Vec<String> = match &self.state {
            State::Idle => Vec::new(),
            State::Asking {
                question,
                creates_remote_branch,
                ..
            } => {
                let line = truncate_right(&format!("{question}  {CONFIRM_HINT}"), width);
                // Yellow marks the push that puts something new on a shared
                // remote. The wording says so too — the color is what carries
                // it in the half second before the words are read.
                vec![if *creates_remote_branch {
                    line.yellow().to_string()
                } else {
                    line
                }]
            }
            State::Running { .. } => vec![truncate_right(RUNNING_NOTICE, width)],
            State::Status { lines, failed } => lines
                .iter()
                .map(|line| {
                    let line = truncate_right(line, width);
                    if *failed {
                        line.red().to_string()
                    } else {
                        line
                    }
                })
                .collect(),
        };
        // The frame never gives up its last row. A pane painted entirely by the
        // push feature would leave the user watching an error with nothing
        // under it to say which repository it belongs to — and the frame is
        // what watch mode is for.
        Overlay {
            lines: lines
                .into_iter()
                .take(dims.height.saturating_sub(1))
                .collect(),
        }
    }
}

/// What the push feature paints under the frame, sized for one particular pane.
///
/// Built only by [`PushUi::overlay`], which is what makes the row count and the
/// text impossible to disagree about: they are the same `Vec` — one measured,
/// the other joined.
pub(crate) struct Overlay {
    /// Painted lines, each already truncated to the pane's width, and at most
    /// one fewer of them than the pane has rows.
    lines: Vec<String>,
}

impl Overlay {
    /// How many rows the frame must give up. Always at least one short of the
    /// pane, so the frame keeps a row whatever the overlay wanted to say.
    pub(crate) fn rows(&self) -> usize {
        self.lines.len()
    }

    /// The text to paint under the frame: exactly [`Overlay::rows`] lines, and
    /// empty when there are none.
    pub(crate) fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// The key hint shown with every confirmation.
///
/// Spelled out rather than the usual `[y/N]`. That convention's capital letter
/// means "this is what Enter gives you", and Enter *confirms* here — so `[y/N]`
/// would promise that the key people reach for by reflex is the safe one, on
/// the one prompt in gsw that writes to a shared remote.
const CONFIRM_HINT: &str = "[y/Enter = push, n/Esc = cancel]";

/// What a running push says while the network round trip is in flight.
const RUNNING_NOTICE: &str = "Pushing…";

/// What a failed push says when git said nothing gsw could show.
const SILENT_FAILURE: &str = "git push failed";

/// Pick the lines of a failed push's output worth the rows they cost.
///
/// git leads with the useful part — `To <remote>`, `! [rejected] …`,
/// `error: failed to push …` — and follows with `hint:` advice that repeats in
/// every rejection. So the hints are dropped first, and only if that leaves
/// nothing are they let back in: a message the user cannot read beats a blank
/// row that reads as success.
fn failure_lines(output: &str) -> Vec<String> {
    let meaningful = |line: &&str| !line.trim().is_empty();
    let mut lines: Vec<String> = output
        .lines()
        .filter(meaningful)
        .filter(|line| !line.trim_start().starts_with(HINT_PREFIX))
        .take(MAX_STATUS_ROWS)
        .map(|line| line.trim_end().to_string())
        .collect();

    if lines.is_empty() {
        lines = output
            .lines()
            .filter(meaningful)
            .take(MAX_STATUS_ROWS)
            .map(|line| line.trim_end().to_string())
            .collect();
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

    /// The command a confirmable prompt carries. Panics on a refusal, so a test
    /// that expected a push and got a message fails on the line that asked.
    fn command(prompt: &PushPrompt) -> &PushCommand {
        match prompt {
            PushPrompt::Confirm { command, .. } => command,
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
            matches!(
                prompt,
                PushPrompt::Confirm {
                    creates_remote_branch: true,
                    ..
                },
            ),
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
            matches!(
                prompt,
                PushPrompt::Confirm {
                    creates_remote_branch: false,
                    ..
                },
            ),
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
        let PushPrompt::Confirm { command, .. } = prompt_for("gsw-push", Some("origin"), None)
        else {
            panic!("an untracked branch with a remote must be confirmable");
        };
        assert_eq!(command.args(), ["push", "-u", "origin", "gsw-push"]);
        assert_eq!(command.branch(), "gsw-push");
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
    /// other tests in this binary toggle (`colored::control::set_override`), so
    /// a raw `UnicodeWidthStr::width` counts escape bytes as columns in some
    /// runs and not others — the assertion would pass or fail depending on test
    /// order. Columns on screen are also the thing under test: the overlay
    /// colors *after* truncating, so the escapes cost the user nothing.
    fn visible_width(line: &str) -> usize {
        let mut visible = String::new();
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                // A CSI sequence runs until a letter terminates it.
                for tail in chars.by_ref() {
                    if tail.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                visible.push(c);
            }
        }
        unicode_width::UnicodeWidthStr::width(visible.as_str())
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
        let mut ui = PushUi::new();
        ui.request(&snapshot(None));
        ui
    }

    #[test]
    fn a_fresh_ui_shows_nothing_and_leaves_the_keys_alone() {
        let ui = PushUi::new();
        assert_eq!(ui.mode(), InputMode::Normal);
        assert_eq!(ui.overlay(tall_pane(80)).rows(), 0);
        assert_eq!(ui.overlay(tall_pane(80)).text(), "");
    }

    #[test]
    fn requesting_a_push_asks_the_question_and_takes_the_keys() {
        // `p` on a pushable branch must put the question on screen AND switch
        // the key table, or `y` would be read as an ordinary key.
        let ui = asking();
        assert_eq!(ui.mode(), InputMode::Confirm);
        assert_eq!(ui.overlay(tall_pane(80)).rows(), 1);
        let overlay = ui.overlay(tall_pane(80)).text();
        assert!(
            overlay.contains("Create new remote branch origin/gsw-push?"),
            "the question must be on screen, got {overlay:?}",
        );
        // Spelled out rather than the usual `[y/N]`, because a capital `N` is
        // the convention for "Enter means no" and Enter confirms here. A hint
        // that lies about the riskiest key on the prompt is worse than a long
        // one.
        assert!(
            overlay.contains(CONFIRM_HINT),
            "the overlay owns the key hint, got {overlay:?}",
        );
    }

    #[test]
    fn requesting_a_push_with_nothing_to_push_explains_instead_of_asking() {
        // The refusal is a message, not a question: the keys must stay normal,
        // so `y` does not answer a prompt that is not there.
        let mut ui = PushUi::new();
        ui.request(&snapshot(tracked(0)));
        assert_eq!(ui.mode(), InputMode::Normal);
        assert_eq!(ui.overlay(tall_pane(80)).rows(), 1);
        assert!(ui
            .overlay(tall_pane(80))
            .text()
            .contains("origin/gsw-push is already up to date"));
        assert!(
            !ui.overlay(tall_pane(80)).text().contains(CONFIRM_HINT),
            "a refusal must not offer keys that do nothing",
        );
    }

    #[test]
    fn confirming_hands_back_the_command_and_switches_to_pushing() {
        let mut ui = asking();
        let command = ui.confirm().expect("a question on screen must confirm");
        assert_eq!(command.args(), ["push", "-u", "origin", "gsw-push"]);
        assert_eq!(
            command.branch(),
            "gsw-push",
            "the branch the question named must reach the runner",
        );
        assert_eq!(ui.mode(), InputMode::Pushing);
        assert_eq!(
            ui.overlay(tall_pane(80)).rows(),
            1,
            "the running push stays on screen"
        );
    }

    #[test]
    fn confirming_with_no_question_up_runs_nothing() {
        // Belt and braces against a stray PushConfirmed: with no confirmation
        // on screen there is no command to run, and inventing one would push
        // without asking.
        let mut ui = PushUi::new();
        assert_eq!(ui.confirm(), None);
        assert_eq!(ui.mode(), InputMode::Normal);
    }

    #[test]
    fn confirming_twice_runs_the_push_once() {
        // The second `y` arrives after the mode has already moved to Pushing.
        // It must not produce a second command.
        let mut ui = asking();
        assert!(ui.confirm().is_some());
        assert_eq!(ui.confirm(), None, "a second confirm must not push again");
    }

    #[test]
    fn cancelling_clears_the_question_without_a_notice() {
        let mut ui = asking();
        ui.cancel();
        assert_eq!(ui.mode(), InputMode::Normal);
        assert_eq!(
            ui.overlay(tall_pane(80)).rows(),
            0,
            "a cancelled prompt leaves nothing behind"
        );
    }

    #[test]
    fn a_successful_push_reports_what_it_did() {
        // The wording comes from the plan, so a create reports itself as a
        // create rather than as a generic success.
        let mut ui = asking();
        ui.confirm();
        ui.finished(PushOutcome {
            success: true,
            output: "To /tmp/origin\n * [new branch] gsw-push -> gsw-push\n".to_string(),
        });
        assert_eq!(ui.mode(), InputMode::Normal);
        assert!(ui
            .overlay(tall_pane(80))
            .text()
            .contains("Created origin/gsw-push"));
    }

    #[test]
    fn a_successful_update_counts_what_it_pushed() {
        let mut ui = PushUi::new();
        ui.request(&snapshot(tracked(3)));
        ui.confirm();
        ui.finished(PushOutcome {
            success: true,
            output: String::new(),
        });
        assert!(ui
            .overlay(tall_pane(80))
            .text()
            .contains("Pushed 3 commits to origin/gsw-push"));
    }

    #[test]
    fn a_failed_push_shows_what_git_said() {
        // The whole point of the feature's error path: git's own words, not a
        // gsw paraphrase.
        let mut ui = asking();
        ui.confirm();
        ui.finished(PushOutcome {
            success: false,
            output: "To /tmp/origin\n ! [rejected] gsw-push -> gsw-push (fetch first)\n\
                     error: failed to push some refs to '/tmp/origin'\n"
                .to_string(),
        });
        assert_eq!(ui.mode(), InputMode::Normal);
        let overlay = ui.overlay(tall_pane(120)).text();
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
        ui.confirm();
        ui.finished(PushOutcome {
            success: false,
            output: "To /tmp/origin\n\
                     hint: Updates were rejected because the tip is behind\n\
                     hint: its remote counterpart. Integrate the changes\n\
                     hint: before pushing again.\n\
                     ! [rejected] gsw-push -> gsw-push (fetch first)\n\
                     error: failed to push some refs\n"
                .to_string(),
        });
        let overlay = ui.overlay(tall_pane(120)).text();
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
    fn a_failed_push_never_takes_more_than_three_rows() {
        // The frame below is what the user is watching. A wall of git output
        // must not push it off the screen.
        let mut ui = asking();
        ui.confirm();
        let output = (1..=20)
            .map(|n| format!("error: line {n}\n"))
            .collect::<String>();
        ui.finished(PushOutcome {
            success: false,
            output,
        });
        assert_eq!(ui.overlay(tall_pane(80)).rows(), MAX_STATUS_ROWS);
        assert_eq!(
            ui.overlay(tall_pane(80)).text().lines().count(),
            MAX_STATUS_ROWS
        );
    }

    #[test]
    fn a_message_taller_than_the_pane_keeps_the_rows_the_frame_can_spare() {
        // Three rows of error in a three-row pane: the frame is laid out to
        // fill the pane exactly, so every row the overlay takes is a row the
        // frame gave up, and the last one is not the frame's to give.
        let mut ui = asking();
        ui.confirm();
        ui.finished(PushOutcome {
            success: false,
            output: "To /tmp/origin\n\
                     ! [rejected] gsw-push -> gsw-push (fetch first)\n\
                     error: failed to push some refs\n"
                .to_string(),
        });
        let overlay = ui.overlay(Dimensions {
            width: 80,
            height: 3,
        });
        assert_eq!(overlay.rows(), 2, "the frame keeps the third row");
        // git leads with the part worth reading, so the tail is what goes.
        let text = overlay.text();
        assert!(text.contains("To /tmp/origin"), "got {text:?}");
        assert!(
            !text.contains("error: failed to push some refs"),
            "the last line is the one to drop, got {text:?}",
        );
    }

    #[test]
    fn a_one_row_pane_is_all_frame() {
        // Nothing is left to overlay onto, and a pane showing only a question
        // with no frame under it is not watch mode.
        let ui = asking();
        let overlay = ui.overlay(Dimensions {
            width: 80,
            height: 1,
        });
        assert_eq!(overlay.rows(), 0);
        assert_eq!(overlay.text(), "");
    }

    #[test]
    fn the_row_count_always_matches_the_text() {
        // The whole reason these are one call: a count that disagrees with the
        // text either leaves a blank strip under the frame or paints past the
        // bottom of the pane.
        let mut ui = asking();
        ui.confirm();
        ui.finished(PushOutcome {
            success: false,
            output: (1..=20)
                .map(|n| format!("error: line {n}\n"))
                .collect::<String>(),
        });
        for height in 0..8 {
            let overlay = ui.overlay(Dimensions { width: 80, height });
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

    #[test]
    fn a_failed_push_that_said_nothing_still_says_something() {
        // A push that fails with no output at all must not leave a blank row
        // that reads as success.
        let mut ui = asking();
        ui.confirm();
        ui.finished(PushOutcome {
            success: false,
            output: "   \n\n".to_string(),
        });
        assert_eq!(ui.overlay(tall_pane(80)).rows(), 1);
        assert!(
            !ui.overlay(tall_pane(80)).text().trim().is_empty(),
            "a failure must always say that it failed",
        );
    }

    #[test]
    fn a_status_stays_until_a_key_arrives() {
        // Tim's requirement: an error must survive every decay tick and
        // repaint, and go away only when the user has pressed something.
        let mut ui = asking();
        ui.confirm();
        ui.finished(PushOutcome {
            success: false,
            output: "error: failed to push some refs\n".to_string(),
        });
        assert_eq!(ui.overlay(tall_pane(80)).rows(), 1);
        ui.dismiss();
        assert_eq!(
            ui.overlay(tall_pane(80)).rows(),
            0,
            "a key press clears the message"
        );
        assert_eq!(ui.overlay(tall_pane(80)).text(), "");
    }

    #[test]
    fn dismissing_leaves_a_question_and_a_running_push_alone() {
        // `dismiss` is what an unrelated key does. It must not answer a
        // question or hide a push that is still running.
        let mut ui = asking();
        ui.dismiss();
        assert_eq!(ui.mode(), InputMode::Confirm, "a stray key must not cancel");

        ui.confirm();
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
        ui.confirm();
        let overlay = ui.overlay(tall_pane(80)).text();
        assert!(
            overlay.to_lowercase().contains("push"),
            "the running notice must name what is happening, got {overlay:?}",
        );
    }

    #[test]
    fn the_overlay_never_exceeds_the_width_it_is_given() {
        // gsw's standing contract: nothing it prints wraps. A long remote name
        // or a long git error must be truncated, not folded onto a new row.
        let mut ui = PushUi::new();
        let mut snap = snapshot(None);
        snap.branch = "a-branch-name-long-enough-to-need-truncating-on-a-narrow-pane".to_string();
        ui.request(&snap);
        for width in [10, 20, 40] {
            let overlay = ui.overlay(tall_pane(width)).text();
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
        let mut ui = PushUi::new();
        let mut snap = snapshot(None);
        snap.branch = "日本語のブランチ名-🎉-café".to_string();
        ui.request(&snap);
        for width in 1..40 {
            let overlay = ui.overlay(tall_pane(width)).text();
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
        ui.confirm();
        ui.finished(PushOutcome {
            success: false,
            output: "error: failed to push some refs\n".to_string(),
        });
        ui.request(&snapshot(None));
        assert_eq!(ui.mode(), InputMode::Confirm);
        assert_eq!(ui.overlay(tall_pane(80)).rows(), 1);
        assert!(!ui.overlay(tall_pane(120)).text().contains("error:"));
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

        let outcome = run_push(
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

        let outcome = run_push(&confirmed(&["push"]), p);
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

        let outcome = run_push(&confirmed(&["push"]), p);
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
        let outcome = run_push(
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
        let outcome = run_push(&confirmed(&["push", "--no-such-flag"]), clone.path());
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
        let outcome = run_push(
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
    /// git execs. Unix-only because `/dev/tty` is — Windows has no such device and
    /// nothing to deny access to.
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

            let outcome = run_push(&confirmed(&["push", "tty-probe", "feature"]), p);
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

        let outcome = run_push(&command, p);

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

        let outcome = run_push(&command, p);

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

        let outcome = run_push(&command, p);

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
