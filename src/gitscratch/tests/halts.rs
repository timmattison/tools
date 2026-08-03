//! What the replay does at each kind of rebase halt.
//!
//! A rebase that stops with nothing unmerged has not told you why. It might be
//! a commit that adds nothing to the new base, which costs nothing to drop —
//! or a commit git could not write, which costs the developer their work if it
//! is dropped and reported as a cheap replay. These tests put a replay in each
//! of those states for real and pin the answer it gives.
//!
//! Every state here is reached by making the object database unwritable, which
//! is a Unix permission trick, so the whole suite is Unix-only.
#![cfg(unix)]

use gitscratch::Scratch;
use gitscratch::testing::modify_delete_repo;

/// The replay's whole job is to say what an operation would cost. A commit git
/// could not write costs nothing to `--skip`, and the rebase then finishes
/// happily having thrown that commit away — so the replay would report a
/// plausible number for a branch it never actually replayed. It must refuse to
/// answer instead, and it must say enough for someone to fix it: which commit
/// was about to be dropped, and what git itself said.
#[test]
fn refuses_to_report_a_cost_when_a_staged_resolution_could_not_be_committed() {
    let repo = modify_delete_repo();
    // Read the abbreviation from the same object database the implementation
    // will abbreviate against, so `%h` here and `%h` there agree.
    let dropped_sha = repo.git(&["log", "-1", "--format=%h", "branch"]);
    let dropped_subject = repo.git(&["log", "-1", "--format=%s", "branch"]);
    let objects = std::fs::canonicalize(repo.path().join(".git").join("objects"))
        .expect("canonicalize the object database path");

    let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
    scratch
        .git()
        .run(&["checkout", "-q", "--detach", "branch"])
        .expect("check out the branch detached in the scratch worktree");

    // Sealed only now: adding the worktree and checking out write no objects,
    // but everything before this point would fail against a read-only store.
    let sealed = repo.seal_object_store();
    let result = scratch.replay_rebase("main");
    // Released before a single assertion runs, so a failing one cannot leave a
    // read-only directory behind for the temporary directory to trip over.
    drop(sealed);

    let error = match result {
        Ok(conflicts) => panic!(
            "the replay reported a cost for a commit git never wrote: {conflicts:?}\n\
             a dry run may answer 'expensive' or 'I cannot answer', never 'cheap'"
        ),
        Err(error) => format!("{error:#}"),
    };

    assert!(
        error.contains(&dropped_sha),
        "the error should name the commit that was about to be dropped ({dropped_sha}): {error}"
    );
    assert!(
        error.contains(&dropped_subject),
        "the error should carry the dropped commit's subject ({dropped_subject}): {error}"
    );

    // git says: "error: insufficient permission for adding an object to
    // repository database <path>". The path is interpolated rather than
    // translated, so matching on it is locale-independent - and it is the
    // canonicalized one because macOS resolves a temp dir's /var/... to
    // /private/var/....
    let objects = objects.display().to_string();
    assert!(
        error.contains(&objects),
        "the error should carry git's own message, which names {objects}: {error}"
    );
}
