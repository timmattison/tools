//! The hold on an unverified worktree, and the three ways it ends.
//!
//! `swt create` hands the worktree it just built to [`hold_unverified_worktree`]
//! and takes it back only once the check has passed. Between those two points
//! the run can end in a way nobody wrote a handler for — a panic in the middle
//! of the check — and the worktree and its branch would be left behind with no
//! caller left to remove them. The hold is a guard for exactly that reason:
//! unwinding past it is a teardown.
//!
//! **This lives in its own test binary on purpose.** The registry the hold
//! writes to is process-global, and cargo runs one binary's tests in threads, so
//! a second test taking a hold concurrently would take this one's worktree.
//! These three serialize against each other explicitly; nothing else in the
//! suite touches the registry from this process.

mod support;

use std::panic::{self, AssertUnwindSafe};
use std::sync::{Mutex, MutexGuard, PoisonError};

use support::TestRepo;
use swt::teardown::{hold_unverified_worktree, remove_unverified_worktree};

/// The message the panicking fixture panics with, so the assertion that it
/// propagated is about *this* panic and not some other failure.
const FIXTURE_PANIC: &str = "swt fixture panic: the check exploded";

/// Serializes these tests against each other; see the module docs.
static SERIAL: Mutex<()> = Mutex::new(());

/// Claims the process-global registry for one test.
fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

// The gap the guard exists to close. The original wraps its green check in
// `try { … } catch { cleanup(); throw }`; a hold that only ever ended in an
// explicit call would leak the worktree *and* the branch on any panic in the
// check, and leave the user with two things to clean up by hand that they were
// never told about.
#[test]
fn a_panic_while_the_hold_is_live_tears_the_worktree_down_and_still_propagates() {
    let _serial = serial();
    let repo = TestRepo::new();
    let worktree = repo.add_worktree("panicking");
    assert!(
        worktree.path.is_dir(),
        "fixture precondition: the worktree must exist first"
    );

    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let _hold = hold_unverified_worktree(repo.path(), &worktree.path, &worktree.branch);
        panic!("{FIXTURE_PANIC}");
    }));

    assert!(
        outcome.is_err(),
        "the panic must still reach the caller; a teardown is not a recovery"
    );
    assert!(
        !worktree.path.exists(),
        "a panic while the hold was live left an orphaned worktree at {}",
        worktree.path.display()
    );
    assert!(
        repo.branches(&worktree.branch).is_empty(),
        "a panic while the hold was live left an orphaned branch {}",
        worktree.branch
    );
}

// The other way out of the window: the check passed, so the worktree is the
// caller's now. Keeping has to be final — a hold that lingered would let a later
// teardown take away a worktree somebody is already working in.
#[test]
fn keeping_a_hold_leaves_the_worktree_and_nothing_left_to_tear_down() {
    let _serial = serial();
    let repo = TestRepo::new();
    let worktree = repo.add_worktree("kept");

    hold_unverified_worktree(repo.path(), &worktree.path, &worktree.branch).keep();

    assert!(
        worktree.path.is_dir(),
        "keeping a verified worktree removed it anyway: {}",
        worktree.path.display()
    );
    assert!(
        remove_unverified_worktree().is_none(),
        "a kept worktree is no longer swt's to tear down"
    );
    assert!(
        worktree.path.is_dir(),
        "a later teardown removed a worktree that had been kept: {}",
        worktree.path.display()
    );
    assert_eq!(
        repo.branches(&worktree.branch),
        vec![worktree.branch.clone()],
        "a later teardown deleted the branch of a worktree that had been kept"
    );
}

// Both parties reach the teardown on the failing path — `create` asks for it
// explicitly so it can report what happened, and the guard's drop asks again on
// the way out — and between those two calls a *successor* run is free to create
// a worktree at the same path on the same branch. A second teardown would delete
// it, which is why the hold is cleared before the git commands rather than
// after.
#[test]
fn an_explicit_teardown_and_the_guards_drop_remove_exactly_once() {
    let _serial = serial();
    let repo = TestRepo::new();
    let worktree = repo.add_worktree("torn");
    let path = worktree.path.to_string_lossy().into_owned();

    {
        let _hold = hold_unverified_worktree(repo.path(), &worktree.path, &worktree.branch);

        let torn = remove_unverified_worktree().expect("the held worktree should be torn down");
        assert!(torn.ok, "teardown reported failure: {}", torn.out);
        assert!(
            !worktree.path.exists(),
            "the explicit teardown left the worktree at {path}"
        );

        // The successor: another `swt` run, holding the same names.
        repo.git(&[
            "worktree",
            "add",
            "--quiet",
            "-b",
            &worktree.branch,
            &path,
            "HEAD",
        ]);
    }

    assert!(
        worktree.path.is_dir(),
        "the guard's drop repeated a teardown that had already happened, \
         removing a successor's worktree at {path}"
    );
    assert_eq!(
        repo.branches(&worktree.branch),
        vec![worktree.branch.clone()],
        "the guard's drop repeated a teardown, deleting a successor's branch"
    );
}
