//! The parent merge lock, exercised against throwaway git repositories.
//!
//! Two guarantees are pinned here that the unit tests inside the module cannot
//! reach, because both are about the *repository* rather than about the
//! acquisition loop:
//!
//! - **Where the lock file lands.** It belongs in the git directory shared by
//!   every worktree of the repository, so a merge launched from any worktree
//!   contends for the same file. The naive `<worktree>/.git/swt.lock` is wrong
//!   twice over in a linked worktree — it names a different path *and* `.git`
//!   there is a regular file, so writing into it is an `ENOTDIR`.
//! - **That the region always gives it back**, including when it panics.
//!
//! The fixture builds the expected path from its own layout rather than from
//! `swt`'s answer, so these pin the location instead of echoing it back.

mod support;

use std::fs::{self, File};
use std::panic;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use support::TestRepo;
use swt::lock::with_parent_lock;

/// Basename of the lock file inside the repository's shared git directory.
const LOCK_FILE: &str = "swt.lock";

/// How far past the staleness window a fixture lock is backdated. Comfortably
/// beyond the shipped one-hour horizon, so the reap is instant instead of the
/// test parking on the retry backoff.
const LONG_ABANDONED: Duration = Duration::from_secs(2 * 60 * 60);

/// Text a deliberately panicking region panics with.
const BOOM: &str = "the region blew up";

/// Where the lock must land for a fixture repository: `swt.lock` in the git
/// directory shared by every worktree of it, which for the fixture's main
/// worktree is a plain `.git` directory.
fn shared_lock_path(repo: &TestRepo) -> PathBuf {
    repo.path().join(".git").join(LOCK_FILE)
}

/// Creates a lock file and backdates its mtime by `age`, standing in for one a
/// run that died left behind.
fn aged_lock(path: &Path, age: Duration) {
    let file = File::create(path).expect("stale lock fixture");
    let when = SystemTime::now()
        .checked_sub(age)
        .expect("backdated mtime is after the epoch");
    file.set_modified(when).expect("backdate the stale lock");
}

#[test]
fn the_lock_lands_in_the_shared_git_dir_and_lives_only_as_long_as_the_region() {
    let repo = TestRepo::new();
    let lock = shared_lock_path(&repo);
    assert!(
        !lock.exists(),
        "fixture precondition: a fresh repository has no lock"
    );

    let returned = with_parent_lock(repo.path(), || {
        assert!(
            lock.exists(),
            "expected the merge lock at {} while the region runs",
            lock.display()
        );
        "region value"
    });

    assert_eq!(
        returned, "region value",
        "the region's value must come back"
    );
    assert!(
        !lock.exists(),
        "the lock at {} outlived its region",
        lock.display()
    );
}

// A linked worktree is the shape `swt merge` actually runs in — the workflow
// this tool serves never merges from the main repo. Its `.git` is a regular
// file, so a lock path built by joining onto it is both the wrong file and an
// unwritable one.
#[test]
fn a_linked_worktree_locks_the_same_file_as_the_main_repository() {
    let repo = TestRepo::new();
    let worktree = repo.add_worktree("lock");
    assert!(
        fs::metadata(worktree.path.join(".git"))
            .expect("linked worktree .git")
            .is_file(),
        "fixture precondition: a linked worktree's .git must be a regular file"
    );
    let lock = shared_lock_path(&repo);

    let ran = with_parent_lock(&worktree.path, || {
        assert!(
            lock.exists(),
            "a merge from a linked worktree must lock the shared git dir at {}",
            lock.display()
        );
        true
    });

    assert!(ran, "the region never ran");
    assert!(!lock.exists(), "the lock outlived its region");
}

// The serialization scope, from the other side: a lock written where the *main*
// worktree keeps it must be seen by a merge launched from a linked one. Aging it
// past the staleness horizon both proves it was seen and keeps the assertion
// instant.
#[test]
fn a_stale_lock_left_in_the_shared_git_dir_is_seen_and_reaped_from_a_linked_worktree() {
    let repo = TestRepo::new();
    let worktree = repo.add_worktree("stale");
    let lock = shared_lock_path(&repo);
    aged_lock(&lock, LONG_ABANDONED);

    let ran = with_parent_lock(&worktree.path, || true);

    assert!(
        ran,
        "an abandoned lock at {} was never reaped from the linked worktree",
        lock.display()
    );
    assert!(!lock.exists(), "the lock outlived its region");
}

#[test]
fn the_lock_is_released_when_the_region_panics_and_the_panic_still_propagates() {
    let repo = TestRepo::new();
    let root = repo.path().to_path_buf();
    let lock = shared_lock_path(&repo);

    let result = panic::catch_unwind(|| {
        with_parent_lock(&root, || {
            // Asserted from inside so the panic under test unwinds out of a
            // region that really did hold the lock; a region holding nothing
            // would have nothing to leak and would pass vacuously.
            assert!(
                lock.exists(),
                "the region must hold the lock before it panics"
            );
            panic!("{BOOM}");
        });
    });

    let payload = result.expect_err("the panic must propagate out of the locked region");
    let message = payload
        .downcast_ref::<String>()
        .map_or("<not a string payload>", String::as_str);
    assert_eq!(message, BOOM, "some other panic reached the caller");
    assert!(
        !lock.exists(),
        "a panicking region left the lock at {} behind",
        lock.display()
    );
}
