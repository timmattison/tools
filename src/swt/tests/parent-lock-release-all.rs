//! `release_all_held_locks`, the release path no destructor can cover.
//!
//! A process that exits from inside a locked region unwinds nothing, so the lock
//! guard's `Drop` never runs. The registry of locks *this* process created is
//! what the signal teardown drains instead, and it is also what keeps that drain
//! honest: a lock file this process did not create is never removed, and a guard
//! still on the stack afterwards must not remove a successor's lock either.
//!
//! **This lives in its own test binary on purpose.** The registry is
//! process-global, so a `release_all_held_locks` call would yank the lock out
//! from under any test holding one concurrently in the same binary — cargo runs
//! a binary's tests in threads. One process, one test, no bystanders.

mod support;

use std::fs::File;

use support::TestRepo;
use swt::lock::{release_all_held_locks, with_parent_lock};

/// Basename of the lock file inside the repository's shared git directory.
const LOCK_FILE: &str = "swt.lock";

#[test]
fn it_drops_the_locks_this_process_holds_and_never_touches_any_other_file() {
    let repo = TestRepo::new();
    let held = repo.path().join(".git").join(LOCK_FILE);

    // A lock file this process never created, in a repository it has no claim
    // on. Standing in for another `swt` run's live lock.
    let other_repo = TestRepo::new();
    let foreign = other_repo.path().join(".git").join(LOCK_FILE);
    File::create(&foreign).expect("foreign lock fixture");

    with_parent_lock(repo.path(), || {
        assert!(
            held.exists(),
            "fixture precondition: the region should hold a lock at {}",
            held.display()
        );

        release_all_held_locks();

        assert!(
            !held.exists(),
            "a lock this process holds must be dropped on the way out"
        );
        assert!(
            foreign.exists(),
            "a lock this process never created must never be removed"
        );

        // With the lock given up, another `swt` is free to take it. Standing one
        // in proves the guard still on this stack cannot delete a file it no
        // longer owns.
        File::create(&held).expect("successor lock fixture");
    });

    assert!(
        held.exists(),
        "the outgoing guard removed a lock it had already given up"
    );
    assert!(foreign.exists(), "a foreign lock was removed after all");
}
