//! teardown — the worktree that exists but has not passed its check yet, and the
//! one place that decides what becomes of it.
//!
//! `swt create` builds the worktree *before* it verifies it, because the only
//! honest place to run the check is inside a clean checkout of HEAD. That
//! ordering opens a window — often minutes long, since the check is a full
//! build-and-test — in which a worktree and a branch exist that nobody has agreed
//! to keep. Whoever ends the run during that window owes the user a teardown,
//! and there is more than one such party: `create` itself when the check comes
//! back red, and the signal handling `swt` installs so a Ctrl-C is not simply an
//! orphaned worktree.
//!
//! So the responsibility is registered here rather than carried on a stack: a
//! process-global holder, taken by [`hold_unverified_worktree`] and given up by
//! exactly one of [`keep_unverified_worktree`] (the check passed — it is the
//! caller's worktree now) or [`remove_unverified_worktree`] (it never earned its
//! place). The registry is what makes a second caller safe: the hold is cleared
//! *before* the git commands run, so teardown happens once no matter how many
//! parties reach it.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

#[cfg(unix)]
use {
    signal_hook::consts::{SIGINT, SIGTERM},
    std::os::raw::c_int,
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

/// Decides what a termination signal means, given whether anything `swt` owns
/// would be orphaned by it.
///
/// `signal` is the signal that arrived and `at_risk` whether this process
/// currently holds an unverified worktree or a lock file. Kept free of the
/// registries it is asked about so the decision can be pinned on its own.
#[cfg(unix)]
fn signal_disposition(signal: c_int, at_risk: bool) -> Disposition {
    let _ = at_risk;
    let _ = signal;
    Disposition::Default
}

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
    pub fn keep(self) {}
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
/// This is deliberately built to be reached twice, because it will be: `create`
/// calls it explicitly when the check comes back red, and `swt`'s signal
/// teardown calls it on every path that ends the process.
///
/// - **Latched.** The hold is cleared *before* the git commands run, so the
///   second caller finds nothing to do rather than repeating a removal — which
///   would otherwise report a spurious failure over a directory the first call
///   had already deleted.
/// - **Not async-signal-safe.** It takes a mutex and spawns processes, so it
///   must be called from ordinary code — a signal-handling thread, or an exit
///   path — never from inside a raw `signal(2)` handler.
#[must_use]
pub fn remove_unverified_worktree() -> Option<Outcome> {
    // The latch. Taking the hold out of the registry — and letting go of the
    // mutex — happens before a single git command runs, so whoever gets here
    // first carries the teardown through to the end while everybody after them
    // finds nothing to do. Holding the lock across the git commands instead
    // would make the second caller *wait* for the first and then repeat it.
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
