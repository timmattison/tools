//! lock — the file that stops two `swt merge` runs interleaving in one
//! repository, and the guarantee that it is always let go of again.
//!
//! Three things are load-bearing here and none of them is obvious from the
//! outside.
//!
//! **Where the lock lives.** It is `swt.lock` inside the git directory *shared*
//! by every worktree of the repository, which is what `git rev-parse
//! --git-common-dir` names. Not `<worktree>/.git/swt.lock`: `.git` is a
//! directory only in the main worktree, and in a linked worktree — the only
//! place the workflow `swt` exists for ever merges from — it is a regular file
//! holding `gitdir: …`, so joining onto it is an `ENOTDIR`. The common dir is
//! also exactly the serialization *scope* wanted: two `swt merge` runs launched
//! from two different worktrees of one repository must contend for the same
//! file.
//!
//! **That it is always released.** [`LockGuard`]'s [`Drop`] covers a region that
//! returns and a region that panics; [`release_all_held_locks`] covers the third
//! path, a process that exits from inside the region without unwinding anything.
//! A process-global registry of the locks *this* process created is what makes
//! that last one safe — a path that is not in the registry belongs to somebody
//! else, and removing it would hand two merges the same repository.
//!
//! **That a reap only ever removes the file it judged.** A lock older than the
//! staleness window is presumed abandoned, and deleting it is the one place
//! `swt` removes a file it did not create. The verdict comes from an mtime,
//! which names no particular file, so every lock carries its creator's
//! [`UniqueToken`] and [`reap_corpse`] confirms that token is still there
//! immediately before the unlink: a corpse cleared and replaced by a live
//! successor in the meantime is left alone rather than deleted out from under
//! its holder. Two adjacent syscalls still separate that confirmation from the
//! unlink, which no portable filesystem call closes; what the token removes is
//! the far wider window that used to run from the staleness verdict all the way
//! to the delete.

#[cfg(test)]
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::create::UniqueToken;
use crate::git::git_must;
use crate::teardown::arm_signal_teardown;

/// Basename of the lock file, inside the repository's shared git directory.
const LOCK_FILE: &str = "swt.lock";

/// The git query that names the directory shared by every worktree of a repo.
const GIT_COMMON_DIR_ARGS: [&str; 2] = ["rev-parse", "--git-common-dir"];

/// What `swt` says when it gives up waiting for somebody else's merge.
const TIMEOUT_MESSAGE: &str = "Timed out waiting for parent repo lock.\n";

/// Status `swt` exits with when it cannot take the lock. The conventional "this
/// did not work" status, distinct from the `2` reserved for a usage error.
const LOCK_FAILURE_EXIT_STATUS: i32 = 1;

/// Lock files this process created and is still responsible for removing.
///
/// Process-global because the responsibility is: teardown triggered from
/// anywhere has to be able to find every lock this process owns, whichever call
/// created it. It is also what makes removal *safe* — see [`release`].
static HELD_LOCKS: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// Borrows the registry, ignoring poisoning.
///
/// Refusing to release a lock because some other thread panicked while holding
/// this mutex for the length of one set operation would leave a stale lock
/// behind for an hour — precisely the failure this module exists to prevent.
/// There is no half-updated state to protect against either: every mutation is
/// a single insert, removal or take.
fn held_locks() -> MutexGuard<'static, BTreeSet<PathBuf>> {
    HELD_LOCKS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Gives up one lock: deregisters it and, only if it was still registered,
/// removes the file.
///
/// The registry check is the whole safety property. [`release_all_held_locks`]
/// may already have dropped this lock on the way out of the process, after which
/// another `swt` is free to create its own file at the same path — removing that
/// one would hand two merges the same repository.
fn release(path: &Path) {
    let was_ours = held_locks().remove(path);
    if was_ours {
        // Best effort: a lock that is already gone is the state we wanted.
        let _ = fs::remove_file(path);
    }
}

/// Removes every lock file this process is currently holding, and never one it
/// did not create.
///
/// This is the third release path, the one no destructor can cover: a process
/// that exits from inside a locked region unwinds nothing, so [`LockGuard`]'s
/// [`Drop`] never runs and the lock outlives its owner — blocking every later
/// merge in that repository until the staleness reap an hour later.
///
/// # Contract for the signal handling that calls this
///
/// `swt`'s signal teardown is expected to call this on every path that ends the
/// process, and it is built to be called that way:
///
/// - **Idempotent and latched.** The registry is emptied up front, so a second
///   call — a signal arriving after an explicit release — finds nothing left to
///   do rather than repeating work.
/// - **Safe to interleave with a live region.** A [`LockGuard`] still on the
///   stack when this runs will find its path already gone from the registry and
///   remove nothing, so a lock a *successor* process has since taken is never
///   deleted out from under it.
/// - **Not async-signal-safe.** It takes a mutex and touches the filesystem, so
///   it must be called from ordinary code — a signal-handling thread, or an
///   exit hook — never from inside a raw `signal(2)` handler.
pub fn release_all_held_locks() {
    // Latched by taking the paths out of the registry up front.
    let paths = std::mem::take(&mut *held_locks());
    for path in paths {
        // Best effort: the process is going down and there is nothing useful to
        // report to.
        let _ = fs::remove_file(&path);
    }
}

/// Reports whether this process is holding any lock file right now.
///
/// The signal teardown asks before it decides what a signal means: a lock this
/// process created is one of the two things a signal would orphan, and a lock
/// left behind blocks every later merge in that repository until the staleness
/// reap an hour later. The answer is also what keeps `swt` from changing what a
/// signal means in a window where it owns nothing.
pub(crate) fn holds_any_lock() -> bool {
    !held_locks().is_empty()
}

/// Ownership of one lock file, released on every path out of the locked region
/// that unwinds — a normal return and a panic alike.
struct LockGuard {
    /// The lock file this guard is responsible for.
    path: PathBuf,
}

impl LockGuard {
    /// Records a freshly created lock as this process's responsibility.
    ///
    /// `path` must be a lock file this process just created with `O_EXCL`;
    /// registering somebody else's would authorize deleting it.
    fn hold(path: PathBuf) -> Self {
        // Armed before the registration, never after: the other order leaves an
        // instant in which the registry names a lock and no thread is reading the
        // signals that would release it. Arming early is free — a signal arriving
        // before the insert finds nothing at risk and keeps its default
        // disposition.
        arm_signal_teardown();
        // Past this insert there is something a signal would orphan, and this is
        // the more expensive of the two: a leaked lock blocks the whole
        // repository.
        held_locks().insert(path.clone());
        Self { path }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        release(&self.path);
    }
}

/// The three durations that govern the acquisition loop.
///
/// Separated from the loop so a test can drive it in milliseconds. At the values
/// `swt` actually ships with, none of the three is reachable by a test: a
/// ten-minute timeout, an hour-long staleness window and a one-second backoff
/// cannot be waited out.
#[derive(Debug, Clone, Copy)]
struct LockTimings {
    /// Age past which an existing lock is presumed abandoned and reaped. Long
    /// enough that no honest merge is ever mistaken for a corpse.
    stale_after: Duration,
    /// How long to keep waiting for somebody else's lock before giving up.
    wait_at_most: Duration,
    /// Pause between acquisition attempts.
    retry_every: Duration,
}

impl LockTimings {
    /// The values `swt` runs with.
    const PRODUCTION: Self = Self {
        stale_after: Duration::from_secs(60 * 60),
        wait_at_most: Duration::from_secs(10 * 60),
        retry_every: Duration::from_secs(1),
    };
}

/// Why a locked region never ran.
///
/// Reported as a value rather than acted on, so the *caller* decides what it
/// means — which is also what lets both cases be asserted in a test without
/// taking the test binary down with them.
#[derive(Debug)]
enum LockFailure {
    /// Somebody else held the lock for longer than [`LockTimings::wait_at_most`].
    TimedOut,
    /// The lock file could not be created for a reason other than "it already
    /// exists" — a missing git directory, a read-only filesystem — so waiting
    /// would never help.
    Unusable(io::Error),
}

impl LockFailure {
    /// The message `swt` prints before giving up.
    ///
    /// `lock_path` is the file that could not be taken; it is named only in the
    /// case where naming it helps, since a timeout is about contention rather
    /// than about the path.
    fn message(&self, lock_path: &Path) -> String {
        match self {
            Self::TimedOut => TIMEOUT_MESSAGE.to_string(),
            Self::Unusable(err) => format!(
                "Could not create the parent repo lock at {}: {err}\n",
                lock_path.display()
            ),
        }
    }
}

// Test-only interposition point inside the stale-lock reap, sitting between
// the verdict that a lock is a corpse and the removal that acts on that
// verdict.
//
// The interleaving the reap has to survive — a corpse cleared and replaced by a
// live successor's lock in the gap between two adjacent syscalls — cannot be
// produced by sleeping and hoping, and a probabilistic test of it would be a
// flake that blocks unrelated commits. A test installs a closure here instead
// and stands the successor's lock up at exactly the wrong moment.
//
// Thread local rather than process global so two tests running concurrently in
// one binary cannot see each other's interposer, and `#[cfg(test)]` so the
// shipped binary carries neither the slot nor a branch on it.
#[cfg(test)]
thread_local! {
    static REAP_INTERPOSER: RefCell<Option<Box<dyn FnMut()>>> = const { RefCell::new(None) };
}

/// Runs the test interposer, if this thread installed one.
///
/// Compiles to an empty function outside tests, where there is nothing to
/// install it.
fn interpose_before_reap() {
    #[cfg(test)]
    REAP_INTERPOSER.with(|slot| {
        if let Some(interposer) = slot.borrow_mut().as_mut() {
            interposer();
        }
    });
}

/// Reads the token whoever created a lock file wrote into it.
///
/// `None` means no owner could be established at all: the file has gone, or it
/// cannot be read. A `None` never matches anything, not even another `None`, so
/// a lock this process cannot identify is waited out rather than removed.
///
/// An *empty* answer is a real identity rather than a missing one. It is what
/// every lock an older `swt` created carries, and it matches itself, so an
/// abandoned old-format lock — or one whose token was lost to a failed write —
/// is still reaped instead of blocking the repository forever.
fn lock_owner(lock_path: &Path) -> Option<Vec<u8>> {
    fs::read(lock_path).ok()
}

/// Removes a lock judged stale, but only while the file at that path is still
/// the one that was judged.
///
/// `owner` is the token read *before* the staleness verdict; the file's token is
/// read once more here and the unlink happens only if the two agree. Returns
/// whether the path was freed, so a refused reap falls through to the ordinary
/// backoff rather than spinning on a lock it will keep declining to remove.
fn reap_corpse(lock_path: &Path, owner: Option<&[u8]>) -> bool {
    interpose_before_reap();
    // A lock whose owner could not be read is never removed. An mtime says
    // nothing about *which* file it belongs to, and this is the one place `swt`
    // deletes a file it did not create, so an unidentifiable one is left for the
    // wait to time out on and report.
    let Some(owner) = owner else {
        return false;
    };
    if lock_owner(lock_path).as_deref() != Some(owner) {
        // Somebody stood a different lock up in the corpse's place between the
        // staleness verdict and here. It is not a corpse; go back to waiting.
        return false;
    }
    match fs::remove_file(lock_path) {
        Ok(()) => true,
        // Already gone: another waiter reaped the same corpse. The path is free
        // either way, which is all this answer is asked about.
        Err(err) => err.kind() == ErrorKind::NotFound,
    }
}

/// Resolves the lock file that serializes merges for a repository.
///
/// `repo_root` is any worktree root of the repository. Returns the absolute path
/// of that repository's one merge lock: from the main worktree git answers with
/// a path relative to its cwd (`.git`), which is resolved against the root, and
/// from a linked worktree it answers with the shared git directory's absolute
/// path, which [`Path::join`] takes whole. Either way both worktrees name one
/// file.
///
/// Exits the process with git's own message if git cannot answer — which is
/// safe here precisely because it happens before anything is locked.
fn parent_lock_path(repo_root: &Path) -> PathBuf {
    // Asking git rather than assuming a layout is the whole point: `.git` is a
    // directory only in the main worktree. `join` resolves the relative answer
    // against the root and takes the absolute one whole, which is exactly the
    // `resolve(repoRoot, commonDir)` the original performs.
    let common_dir = git_must(GIT_COMMON_DIR_ARGS, Some(repo_root));
    repo_root.join(common_dir).join(LOCK_FILE)
}

/// Runs `f` while holding a lock file, retrying until it can be created.
///
/// The internal entrance every timing decision goes through, so a test can drive
/// the loop in milliseconds instead of waiting out the shipped values.
///
/// `lock_path` is the file whose existence *is* the lock, `timings` the
/// durations to run the loop at, and `f` the work to perform under it. Returns
/// `f`'s value, or the reason the region never ran.
fn locked<T>(
    lock_path: &Path,
    timings: LockTimings,
    f: impl FnOnce() -> T,
) -> Result<T, LockFailure> {
    let start = Instant::now();
    loop {
        // `create_new` is `O_CREAT | O_EXCL`: the kernel decides who wins, so
        // two processes reaching here at the same instant cannot both succeed.
        // Anything short of that — probing for the file and then creating it —
        // has a window between the two calls, and that window is the bug.
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)
        {
            Ok(mut file) => {
                // The lock is the file's *existence*, not the open handle, so
                // the handle is let go as soon as the owner token is in it:
                // keeping it would only add a second thing to get right on the
                // release path, and a process that dies still leaves the file
                // behind either way — which is what the staleness reap below is
                // for, and what the token in it makes safe.
                let _guard = LockGuard::hold(lock_path.to_path_buf());
                // Deliberately after the guard, and deliberately best effort. A
                // token that could not be written costs this lock only its name
                // in somebody's future reap — it still excludes, and it reads
                // back as the old format, which is reapable. Failing out here
                // instead would leak the lock and block the repository.
                let _ = file.write_all(UniqueToken::mint().to_string().as_bytes());
                drop(file);
                return Ok(f());
            }
            // Somebody else holds it. Fall through and wait.
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
            // Not contention: a missing git directory, a read-only filesystem.
            // Waiting would never help, so this is reported straight away
            // rather than sat out as if it were somebody else's merge.
            Err(err) => return Err(LockFailure::Unusable(err)),
        }

        // Reap a lock old enough to be a corpse rather than a merge in
        // progress. A lock that vanishes under the stat is simply retried:
        // whoever held it has just let go.
        //
        // The owner is read *first*, ahead of the staleness verdict, and
        // confirmed again inside the reap. Reading it afterwards would defeat
        // the whole check — a successor's token would be compared against
        // itself and match.
        let owner = lock_owner(lock_path);
        if let Ok(metadata) = fs::metadata(lock_path) {
            let abandoned = metadata
                .modified()
                .ok()
                // An mtime in the future yields no elapsed duration, and is
                // treated as "not stale" — the conservative answer, since
                // reaping a live lock hands two merges the same repository.
                .and_then(|mtime| SystemTime::now().duration_since(mtime).ok())
                .is_some_and(|age| age > timings.stale_after);
            // Only a freed path skips the backoff: a reap that was refused
            // because the lock changed underneath it has something live to wait
            // on, not a corpse to clear.
            if abandoned && reap_corpse(lock_path, owner.as_deref()) {
                continue;
            }
        }

        if start.elapsed() > timings.wait_at_most {
            return Err(LockFailure::TimedOut);
        }
        thread::sleep(timings.retry_every);
    }
}

/// Runs `f` while holding the parent repository's merge lock, so concurrent
/// `swt merge` runs against one repository are serialized.
///
/// The lock is released when `f` returns, when it panics, and — via
/// [`release_all_held_locks`] — when the process exits from inside the region.
/// Contention is waited out with a one-second backoff for up to ten minutes; a
/// lock older than an hour is presumed abandoned and reaped, but only while it
/// is still demonstrably the same lock that was judged. Giving up writes
/// [`TIMEOUT_MESSAGE`] to stderr and exits, which is safe because it happens
/// while holding nothing.
///
/// # Nothing inside the locked region may exit the process
///
/// An exit skips the [`Drop`] that releases the lock, and the failure most
/// likely to tempt one — a rebase conflict — is the very case the region exists
/// to handle. A lock leaked there blocks every later merge in the repository
/// until the staleness reap an hour later. So the region must *return* its
/// outcome and let the caller exit out here, after the lock is gone.
/// [`git_must`](crate::git::git_must) is banned inside it for the same reason:
/// it exits on failure.
///
/// `repo_root` is the root of any worktree of the parent repository, and `f` the
/// work to perform under the lock. Returns whatever `f` returns.
pub fn with_parent_lock<T>(repo_root: &Path, f: impl FnOnce() -> T) -> T {
    let lock_path = parent_lock_path(repo_root);
    match locked(&lock_path, LockTimings::PRODUCTION, f) {
        Ok(value) => value,
        // Reported by `locked` rather than acted on there, so this exit happens
        // out here — holding nothing, with no region left to cut short.
        Err(failure) => {
            eprint!("{}", failure.message(&lock_path));
            process::exit(LOCK_FAILURE_EXIT_STATUS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{locked, LockFailure, LockTimings, LOCK_FILE, REAP_INTERPOSER};
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime};
    use tempfile::TempDir;

    /// How long a contended acquisition is given before it is called a timeout.
    /// Generous enough to survive a loaded machine, short enough to fail fast.
    const TEST_TIMEOUT: Duration = Duration::from_millis(400);

    /// Backoff between attempts in tests — short enough that a waiter notices a
    /// release promptly, long enough not to spin.
    const TEST_BACKOFF: Duration = Duration::from_millis(5);

    /// A staleness window nothing in a test is old enough to fall past.
    const NEVER_STALE: Duration = Duration::from_secs(60 * 60);

    /// Timings for a test that expects to acquire, possibly after waiting.
    const FAST: LockTimings = LockTimings {
        stale_after: NEVER_STALE,
        // Mutual exclusion has to outlast the deliberate hold below on a loaded
        // machine; giving up early would fail the test for the wrong reason.
        wait_at_most: Duration::from_secs(30),
        retry_every: TEST_BACKOFF,
    };

    /// How long the first holder stays inside its region. Long enough that a
    /// second acquisition running unserialized would visibly interleave.
    const HOLD: Duration = Duration::from_millis(150);

    /// Timings for a test about the staleness reap.
    ///
    /// The staleness window is deliberately *longer* than the wait: a lock
    /// stood up while a test runs has to stay a live lock for the rest of it,
    /// and a window shorter than the wait would let it age into a corpse and be
    /// reaped for a reason the test is not about.
    const REAPING: LockTimings = LockTimings {
        stale_after: Duration::from_secs(1),
        wait_at_most: TEST_TIMEOUT,
        retry_every: TEST_BACKOFF,
    };

    /// Timings for a reap test that has to resolve on its first pass.
    ///
    /// A zero wait means the acquisition loop tries exactly once: either the
    /// reap reports the path free and the immediate retry takes it, or the call
    /// gives up. That turns "how long did this take" into "which branch did it
    /// take", so the answer is a decision rather than a race against a clock.
    /// The staleness window is [`REAPING`]'s, for the reason given there.
    const ONE_PASS: LockTimings = LockTimings {
        stale_after: REAPING.stale_after,
        wait_at_most: Duration::ZERO,
        retry_every: TEST_BACKOFF,
    };

    /// How far back a fixture corpse's mtime is set — far past [`REAPING`]'s
    /// window, so it is reaped on the first pass and nothing about these tests
    /// depends on how long they take to run.
    const ABANDONED_FOR: Duration = Duration::from_secs(60);

    /// The token a fixture corpse carries, and the token a *different* run's
    /// lock carries. Two acquisitions are distinguishable exactly when the
    /// tokens in their lock files differ, so these must never be equal.
    const CORPSE_OWNER: &[u8] = b"corpse-run-token";
    const SUCCESSOR_OWNER: &[u8] = b"successor-run-token";

    /// What an older `swt` left in the lock file it created: nothing at all.
    const NO_OWNER: &[u8] = b"";

    /// A private directory for one test's lock file.
    ///
    /// Every path comes from a fresh [`TempDir`], never a fixed name: two copies
    /// of this test binary run concurrently in this repository, and a shared
    /// lock path would have them contend for real.
    fn lock_dir() -> TempDir {
        tempfile::Builder::new()
            .prefix("swt-lock-")
            .tempdir()
            .expect("lock fixture temp dir")
    }

    /// Creates a lock file carrying `owner` and backdates its mtime by `age`,
    /// standing in for one a run that died left behind.
    ///
    /// The write comes first and the backdating last, because writing is itself
    /// what sets an mtime: the other order would leave the fixture brand new.
    fn aged_lock(path: &Path, owner: &[u8], age: Duration) {
        let mut file = File::create(path).expect("lock fixture");
        file.write_all(owner).expect("the lock fixture's owner");
        let when = SystemTime::now()
            .checked_sub(age)
            .expect("backdated mtime is after the epoch");
        file.set_modified(when).expect("backdate the lock fixture");
    }

    /// Runs `body` with `interposer` installed at the reap's interleaving
    /// point, then clears the slot so a later test on this thread cannot
    /// inherit it.
    fn with_reap_interposer<T>(interposer: impl FnMut() + 'static, body: impl FnOnce() -> T) -> T {
        REAP_INTERPOSER.with(|slot| *slot.borrow_mut() = Some(Box::new(interposer)));
        let outcome = body();
        REAP_INTERPOSER.with(|slot| *slot.borrow_mut() = None);
        outcome
    }

    #[test]
    fn a_free_lock_is_taken_for_the_region_and_dropped_after_it() {
        let dir = lock_dir();
        let lock = dir.path().join(LOCK_FILE);

        let returned = locked(&lock, FAST, || {
            assert!(
                lock.exists(),
                "the lock file must exist for as long as the region runs"
            );
            "region value"
        })
        .expect("an uncontended lock must be acquired");

        assert_eq!(
            returned, "region value",
            "the region's value must come back"
        );
        assert!(!lock.exists(), "the lock outlived its region");
    }

    // A lock somebody else is still holding is not a corpse. Reaping it early is
    // the one failure that would silently hand two merges the same repository,
    // so a fresh lock has to be *waited* on — and the wait has to end in a
    // reported timeout rather than in an acquisition.
    #[test]
    fn a_fresh_lock_is_waited_on_and_the_wait_ends_in_a_reported_timeout() {
        let dir = lock_dir();
        let lock = dir.path().join(LOCK_FILE);
        File::create(&lock).expect("held lock fixture");

        let timings = LockTimings {
            stale_after: NEVER_STALE,
            wait_at_most: TEST_TIMEOUT,
            retry_every: TEST_BACKOFF,
        };
        let ran = AtomicBool::new(false);
        let started = Instant::now();
        let result = locked(&lock, timings, || ran.store(true, Ordering::SeqCst));

        assert!(
            matches!(result, Err(LockFailure::TimedOut)),
            "a lock held by somebody else must be reported as a timeout, got {result:?}"
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "the region ran without holding the lock"
        );
        assert!(
            lock.exists(),
            "a fresh lock must never be reaped out from under its holder"
        );
        assert!(
            started.elapsed() >= TEST_TIMEOUT,
            "gave up after {:?}, before the wait was up",
            started.elapsed()
        );
    }

    // The other half: a lock nobody released because its owner died must not
    // block the repository forever.
    #[test]
    fn a_lock_older_than_the_staleness_window_is_reaped() {
        let dir = lock_dir();
        let lock = dir.path().join(LOCK_FILE);
        aged_lock(&lock, CORPSE_OWNER, ABANDONED_FOR);

        let ran = locked(&lock, REAPING, || true).expect("a stale lock must be reaped");

        assert!(ran, "the region never ran");
        assert!(!lock.exists(), "the lock outlived its region");
    }

    // The reap can only tell a corpse from a successor if the lock file says
    // who created it, so every acquisition writes its own token in.
    #[test]
    fn the_lock_file_names_the_run_holding_it() {
        let dir = lock_dir();
        let lock = dir.path().join(LOCK_FILE);

        let owner = locked(&lock, FAST, || fs::read(&lock).expect("read the held lock"))
            .expect("an uncontended lock must be acquired");

        assert!(
            !owner.is_empty(),
            "the lock file names no owner, so a reap cannot tell a corpse from a successor"
        );
    }

    // The race the owner token exists for. A corpse is judged stale, somebody
    // clears it, and a competitor wins the `O_EXCL` race for the freed path —
    // all before this run's unlink. Removing by path alone deletes that
    // successor's brand new, entirely legitimate lock and then takes the lock,
    // putting two merges inside one repository.
    #[test]
    fn a_corpse_replaced_by_a_live_lock_before_the_unlink_is_not_reaped() {
        let dir = lock_dir();
        let lock = dir.path().join(LOCK_FILE);
        aged_lock(&lock, CORPSE_OWNER, ABANDONED_FOR);

        // The interleaving, made deterministic rather than raced for: the
        // successor's lock is stood up at exactly the point where the reap has
        // decided to delete and has not yet done so.
        let successor_lock = lock.clone();
        let mut already_replaced = false;
        let result = with_reap_interposer(
            move || {
                if already_replaced {
                    return;
                }
                already_replaced = true;
                fs::remove_file(&successor_lock).expect("clear the corpse");
                fs::write(&successor_lock, SUCCESSOR_OWNER).expect("the successor's lock");
            },
            || locked(&lock, REAPING, || "the region ran"),
        );

        assert!(
            matches!(result, Err(LockFailure::TimedOut)),
            "a live successor's lock was reaped and its region entered anyway, got {result:?}"
        );
        assert_eq!(
            fs::read(&lock).ok().as_deref(),
            Some(SUCCESSOR_OWNER),
            "the successor's lock at {} was deleted out from under it",
            lock.display()
        );
    }

    // The other way that race can land: a competing waiter clears the same
    // corpse and stands nothing up in its place, so the reap arrives to find the
    // path already free. That is the outcome this reap was after, whoever
    // performed the unlink, so the acquisition has to retry the freed path at
    // once rather than sit out a backoff on a lock that is no longer there.
    #[test]
    fn a_corpse_another_waiter_already_cleared_leaves_the_path_free() {
        let dir = lock_dir();
        let lock = dir.path().join(LOCK_FILE);
        aged_lock(&lock, CORPSE_OWNER, ABANDONED_FOR);

        // The competitor's unlink, placed at exactly the point where this reap
        // has decided to delete and has not yet done so — and, unlike the
        // successor case above, leaving no replacement behind.
        let cleared_lock = lock.clone();
        let mut already_cleared = false;
        let result = with_reap_interposer(
            move || {
                if already_cleared {
                    return;
                }
                already_cleared = true;
                fs::remove_file(&cleared_lock).expect("the competing waiter's reap");
            },
            || locked(&lock, ONE_PASS, || "the region ran"),
        );

        assert!(
            matches!(result, Ok("the region ran")),
            "a path another waiter had already freed was waited on instead of taken, got {result:?}"
        );
        assert!(!lock.exists(), "the lock outlived its region");
    }

    // An older `swt` wrote nothing into the lock file it created, and one of
    // those left behind by a dead run still has to be reapable: "names no
    // owner" is an identity like any other, and it matches itself. Otherwise
    // upgrading `swt` would strand every repository holding an old corpse until
    // somebody deleted it by hand.
    #[test]
    fn a_corpse_from_an_older_swt_that_names_no_owner_is_still_reaped() {
        let dir = lock_dir();
        let lock = dir.path().join(LOCK_FILE);
        aged_lock(&lock, NO_OWNER, ABANDONED_FOR);

        let ran = locked(&lock, REAPING, || true).expect("an old-format corpse must be reaped");

        assert!(ran, "the region never ran");
        assert!(!lock.exists(), "the lock outlived its region");
    }

    // Never remove a file whose owner cannot be established at all. Such a lock
    // is waited out and reported instead — bounded by the timeout, so it is a
    // report rather than a deadlock — rather than deleted on the strength of an
    // mtime that says nothing about which file it belongs to.
    #[cfg(unix)]
    #[test]
    fn a_corpse_whose_owner_cannot_be_read_is_reported_rather_than_reaped() {
        use std::os::unix::fs::PermissionsExt;

        let dir = lock_dir();
        let lock = dir.path().join(LOCK_FILE);
        aged_lock(&lock, CORPSE_OWNER, ABANDONED_FOR);
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o000))
            .expect("make the lock fixture unreadable");
        if fs::read(&lock).is_ok() {
            // Root reads a mode 0 file regardless, which would make this an
            // ordinary readable corpse and pin nothing at all.
            return;
        }

        let result = locked(&lock, REAPING, || "the region ran");

        assert!(
            matches!(result, Err(LockFailure::TimedOut)),
            "a lock whose owner could not be read was reaped anyway, got {result:?}"
        );
        assert!(
            lock.exists(),
            "an unidentifiable lock at {} was removed",
            lock.display()
        );
    }

    // Real mutual exclusion, not merely a file that appears and disappears: the
    // second region must not start until the first has finished. Unserialized,
    // the log would read first-in, second-in, second-out, first-out.
    #[test]
    fn a_second_acquisition_waits_until_the_first_releases() {
        let dir = lock_dir();
        let lock = dir.path().join(LOCK_FILE);
        let log: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
        let first_is_inside = AtomicBool::new(false);
        let note = |event: &'static str| log.lock().expect("event log").push(event);

        thread::scope(|scope| {
            let first = scope.spawn(|| {
                locked(&lock, FAST, || {
                    note("first in");
                    first_is_inside.store(true, Ordering::SeqCst);
                    thread::sleep(HOLD);
                    note("first out");
                })
                .expect("the first acquisition is uncontended")
            });

            // Contend only once the first holder is demonstrably inside, so the
            // test pins waiting rather than a race it happened to win.
            while !first_is_inside.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1));
            }

            let second = scope.spawn(|| {
                locked(&lock, FAST, || {
                    note("second in");
                    note("second out");
                })
                .expect("the second acquisition must succeed once the first releases")
            });

            first.join().expect("first thread");
            second.join().expect("second thread");
        });

        assert_eq!(
            *log.lock().expect("event log"),
            vec!["first in", "first out", "second in", "second out"],
            "the two regions interleaved, so nothing was actually excluded"
        );
        assert!(!lock.exists(), "the lock outlived both regions");
    }

    // A lock file cannot be created where there is no directory to create it in,
    // and no amount of waiting will change that — so it is reported straight
    // away rather than waited out as if somebody else held it.
    #[test]
    fn a_lock_that_cannot_be_created_is_reported_without_waiting() {
        let dir = lock_dir();
        let lock: PathBuf = dir.path().join("no-such-directory").join(LOCK_FILE);

        let timings = LockTimings {
            stale_after: NEVER_STALE,
            wait_at_most: Duration::from_secs(30),
            retry_every: TEST_BACKOFF,
        };
        let started = Instant::now();
        let result = locked(&lock, timings, || unreachable!("the region must not run"));

        assert!(
            matches!(result, Err(LockFailure::Unusable(_))),
            "an uncreatable lock is not contention, got {result:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "an uncreatable lock must not be waited out"
        );
    }
}
