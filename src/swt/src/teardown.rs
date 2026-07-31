//! teardown — the worktree that exists but has not passed its check yet, the
//! signals that would otherwise orphan it, and the one place that decides what
//! becomes of both.
//!
//! `swt create` builds the worktree *before* it verifies it, because the only
//! honest place to run the check is inside a clean checkout of HEAD. That
//! ordering opens a window — often minutes long, since the check is a full
//! build-and-test — in which a worktree and a branch exist that nobody has agreed
//! to keep. Whoever ends the run during that window owes the user a teardown,
//! and there is more than one such party: `create` itself when the check comes
//! back red, an unwind out of the check, and the signal handling `swt` installs
//! so a Ctrl-C is not simply an orphaned worktree.
//!
//! So the responsibility is registered here rather than carried on a stack: a
//! process-global holder, taken by [`hold_unverified_worktree`] and given up by
//! exactly one of [`WorktreeHold::keep`] (the check passed — it is the caller's
//! worktree now) or [`remove_unverified_worktree`] (it never earned its place).
//! The registry is what makes a second caller safe: the hold is cleared *before*
//! the git commands run, so teardown happens once no matter how many parties
//! reach it, and it is what lets a party with no access to the stack — a signal
//! handler — find the worktree at all. The stack still gets a say: the hold is a
//! guard, so an unwind past it is a teardown nobody had to write a handler for.
//!
//! # What a signal means
//!
//! Left alone, a terminating signal's default disposition kills the process
//! outright, which is precisely how a worktree that never passed its check — or
//! a merge lock — outlives its owner. So while `swt` owns something orphanable,
//! [`TERMINATION_SIGNALS`] mean teardown first and then an exit with the
//! conventional 128 + signal status. Three properties of that are load-bearing:
//!
//! - **It is scoped.** [`arm_signal_teardown`] is called when a responsibility is
//!   taken, never before, and a signal arriving with nothing at risk is handed
//!   straight back to the default handler — so `swt` never makes a signal mean
//!   something new in a window where there is nothing to protect.
//! - **It runs on a thread, not in a handler.** Teardown takes mutexes and spawns
//!   git, none of which is async-signal-safe. A dedicated thread reading the
//!   signals turns them into ordinary code, which is what makes the work legal.
//! - **A second signal cannot truncate it.** One thread reads every signal, and
//!   it never comes back for the next one: the first ends in `process::exit`.
//!   Teardown's git commands run outside `swt`'s process group (see
//!   [`remove_worktree`]), so the repeat a held-down Ctrl-C sends cannot kill
//!   them either, and [`TEARDOWN_SEQUENCE`] keeps any other party from exiting
//!   the process out from under a teardown in flight.
//!
//! SIGKILL is the exception no program is allowed to handle, and `swt` does not
//! pretend otherwise.
//!
//! # The one case the exit status is not `swt`'s to decide
//!
//! A terminal aims Ctrl-C at the whole foreground process group, which contains
//! the green check `swt` is blocked on. That check dies, `swt`'s ordinary path
//! wakes up, reads the dead check as a red one, tears the worktree down itself
//! and exits with *its* status. Nothing is orphaned — same teardown, latched so
//! it happens once — but such a run may report 1 rather than 130, because the
//! work the signal interrupted got to the exit first. A signal aimed at `swt`
//! alone leaves the check running, and there this module is the only way the run
//! can end.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

#[cfg(unix)]
use {
    crate::lock::{holds_any_lock, release_all_held_locks},
    signal_hook::consts::{SIGINT, SIGTERM},
    signal_hook::iterator::Signals,
    signal_hook::low_level::emulate_default_handler,
    std::os::raw::c_int,
    std::process,
    std::sync::OnceLock,
    std::thread,
};

use crate::git::remove_worktree;
use crate::green_check::Outcome;

/// The signals `swt` turns into a teardown followed by an ordinary exit.
///
/// SIGINT is the terminal's Ctrl-C and SIGTERM the polite kill. SIGKILL is the
/// one no program is allowed to handle, and `swt` does not pretend otherwise.
#[cfg(unix)]
const TERMINATION_SIGNALS: [c_int; 2] = [SIGINT, SIGTERM];

/// The conventional shell status for a death by signal: 128 plus the signal
/// number, so SIGINT is 130 and SIGTERM is 143.
#[cfg(unix)]
const SIGNAL_EXIT_BASE: i32 = 128;

/// What a termination signal means to `swt` at the moment it arrives.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposition {
    /// `swt` owns nothing the signal would orphan, so it must behave exactly as
    /// it would have if `swt` had never installed a handler at all.
    Default,
    /// Something would be orphaned: tear it down, then exit with this status.
    Terminate(i32),
}

/// Name the signal-handling thread carries, so it is recognizable in a debugger
/// or a crash report rather than being one more anonymous worker.
#[cfg(unix)]
const SIGNAL_THREAD_NAME: &str = "swt-signal-teardown";

/// Decides what a termination signal means, given whether anything `swt` owns
/// would be orphaned by it.
///
/// `signal` is the signal that arrived and `at_risk` whether this process
/// currently holds an unverified worktree or a lock file. Kept free of the
/// registries it is asked about so the decision can be pinned on its own.
#[cfg(unix)]
fn signal_disposition(signal: c_int, at_risk: bool) -> Disposition {
    if at_risk {
        Disposition::Terminate(SIGNAL_EXIT_BASE + signal)
    } else {
        Disposition::Default
    }
}

/// Whether this process currently owns anything a signal would orphan: the two
/// registries that outlive a stack, asked in one place so the answer cannot
/// disagree with itself.
#[cfg(unix)]
fn anything_at_risk() -> bool {
    unverified_worktree().is_some() || holds_any_lock()
}

/// Acts on one termination signal, on the signal thread and never in a handler.
///
/// Both branches are terminal by design: either the process dies by the signal
/// exactly as it would have without `swt`, or it dies by `swt`'s hand after the
/// teardown it owed. Nothing returns to read a second signal, which is what
/// makes an impatient repeat harmless.
#[cfg(unix)]
fn handle_signal(signal: c_int) {
    match signal_disposition(signal, anything_at_risk()) {
        // Best effort: a signal the table does not recognize leaves the process
        // running, which is the same thing it would have done with a handler
        // that had nothing to do.
        Disposition::Default => drop(emulate_default_handler(signal)),
        Disposition::Terminate(status) => {
            // Taken before the work and held across the exit: any other party
            // reaching a teardown — `create` on a red check, a guard unwinding —
            // waits here rather than racing this one out of the process.
            let _sequence = teardown_sequence();
            drop(remove_held_worktree());
            release_all_held_locks();
            process::exit(status);
        }
    }
}

/// Installs `swt`'s signal teardown, once per process.
///
/// Called the moment a responsibility is taken — an unverified worktree, a lock
/// file — and never before, so an interruption arriving while `swt` owns nothing
/// keeps its default disposition rather than being routed through machinery with
/// nothing to do. It stays installed afterwards, which costs nothing: a signal
/// arriving once everything has been given up is handed back to the default
/// handler by [`handle_signal`].
///
/// Best effort by design. If the signals cannot be registered, or the thread
/// that reads them cannot be spawned, the run continues under the default
/// disposition — the state it was in a moment ago — rather than failing over a
/// safety net. The spawn failure is safe precisely because the registration
/// travels with the closure: dropping it unregisters, so a registered signal can
/// never be left with nobody reading it, which would silently *ignore* it.
#[cfg(unix)]
pub fn arm_signal_teardown() {
    /// Latches the installation, so every later responsibility finds it done.
    static ARMED: OnceLock<()> = OnceLock::new();

    ARMED.get_or_init(|| {
        let Ok(mut signals) = Signals::new(TERMINATION_SIGNALS) else {
            return;
        };
        drop(
            thread::Builder::new()
                .name(SIGNAL_THREAD_NAME.to_string())
                .spawn(move || {
                    for signal in &mut signals {
                        handle_signal(signal);
                    }
                }),
        );
    });
}

/// Installs `swt`'s signal teardown — a documented no-op off Unix.
///
/// Signal dispositions, process groups and the 128 + signal convention are all
/// POSIX notions, and everything else in `swt` already assumes a POSIX `sh`.
/// Claiming a protection that cannot be delivered would be worse than saying so:
/// the callers are identical on both platforms, and off Unix a terminated run
/// leaves its worktree behind.
#[cfg(not(unix))]
pub fn arm_signal_teardown() {}

/// A worktree that exists but has not passed its green check yet: the three
/// facts teardown needs and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UnverifiedWorktree {
    /// Repository worktree to run git from — never the one being removed.
    root: PathBuf,
    /// The worktree directory that would have to be deleted.
    path: PathBuf,
    /// The branch checked out in it, which would have to be deleted too.
    branch: String,
}

/// The unverified worktree this process is on the hook for, if any.
///
/// Process-global because the responsibility is: the party that ends the run is
/// not necessarily the one that created the worktree, and it has no stack to
/// find it on.
static UNVERIFIED_WORKTREE: Mutex<Option<UnverifiedWorktree>> = Mutex::new(None);

/// Borrows the holder, ignoring poisoning.
///
/// Refusing to tear a worktree down because another thread panicked while
/// holding this mutex for the length of one assignment would leave exactly the
/// orphan this module exists to prevent. There is no half-updated state to
/// protect against either: every mutation is a whole-value store or take.
fn unverified_worktree() -> MutexGuard<'static, Option<UnverifiedWorktree>> {
    UNVERIFIED_WORKTREE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Held for the length of one teardown, so two of them cannot interleave and —
/// more importantly — so nobody can end the process in the middle of one.
///
/// The latch below already keeps the *work* to a single run. This is the other
/// half: the signal thread holds this across its `process::exit`, so a party
/// that would otherwise have exited first waits here instead of cutting a
/// teardown in flight in half. Deliberately not reentrant-safe — a std [`Mutex`]
/// is not — so nothing taken under it may take it again.
static TEARDOWN_SEQUENCE: Mutex<()> = Mutex::new(());

/// Claims the teardown sequence, ignoring poisoning.
///
/// A thread that panicked mid-teardown has left work undone, which is a reason
/// to carry on rather than to refuse: the alternative is the orphan this module
/// exists to prevent.
fn teardown_sequence() -> MutexGuard<'static, ()> {
    TEARDOWN_SEQUENCE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// One caller's claim on an unverified worktree, given up by exactly one of
/// [`WorktreeHold::keep`] and being dropped.
///
/// The guard carries no data of its own — the worktree lives in the registry
/// above, where the signal teardown can also find it — but its lifetime is what
/// covers the path no registry can: an unwind. A panic anywhere inside the
/// check would otherwise leave the worktree and its branch behind with nobody
/// left to remove them.
#[must_use = "dropping the hold immediately tears the worktree down again"]
pub struct WorktreeHold {
    /// Nothing to hold; the private field is what keeps a hold from being
    /// forged outside this module, where it would authorize a teardown.
    _private: (),
}

impl WorktreeHold {
    /// Gives up the responsibility without removing anything: the check passed,
    /// so the worktree stays.
    ///
    /// Consuming, because keeping is final — a verified worktree belongs to the
    /// caller who asked for it, and no later signal may take it away.
    pub fn keep(self) {
        // Clearing the registry is the whole of it. The drop that follows finds
        // nothing to remove, which is why keeping needs no second flag to
        // suppress it: there is one source of truth, and this empties it.
        *unverified_worktree() = None;
    }
}

impl Drop for WorktreeHold {
    /// Tears the worktree down unless somebody has already said what should
    /// become of it.
    ///
    /// This is what covers the path nobody writes a handler for: a panic between
    /// the hold and the keep unwinds through here, and the half-built worktree
    /// goes with it. Silent by design — the paths that can explain *why* a
    /// worktree is going away report it themselves, and an unwind has a panic
    /// message of its own to carry the news.
    fn drop(&mut self) {
        drop(remove_unverified_worktree());
    }
}

/// Takes responsibility for a worktree that exists but is not verified yet.
///
/// Call this the moment `git worktree add` returns, before the check starts: the
/// window this covers is precisely the one where a worktree and a branch exist
/// that nobody has agreed to keep. The returned [`WorktreeHold`] ends that
/// window whichever way the run goes — [`WorktreeHold::keep`] on a green check,
/// its [`Drop`] on a panic, and [`remove_unverified_worktree`] when a caller
/// wants to report what the teardown actually did.
///
/// `root` is the repository worktree teardown would run git from, `path` the
/// worktree directory that would be removed, and `branch` the branch checked out
/// in it.
pub fn hold_unverified_worktree(root: &Path, path: &Path, branch: &str) -> WorktreeHold {
    *unverified_worktree() = Some(UnverifiedWorktree {
        root: root.to_path_buf(),
        path: path.to_path_buf(),
        branch: branch.to_string(),
    });
    // There is now something to orphan, so this is the moment a signal starts
    // meaning something other than "die where you stand".
    arm_signal_teardown();
    WorktreeHold { _private: () }
}

/// Tears down the held worktree and its branch, if one is still held.
///
/// Returns the teardown outcome, or `None` when there was nothing left to
/// remove. Teardown is best-effort by nature, so the outcome is *reported*
/// rather than assumed: callers are expected to print what actually happened
/// instead of claiming a cleanup they did not verify.
///
/// # Contract for the signal handling that also calls this
///
/// This is deliberately built to be reached three times over, because it will
/// be: `create` calls it explicitly when the check comes back red, the hold's
/// [`Drop`] calls it on the way out of the scope, and `swt`'s signal teardown
/// calls it on the path that ends the process.
///
/// - **Latched.** The hold is cleared *before* the git commands run, so the
///   second caller finds nothing to do rather than repeating a removal — which
///   would otherwise report a spurious failure over a directory the first call
///   had already deleted, or delete a successor run's worktree of the same name.
/// - **Sequenced.** A second caller *waits* for a teardown in flight instead of
///   walking past it. That is the point: the waiting party is usually somebody
///   who was about to end the process, and letting it through would truncate the
///   teardown between the two git commands.
/// - **Not async-signal-safe.** It takes mutexes and spawns processes, so it
///   must be called from ordinary code — a signal-handling thread, or an exit
///   path — never from inside a raw `signal(2)` handler.
#[must_use]
pub fn remove_unverified_worktree() -> Option<Outcome> {
    let _sequence = teardown_sequence();
    remove_held_worktree()
}

/// Tears the held worktree down, for a caller that already holds the teardown
/// sequence.
///
/// Split out because [`TEARDOWN_SEQUENCE`] is a std [`Mutex`] and therefore not
/// reentrant: the signal path takes the sequence, does this, releases the locks
/// and exits, all as one uninterruptible step, and would deadlock on itself if
/// it went through the public entrance.
///
/// Returns the teardown outcome, or `None` when there was nothing left to
/// remove.
fn remove_held_worktree() -> Option<Outcome> {
    // The latch. Taking the hold out of the registry — and letting go of *that*
    // mutex — happens before a single git command runs, so whoever gets here
    // first carries the teardown through to the end while everybody after them
    // finds nothing to do rather than repeating it.
    let held = unverified_worktree().take()?;
    Some(remove_worktree(&held.root, &held.path, &held.branch))
}

#[cfg(test)]
mod tests {
    use super::{hold_unverified_worktree, remove_unverified_worktree, UnverifiedWorktree};
    #[cfg(unix)]
    use super::{signal_disposition, Disposition, SIGINT, SIGTERM};
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Serializes these tests against each other. The holder under test is
    /// process-global and cargo runs one binary's tests in threads, so two of
    /// them left to overlap would take each other's worktree.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// Claims the registry for one test.
    fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A worktree triple no git command can act on and no other run can name.
    ///
    /// The root does not exist, so both teardown commands fail to spawn at all:
    /// these tests are about the *registry* — who is held and who is released —
    /// and must never be able to touch a real repository to find that out.
    fn unreachable_worktree() -> UnverifiedWorktree {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/nonexistent-swt-teardown-{}-{nanos}",
            std::process::id()
        ));
        UnverifiedWorktree {
            path: root.join("subagent.swt"),
            branch: format!("swt/never-existed-{}-{nanos}", std::process::id()),
            root,
        }
    }

    #[test]
    fn nothing_held_means_nothing_to_remove() {
        let _serial = serial();
        assert!(
            remove_unverified_worktree().is_none(),
            "a teardown with no hold must report that there was nothing to do"
        );
    }

    // The hold is what a red check and a Ctrl-C both act on, and it has to be
    // acted on exactly once: a second teardown would report a spurious failure
    // over a directory the first one already deleted.
    #[test]
    fn a_held_worktree_is_torn_down_once_and_the_hold_is_latched() {
        let _serial = serial();
        let held = unreachable_worktree();
        // Bound, not dropped on the spot: the hold *is* a guard, so a temporary
        // would tear the worktree down before the assertions could ask.
        let _hold = hold_unverified_worktree(&held.root, &held.path, &held.branch);

        assert!(
            remove_unverified_worktree().is_some(),
            "the held worktree must be torn down and the attempt reported"
        );
        assert!(
            remove_unverified_worktree().is_none(),
            "the hold must be cleared before the work, so a second caller is a no-op"
        );
    }

    // The other way out of the window: the check passed, so the worktree is the
    // caller's now and no later signal may take it away. That nothing is
    // *removed* is pinned end to end by `swt create`'s happy path, which finds
    // the worktree still there.
    #[test]
    fn keeping_a_worktree_releases_the_hold() {
        let _serial = serial();
        let held = unreachable_worktree();
        let hold = hold_unverified_worktree(&held.root, &held.path, &held.branch);

        hold.keep();

        assert!(
            remove_unverified_worktree().is_none(),
            "a kept worktree must no longer be swt's to tear down"
        );
    }

    // A signal `swt` owns nothing for must behave exactly as it would have if
    // `swt` had never touched the process's signal handling: anything else means
    // a Ctrl-C in a window where there is nothing to clean up behaves
    // differently for no reason, which is a worse tool, not a safer one.
    #[cfg(unix)]
    #[test]
    fn a_signal_is_left_to_its_default_disposition_when_nothing_is_at_risk() {
        for signal in [SIGINT, SIGTERM] {
            assert_eq!(
                signal_disposition(signal, false),
                Disposition::Default,
                "signal {signal} with nothing at risk"
            );
        }
    }

    // With something at risk the same signal has to become a teardown and the
    // conventional 128 + signal status, so a caller can still tell a Ctrl-C from
    // a red check.
    #[cfg(unix)]
    #[test]
    fn a_signal_with_something_at_risk_becomes_a_teardown_and_128_plus_the_signal() {
        assert_eq!(
            signal_disposition(SIGINT, true),
            Disposition::Terminate(130),
            "SIGINT is 128 + 2"
        );
        assert_eq!(
            signal_disposition(SIGTERM, true),
            Disposition::Terminate(143),
            "SIGTERM is 128 + 15"
        );
    }
}
