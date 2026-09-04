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
//! survive it: the replay works at all, and it stays attributed to the harness -
//! by the name on the commit and by the time the commit carries. Both halves of
//! the attribution travel in the same six variables, and the date half is the
//! one that leaks quietly: it stamps every commit of a run with one identical
//! timestamp, which reads as an ordinary date.
//!
//! It is its own test binary on purpose. The environment is process-wide, so
//! polluting it deliberately is only safe where nothing else is running — one
//! test, one process. Cargo gives every integration test file its own binary,
//! which is exactly that guarantee.

use gitscratch::testing::conflicting_repo;
use gitscratch::Files;

/// A timestamp no run of this suite can produce on its own, so a commit that
/// carries it can only have taken it from the environment.
///
/// Written in git's raw `<epoch> <timezone>` form, which is the form
/// `--date=raw` prints back byte for byte. Git reformats every other spelling
/// it accepts on the way out - an ISO date set as `+00:00` comes back as `Z` -
/// so an assertion against one of those could only fail to match, which is a
/// test that passes for the wrong reason.
const LEAKED_DATE: &str = "1700000000 +0000";

#[test]
fn replays_under_the_environment_a_git_hook_hands_down() {
    // Exactly what `git commit` exports to a pre-commit hook, measured rather
    // than imagined. The relative index path is the dangerous one: it is valid
    // in the repository being committed to and nowhere else, and a linked
    // worktree's `.git` is a *file*, so anything resolving `.git/index` inside
    // one fails with "not a directory".
    std::env::set_var("GIT_AUTHOR_NAME", "A Developer");
    std::env::set_var("GIT_AUTHOR_EMAIL", "developer@example.com");
    std::env::set_var("GIT_AUTHOR_DATE", LEAKED_DATE);
    std::env::set_var("GIT_INDEX_FILE", ".git/index");
    std::env::set_var("GIT_PREFIX", "");
    // A pre-commit hook is handed the author half only, but the committer half
    // travels the same way - exported by other hooks, and by plenty of CI - and
    // git reads both ahead of any configuration.
    std::env::set_var("GIT_COMMITTER_NAME", "A Developer");
    std::env::set_var("GIT_COMMITTER_EMAIL", "developer@example.com");
    std::env::set_var("GIT_COMMITTER_DATE", LEAKED_DATE);

    // The fixtures build repositories by running git too, so they have to be as
    // immune as the harness is - a fixture that inherits the environment cannot
    // even reach the code under test.
    let repo = conflicting_repo();
    let _elsewhere = repo.add_worktree("left");

    let scratch = repo.scratch("main");
    let git = scratch.testing_git();
    git.run("checkout", &["-q", "--detach", "right"])
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

    // The *committer*, deliberately: a rebase carries each replayed commit's
    // original author across by design, so the author here belongs to whoever
    // wrote the fixture's commit and says nothing about the harness. Who made
    // the new commit is the committer, and that is the harness.
    //
    // The date is read back beside the name, because a hook hands down all six
    // identity variables and the two that carry a time are the quiet half. A
    // name that leaks is visible in `git log`; a date that leaks stamps every
    // commit of a run with one identical timestamp, which reads as an ordinary
    // date and orders nothing.
    let committer = git
        .run("log", &["-1", "--date=raw", "--format=%cn <%ce>|%cd"])
        .expect("read the committer of the commit the replay wrote");
    assert!(
        committer.starts_with("gitscratch <gitscratch@localhost>|"),
        "a replay must stay attributable to the harness that made it, even when the environment \
         it inherited names the developer: {committer}"
    );
    assert!(
        !committer.contains(LEAKED_DATE),
        "a replay must timestamp its commits when it made them, not when an inherited \
         GIT_COMMITTER_DATE says: {committer}"
    );

    // A commit the harness makes from nothing has no original author to
    // inherit, so both halves are its own - which is the shape a consumer's
    // squash step uses, and the one an inherited GIT_AUTHOR_NAME and
    // GIT_AUTHOR_DATE would sign with the developer's name and the timestamp of
    // whatever commit the hook was running for.
    let tree = git
        .run("rev-parse", &["HEAD^{tree}"])
        .expect("read the replayed tree");
    let squashed = git
        .run("commit-tree", &[&tree, "-m", "squash"])
        .expect("make a commit the way a consumer squashes one in");
    let identity = git
        .run(
            "log",
            &[
                "-1",
                "--date=raw",
                "--format=%an <%ae>|%cn <%ce>|%ad|%cd",
                &squashed,
            ],
        )
        .expect("read the identity on the squashed commit");
    assert!(
        identity
            .starts_with("gitscratch <gitscratch@localhost>|gitscratch <gitscratch@localhost>|"),
        "a commit the harness creates is entirely its own, author included: {identity}"
    );
    assert!(
        !identity.contains(LEAKED_DATE),
        "a commit the harness creates carries the time it was made, on both halves, rather than \
         the timestamp of whatever commit the hook was running for: {identity}"
    );
}
