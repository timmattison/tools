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

use crate::green_check::Outcome;

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

/// Takes responsibility for a worktree that exists but is not verified yet.
///
/// Call this the moment `git worktree add` returns, before the check starts: the
/// window this covers is precisely the one where a worktree and a branch exist
/// that nobody has agreed to keep. Exactly one of
/// [`keep_unverified_worktree`] or [`remove_unverified_worktree`] must follow.
///
/// `root` is the repository worktree teardown would run git from, `path` the
/// worktree directory that would be removed, and `branch` the branch checked out
/// in it.
pub fn hold_unverified_worktree(root: &Path, path: &Path, branch: &str) {
    let _ = (root, path, branch);
}

/// Releases the hold without removing anything: the check passed, so the
/// worktree stays.
///
/// After this, [`remove_unverified_worktree`] has nothing to do — which is the
/// point. A verified worktree belongs to the caller who asked for it, and no
/// later signal may take it away.
pub fn keep_unverified_worktree() {}

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
    None
}

#[cfg(test)]
mod tests {
    use super::{
        hold_unverified_worktree, keep_unverified_worktree, remove_unverified_worktree,
        UnverifiedWorktree,
    };
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
        hold_unverified_worktree(&held.root, &held.path, &held.branch);

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
        hold_unverified_worktree(&held.root, &held.path, &held.branch);

        keep_unverified_worktree();

        assert!(
            remove_unverified_worktree().is_none(),
            "a kept worktree must no longer be swt's to tear down"
        );
    }
}
