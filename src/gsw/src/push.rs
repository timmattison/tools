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
use crate::lines::LineSplitter;
use crate::render::{truncate_right, Snapshot, UpstreamStatus};
use crate::repo::DETACHED_HEAD;
use crate::watch::{Dimensions, InputMode};

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
    let _ = on_line;
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

    // One thread per pipe, and both must run at once. A pipe holds a fixed
    // number of bytes, so a reader that waits its turn lets the other pipe
    // fill, and a child blocked writing into a full pipe never exits — which
    // is the deadlock a single-threaded read of two streams always eventually
    // finds. Scoped threads because `on_line` is borrowed, not owned: it is
    // `Sync`, so both threads can call it, and the scope is what proves to the
    // compiler that neither outlives the borrow.
    let (stderr_text, stdout_text) = std::thread::scope(|scope| {
        let errors = scope.spawn(|| drain(child_stderr, on_line));
        let output = scope.spawn(|| drain(child_stdout, on_line));
        // A panicking reader loses its own text and nothing else. The
        // alternative is to carry the panic out of `run_push`, which runs on
        // the push thread — and a push thread that dies never calls
        // `on_finish`, so the monitor would sit in the pushing mode for the
        // rest of the session over a callback that misbehaved.
        (
            errors.join().unwrap_or_default(),
            output.join().unwrap_or_default(),
        )
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

    // stderr first: `git push` reports what it did — `To <remote>`, the ref
    // updates, and every rejection — on stderr, and writes to stdout only under
    // flags gsw does not pass. A hook writes to both, and git passes each
    // through to its own stream rather than merging them, so the two halves are
    // joined here in the order that puts git's own account first.
    let mut text = stderr_text;
    text.push_str(&stdout_text);

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

/// Read one of the child's pipes to the end, reporting each line as it lands
/// and returning everything the pipe carried.
///
/// The two jobs are one pass on purpose. The live window wants each line at the
/// moment it arrives, and [`PushOutcome`] wants the whole text at the end. Read
/// twice they would be two different accounts of one stream, and the pipe only
/// gives its bytes up once anyway.
///
/// `stream` is an `Option` because [`std::process::Child`]'s handles are, and a
/// missing pipe is treated as an empty one: it cannot happen for a child
/// configured with [`Stdio::piped`], and inventing an error message for it
/// would put words in git's mouth.
///
/// A read error ends the drain with what was read so far. The child is on the
/// other end of a pipe that is about to close anyway, and the exit status —
/// which is what decides success — is read from the child itself.
fn drain(stream: Option<impl std::io::Read>, on_line: &(dyn Fn(String) + Sync)) -> String {
    let Some(mut stream) = stream else {
        return String::new();
    };

    let mut splitter = LineSplitter::new();
    let mut collected = String::new();
    let mut buffer = [0_u8; 8192];
    let report = |line: String, collected: &mut String| {
        collected.push_str(&line);
        collected.push('\n');
        on_line(line);
    };

    loop {
        match stream.read(&mut buffer) {
            // End of file: the child closed this pipe.
            Ok(0) => break,
            Ok(read) => {
                for line in splitter.feed(&buffer[..read]) {
                    report(line, &mut collected);
                }
            }
            // A signal arrived mid-read. Nothing was lost and nothing is wrong.
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }

    // A hook that exits without a trailing newline still said something.
    if let Some(line) = splitter.finish() {
        report(line, &mut collected);
    }

    collected
}

/// Arrange for `command`'s child to run detached from the terminal, so nothing
/// in its process tree can reach the keyboard gsw is reading.
///
/// Each platform names the terminal differently, so each gets its own arm: a
/// session of its own on Unix, no inherited console on Windows. Both deny the
/// same thing — the direct path to the terminal device, the one a closed stdin
/// and captured output streams do not cover, because it bypasses the inherited
/// descriptors entirely.
///
/// The Unix half asks for a new session before the exec. A session leader has
/// no controlling terminal until it deliberately acquires one, and no program
/// git runs does that — so `open("/dev/tty")` returns `ENXIO` for the child,
/// for ssh, and for every credential helper below them. Refused it, OpenSSH
/// sets `use_askpass` and either runs `SSH_ASKPASS` (a GUI prompt, which is
/// fine — it does not touch the pane gsw is drawing on) or gives up at once
/// with a message that reaches the status rows.
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

/// `CreateProcess`'s `DETACHED_PROCESS`: the new process does not inherit the
/// console of the process that started it, and Windows will not give it one of
/// its own. Deliberately not combined with `CREATE_NEW_CONSOLE`, which is its
/// opposite and which `CreateProcess` rejects alongside it, and not written as
/// `CREATE_NO_WINDOW`, which only hides a console the child still has and can
/// still read from.
///
/// Spelled out here rather than pulled in from `windows-sys` or `winapi`: it is
/// one integer fixed by the Win32 ABI, and a dependency the whole crate would
/// carry for it is a worse trade than a constant with its value written down.
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;

/// See the Unix half above. Windows has no `/dev/tty`, but it has the same
/// hazard by a different door: a console. OpenSSH for Windows asks for a key
/// passphrase or an unknown-host-key answer by opening `CONIN$` itself, which
/// reaches the console the child inherited no matter what was done to its
/// standard handles — so a closed stdin and captured output streams leave the
/// push free to paint a prompt over gsw's alternate screen and race the event
/// thread for the user's keystrokes, with no timeout to end it.
///
/// [`DETACHED_PROCESS`] closes that door the way `setsid` closes the Unix one.
/// The child inherits no console and cannot be assigned one, so `CONIN$` and
/// `CONOUT$` fail to open for it, for ssh, and for every credential helper
/// below them. Denied the console, OpenSSH does what it does on Unix: it falls
/// back to `SSH_ASKPASS` (a GUI prompt, which does not touch the pane gsw is
/// drawing on) or fails immediately with a message that arrives on the captured
/// stderr and lands in the status rows like any other error. The pipes are
/// unaffected — the flag governs the console, not the standard handles, which
/// the caller has already set.
///
/// **Not covered by any test in this repository.** The Unix half has a runtime
/// test, `the_push_child_cannot_open_the_controlling_terminal`, which plants a
/// fake ssh and asserts the child was refused the terminal. There is no Windows
/// equivalent: no Windows host runs these tests and this repository has no CI,
/// so such a test would be one nobody has ever seen pass or fail. This arm is
/// verified by compiling for `x86_64-pc-windows-msvc` and by nothing else.
#[cfg(windows)]
fn detach_from_terminal(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    command.creation_flags(DETACHED_PROCESS);
}

/// Neither Unix nor Windows, so there is no terminal this code knows how to
/// take away. The arm exists so the crate still builds for such a target rather
/// than failing to find `detach_from_terminal`; on one, `GIT_TERMINAL_PROMPT=0`
/// and the closed stdin are the whole defense.
#[cfg(not(any(unix, windows)))]
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
        creates_remote_branch: bool,
        command: PushCommand,
        success_message: String,
    },
    /// `git push` is running, and this is what it has said so far.
    Running {
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
}

/// How long a [`State::Status`] message stays under the frame, and how it is
/// drawn while it does.
///
/// The split is between what gsw said and what git said, and it is one decision
/// rather than two because the two halves are the same fact. Everything gsw
/// composes itself — a push that worked, a push it would not run — is a
/// *report*: the user pressed a key, the answer came back, and a monitor that
/// holds it on screen for the rest of the session is spending a row on news.
/// git's error text is a *remedy*: the user has to read it and act on it, so
/// gsw must not take it away while they are looking at another pane.
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
            Self::UntilDismissed => None,
            Self::Fading { posted_at } => Some(now.saturating_duration_since(*posted_at)),
        }
    }
}

impl PushUi {
    /// A UI with nothing on screen, drawing on a terminal that takes 24-bit
    /// color when `truecolor` says so.
    pub(crate) fn new(truecolor: bool) -> Self {
        Self {
            state: State::Idle,
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
    /// A question, a push in flight, and an error that never expires all say
    /// `None`: none of them changes with the clock, and a wake-up costs a
    /// repaint of the whole pane.
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
            State::Idle | State::Status { .. } | State::Asking { .. } => None,
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
        self.state = match prompt_for(
            &snapshot.branch,
            snapshot.push_remote.as_deref(),
            snapshot.upstream.as_ref(),
        ) {
            // Nowhere to put the question, so it is not asked. Idle rather than
            // a message: see above.
            PushPrompt::Confirm { .. } if Overlay::rows_to_spare(dims) == 0 => State::Idle,
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
            // A refusal describes the repository as it stood when `p` was
            // pressed, so it goes stale exactly the way a success does — and
            // costs the frame the same row until it does.
            PushPrompt::Refuse { message } => State::Status {
                lines: vec![message],
                life: Life::Fading { posted_at: now },
            },
        };
    }

    /// Handle `y`: start the push, returning the [`PushCommand`] to run, or
    /// `None` when no confirmation was on screen to accept.
    ///
    /// Moving to [`State::Running`] as it hands the command over is what makes
    /// a second `y` — one that raced the mode change — return `None` rather than
    /// start an overlapping push.
    pub(crate) fn confirm(&mut self, now: Instant) -> Option<PushCommand> {
        let State::Asking {
            command,
            success_message,
            ..
        } = std::mem::replace(&mut self.state, State::Idle)
        else {
            return None;
        };
        self.state = State::Running {
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

    /// Handle a key with no other meaning: clear a status message if one is up.
    /// Leaves a question or a running push alone — neither is the user's to
    /// dismiss by pressing an unrelated key.
    pub(crate) fn dismiss(&mut self) {
        if matches!(self.state, State::Status { .. }) {
            self.state = State::Idle;
        }
    }

    /// What the push feature shows in a pane of `dims`: the lines that fit
    /// there, how many rows the frame must give up to make room for them
    /// ([`Overlay::rows`]), and how many rows the frame keeps
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
            State::Running {
                started_at, recent, ..
            } => {
                let elapsed = now.saturating_duration_since(*started_at);
                let notice = format!("{RUNNING_NOTICE} ({})", format_age_detailed(elapsed));
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
                let elapsed = life.elapsed(now);
                // Which rows go when the pane cannot hold them all, decided
                // here rather than by the clamp at the end of this function.
                // That clamp drops from the end, and the end is where a
                // failure's reason is — see [`failure_lines`]. The running
                // window sizes itself first for the same reason.
                let dropped = lines.len().saturating_sub(Overlay::rows_to_spare(dims));
                // The age goes on the last row, which for every message that
                // has one is the only row: a success and a refusal are one
                // sentence each, and git's several-line error text is the one
                // kind that never ages. Numbered before the drop above, so the
                // row that carries it is the message's last and not merely the
                // last one that fitted.
                let last = lines.len().saturating_sub(1);
                lines
                    .iter()
                    .enumerate()
                    .skip(dropped)
                    .map(|(row, line)| match elapsed {
                        // Appended *before* the truncation, so the age is part
                        // of what the pane has to fit rather than something
                        // added to a row already measured against its width.
                        Some(elapsed) => {
                            let line = if row == last {
                                format!("{line} ({} ago)", format_age_detailed(elapsed))
                            } else {
                                line.clone()
                            };
                            colorize_status(&truncate_right(&line, width), elapsed, self.truecolor)
                                .to_string()
                        }
                        None => truncate_right(line, width).red().to_string(),
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
/// subtract [`Overlay::rows`] from the pane height itself, in another module —
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
///
/// The same reasoning is why a question this hint cannot be drawn with is never
/// raised (see [`PushUi::request`], and [`PushUi::overlay`] for the pane that
/// shrinks under one). What makes Enter safe to bind to a push is that the user
/// is looking at the sentence saying so. Off the screen, the binding keeps the
/// risk and loses the sentence.
const CONFIRM_HINT: &str = "[y/Enter = push, n/Esc = cancel]";

/// What the window's rows are indented by.
///
/// The indent and the dim together say these rows are the child process
/// speaking rather than gsw, which matters because the notice above them and
/// the frame below them are both gsw's own words.
const WINDOW_INDENT: &str = "  ";

/// What a running push says while the network round trip is in flight.
const RUNNING_NOTICE: &str = "Pushing…";

/// How long a status message gsw wrote itself stays under the frame.
///
/// It is also the length of the fade, so the message reaches black exactly as
/// it is removed and nothing ever blinks out at full brightness.
const STATUS_LIFETIME: Duration = Duration::from_secs(60);

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
        testcolor::strip_ansi(&testcolor::with_forced_ansi(|| ui.overlay(dims, now).text()))
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
            overlay.contains(CONFIRM_HINT),
            "the overlay owns the key hint, got {overlay:?}",
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
                .contains(CONFIRM_HINT),
            "a refusal must not offer keys that do nothing",
        );
    }

    #[test]
    fn confirming_hands_back_the_command_and_switches_to_pushing() {
        let mut ui = asking();
        let command = ui.confirm(t0()).expect("a question on screen must confirm");
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
        assert_eq!(ui.confirm(t0()), None, "a second confirm must not push again");
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
        ui.confirm(t0());
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
            // Stated rather than inherited: a developer with `core.hooksPath`
            // set globally would otherwise run their own hooks here, and the
            // test would pass or fail on a machine's configuration.
            git(p, &["config", "core.hooksPath", ".git/hooks"]);

            // Inside the git dir rather than the worktree, so the gate cannot
            // appear as an untracked file in the repository under test.
            let gate = p.join(".git").join("gate");
            let hook = p.join(".git").join("hooks").join("pre-push");
            std::fs::create_dir_all(hook.parent().expect("the hook has a parent"))
                .expect("create the hooks directory");
            std::fs::write(
                &hook,
                format!(
                    r#"#!/bin/sh
echo "{FIRST_LINE}"
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
            )
            .expect("write the pre-push hook");
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
                .expect("make the hook executable");

            let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let recorded = Arc::clone(&seen);
            let gate_path = gate.clone();
            let outcome = run_push(
                &confirmed(&["push", "-u", "origin", "feature"]),
                p,
                &move |line: String| {
                    if line == FIRST_LINE {
                        std::fs::write(&gate_path, b"open").expect("open the gate");
                    }
                    recorded.lock().expect("the record lock is never poisoned").push(line);
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
