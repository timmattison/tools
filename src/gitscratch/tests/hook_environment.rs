//! What a replay does when it inherits a git environment aimed somewhere else.
//!
//! A tool built on this harness can be invoked from inside a git hook — a
//! pre-commit hook running a test suite is the everyday case — and git hands its
//! hooks an environment describing the commit being made: `GIT_AUTHOR_NAME`,
//! `GIT_AUTHOR_EMAIL` and `GIT_AUTHOR_DATE` naming the developer, and
//! `GIT_INDEX_FILE` pointing at the index being committed from, often as the
//! *relative* path `.git/index`, which silently re-anchors on whatever directory
//! each git command happens to run in. Other hooks add `GIT_DIR` and
//! `GIT_WORK_TREE`.
//!
//! Git resolves that environment in preference to `-c` and to `git config`, so
//! it walks straight through the harness's pinned configuration. This suite runs
//! a whole replay with that environment set and pins the two things that must
//! survive it: the replay works at all, and it stays attributed to the harness.
//!
//! It is its own test binary on purpose. The environment is process-wide, so
//! polluting it deliberately is only safe where nothing else is running — one
//! test, one process. Cargo gives every integration test file its own binary,
//! which is exactly that guarantee.

use gitscratch::testing::conflicting_repo;
use gitscratch::{Files, Scratch};

#[test]
fn replays_under_the_environment_a_git_hook_hands_down() {
    // Exactly what `git commit` exports to a pre-commit hook, measured rather
    // than imagined. The relative index path is the dangerous one: it is valid
    // in the repository being committed to and nowhere else, and a linked
    // worktree's `.git` is a *file*, so anything resolving `.git/index` inside
    // one fails with "not a directory".
    std::env::set_var("GIT_AUTHOR_NAME", "A Developer");
    std::env::set_var("GIT_AUTHOR_EMAIL", "developer@example.com");
    std::env::set_var("GIT_AUTHOR_DATE", "@1700000000 +0000");
    std::env::set_var("GIT_INDEX_FILE", ".git/index");
    std::env::set_var("GIT_PREFIX", "");

    // The fixtures build repositories by running git too, so they have to be as
    // immune as the harness is - a fixture that inherits the environment cannot
    // even reach the code under test.
    let repo = conflicting_repo();
    let _elsewhere = repo.add_worktree("left");

    let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
    let git = scratch.git();
    git.run(&["checkout", "-q", "--detach", "right"])
        .expect("check out the branch detached in the scratch worktree");

    let conflicts = scratch
        .replay_rebase("left")
        .expect("replay the contested branch under a hook's environment");

    // Asserting on the conflict it had to resolve, so this cannot pass by having
    // quietly replayed nothing at all.
    assert_eq!(
        conflicts.files(),
        Files::new(1),
        "the contested file should still have conflicted: {conflicts:?}"
    );

    let author = git
        .run(&["log", "-1", "--format=%an <%ae>"])
        .expect("read the author of the commit the replay wrote");
    assert_eq!(
        author, "gitscratch <gitscratch@localhost>",
        "a replay must stay attributable to the harness that made it, even when the environment \
         it inherited names the developer"
    );
}
