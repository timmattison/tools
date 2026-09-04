//! `Repo` is the pre-flight a consumer runs before deciding a replay is worth
//! starting, so what it must get right is the *cheap rejection*: a directory
//! that is not a repository and a revision that does not resolve have to fail
//! here, clearly and by name, rather than surfacing later as a simulation that
//! mysteriously failed.
//!
//! Every fixture lives in its own `TempDir`, so concurrent `cargo test` runs
//! never share a path.

use gitscratch::testing::{conflicting_repo, nested_conflict_repo, not_a_repository, TestRepo};
use gitscratch::{Repo, Uncommitted};

/// The whole point of opening a repository up front is that "you pointed me at
/// somewhere that is not a repository" is a different, cheaper answer than "the
/// simulation failed" - so it has to be said in those words.
///
/// The premise arrives through [`not_a_repository`] rather than through a bare
/// `TempDir`, because a bare one only *assumes* the premise: a developer whose
/// `TMPDIR` sits inside a git repository would see this test fail on the
/// `expect_err` below, blaming the pre-flight for accepting a directory that was
/// a repository all along. The fixture probes instead, and names the offending
/// path where the mistake actually is.
#[test]
fn open_rejects_a_directory_that_is_not_a_git_repository() {
    let outside = not_a_repository();

    let error = Repo::open(outside.path()).expect_err("a bare temp dir is not a git repository");

    let message = format!("{error:#}");
    assert!(
        message.contains("not inside a git repository"),
        "the error should say the directory is not a repository: {message}"
    );
    assert!(
        message.contains(&outside.path().display().to_string()),
        "the error should name the directory it was given: {message}"
    );
}

/// Opening a repository has to *lead somewhere*, and the somewhere is a scratch
/// worktree of that same repository.
///
/// This used to assert that `Repo::path()` handed back the directory `open` was
/// given, which was true and worth nothing: the checked path and an unchecked
/// one were the same `&Path`, so the pre-flight validated something and then
/// published a value that carried no trace of having been validated. Every
/// consumer was free to skip it, and `grist` did. `Repo::scratch` is the
/// replacement, so what is worth pinning is that the worktree it builds really
/// is a worktree of the repository that was opened - a door that leads to the
/// wrong room is worse than no door.
///
/// `main` is the fixture's branch, so its commit is the discriminator: a scratch
/// checked out anywhere else, or of anything else, cannot be sitting on it.
#[test]
fn scratch_builds_a_worktree_of_the_repository_that_was_opened() {
    let fixture = conflicting_repo();

    let repo = Repo::open(fixture.path()).expect("open the fixture repository");
    let scratch = repo
        .scratch("main")
        .expect("create a scratch worktree of the fixture");

    assert_eq!(
        scratch
            .testing_git()
            .rev_parse("HEAD")
            .expect("read the scratch worktree's HEAD"),
        fixture.rev_parse("main"),
        "the scratch should be checked out at the opened repository's own 'main'"
    );
    assert!(
        scratch.path().is_dir(),
        "the scratch worktree should exist on disk at {}",
        scratch.path().display()
    );
}

/// The revision a scratch is built at is a revision, even when it starts with a
/// dash, and `git worktree add` has to read it as one.
///
/// `git worktree add -q --detach <path> --force` is a complete and valid
/// command. Git reads `--force` in the commit-ish slot as its own `--force`
/// flag, finds no commit-ish left, and builds the worktree at HEAD - exit 0,
/// no complaint. So a caller who asked for a scratch of one revision silently
/// got one of another, and every number measured in it describes work nobody
/// asked about. That is the cheap answer this crate exists never to give, and
/// it costs a whole simulation to produce.
///
/// `--force` rather than a name nobody would type, because it is the shape that
/// succeeds. A dash-leading name git does not know fails either way; this one
/// is the name that used to be obeyed.
#[test]
fn scratch_refuses_a_revision_that_starts_with_a_dash_rather_than_building_one_at_head() {
    let fixture = conflicting_repo();
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    let error = repo.scratch("--force").map(|_| ()).expect_err(
        "a revision that names no commit has to be refused, or the scratch is checked out \
         somewhere the caller never asked about and every measurement taken in it is about \
         another branch",
    );

    let message = format!("{error:#}");
    assert!(
        message.contains("--force"),
        "the refusal has to name the revision it could not use: {message}"
    );
}

/// A developer is hardly ever standing in the repository root, so the directory
/// a tool hands to [`Repo::open`] is usually a subdirectory of one.
///
/// `Repo::open` says in as many words that this works, and until now nothing
/// checked it: every fixture was opened at its own root, so the claim was carried
/// by a comment. It is the kind of claim that breaks quietly, too - the path the
/// pre-flight validated is private now, so no test can inspect it, and every
/// consequence of getting it wrong arrives as an answer that merely looks
/// smaller or fails somewhere else entirely.
///
/// So the assertions are the three things a subdirectory must not change, and
/// none of them is about the path itself:
///
/// - **Where a revision points**, which has nothing to do with the directory the
///   question was asked from.
/// - **What counts as uncommitted**, asserted over an edit made *outside* the
///   subdirectory: a status scoped to the cwd would report a clean tree and the
///   caveat about work a replay cannot see would go unsaid.
/// - **That a worktree still comes out**, since [`Repo::scratch`] is the only
///   route to one and it is the stored subdirectory that git is asked from.
///
/// Verified by mutation: making `Repo::open` refuse a non-empty
/// `rev-parse --show-prefix` fails this test, and scoping `uncommitted_files` to
/// the cwd with a `-- .` pathspec fails it too, while the rest of the suite stays
/// green in both cases.
#[test]
fn open_from_a_subdirectory_answers_for_the_whole_repository() {
    let fixture = nested_conflict_repo();
    let root = Repo::open(fixture.path()).expect("open the fixture repository at its root");

    let nested = fixture.path().join("sub").join("nested");
    let repo = Repo::open(&nested).expect("a subdirectory of a repository is inside one");

    assert_eq!(
        repo.resolve("left")
            .expect("resolve a branch from the subdirectory"),
        root.resolve("left")
            .expect("resolve the same branch from the root"),
        "a branch points where it points; which directory the question was asked \
         from is no part of the answer"
    );

    // At the repository root, so the edit sits outside the subdirectory the
    // question is being asked from.
    fixture.write_file("shared.txt", "locally edited, never committed\n");

    assert_eq!(
        repo.uncommitted_files()
            .expect("count uncommitted files from the subdirectory"),
        Uncommitted::new(1),
        "uncommitted work is uncommitted wherever it sits, so a count taken from \
         a subdirectory has to cover the whole repository"
    );

    let scratch = repo
        .scratch("main")
        .expect("create a scratch worktree from the subdirectory-opened repository");

    assert_eq!(
        scratch
            .testing_git()
            .rev_parse("HEAD")
            .expect("read the scratch worktree's HEAD"),
        fixture.rev_parse("main"),
        "the scratch should be checked out at the opened repository's own 'main', \
         not at anything the subdirectory implies"
    );
    assert!(
        scratch.path().is_dir(),
        "the scratch worktree should exist on disk at {}",
        scratch.path().display()
    );
}

/// The defect this whole type exists to kill: a mistyped branch name used to
/// come back as "you have conflicts" because a failed rebase and a bad argument
/// were indistinguishable. Resolving up front turns that into an error, and the
/// error is only actionable if it repeats the name that did not resolve.
#[test]
fn resolve_rejects_an_unresolvable_revision_and_names_it() {
    let fixture = conflicting_repo();
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    let error = repo
        .resolve("mian")
        .expect_err("'mian' is not a branch in the fixture");

    let message = format!("{error:#}");
    assert!(
        message.contains("mian"),
        "the error should name the revision that did not resolve: {message}"
    );
}

/// Resolving is what lets a caller compare candidates and detect a no-op before
/// building anything, so it has to agree with git about where a branch points.
#[test]
fn resolve_returns_the_commit_a_branch_points_at() {
    let fixture = conflicting_repo();
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    let resolved = repo.resolve("left").expect("resolve an existing branch");

    assert_eq!(
        resolved,
        fixture.rev_parse("left"),
        "resolve should agree with git about where 'left' points"
    );
}

/// A clean tree has to read as clean, or every caller that warns about
/// uncommitted work would cry wolf on every run.
#[test]
fn uncommitted_files_is_zero_on_a_clean_tree() {
    let fixture = conflicting_repo();
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    assert_eq!(
        repo.uncommitted_files().expect("count uncommitted files"),
        Uncommitted::new(0),
        "a freshly committed fixture should have nothing uncommitted"
    );
}

/// "Uncommitted" means everything a replay would not carry with it, so all
/// three flavours count: what is staged, what is only in the working tree, and
/// what git is not tracking at all.
#[test]
fn uncommitted_files_counts_staged_unstaged_and_untracked_work() {
    let fixture = TestRepo::init();
    fixture.commit_files(
        &[
            ("staged.txt", "committed\n"),
            ("unstaged.txt", "committed\n"),
        ],
        "base",
    );

    std::fs::write(fixture.path().join("staged.txt"), "staged edit\n").expect("edit a file");
    fixture.git(&["add", "staged.txt"]);
    std::fs::write(fixture.path().join("unstaged.txt"), "unstaged edit\n").expect("edit a file");
    std::fs::write(fixture.path().join("untracked.txt"), "brand new\n").expect("write a new file");

    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    assert_eq!(
        repo.uncommitted_files().expect("count uncommitted files"),
        Uncommitted::new(3),
        "staged, unstaged and untracked work should each count"
    );
}

/// A renamed file is one uncommitted file, and the format the count is read
/// from is the one that makes that hard to see.
///
/// `git status --porcelain` writes a rename as a single `R  old -> new`, so
/// counting records was counting lines. Its NUL-separated form cannot do that -
/// a path may itself contain ` -> ` - and spends *two* fields on the one record
/// instead, the new name and then the old. Counting fields would call a moved
/// file two uncommitted files, and inflate every warning about uncovered work
/// in precisely the situation a developer is most likely to be in: mid-refactor,
/// with a pile of renames staged.
///
/// The two plain files beside the rename are what make this fail from both
/// directions, and both directions are reachable. Pair nothing and the answer is
/// 4, one field per name. Pair unconditionally - swallow whatever follows every
/// record rather than only what follows a rename - and it is 2. Only a count
/// that pairs exactly the rename gives 3.
#[test]
fn uncommitted_files_counts_a_rename_as_the_one_file_it_is() {
    let fixture = TestRepo::init();
    fixture.commit_file("before.txt", "committed\n", "base");

    // `git mv` stages the rename, which is what lets git's rename detection
    // report it as one `R` record rather than a delete beside an addition.
    fixture.git(&["mv", "before.txt", "after.txt"]);
    for name in ["one-more.txt", "two-more.txt"] {
        std::fs::write(fixture.path().join(name), "brand new\n").expect("write a new file");
    }

    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    assert_eq!(
        repo.uncommitted_files().expect("count uncommitted files"),
        Uncommitted::new(3),
        "a rename is one uncommitted file, not one per name it has had"
    );
}

/// By default git collapses an untracked directory into a single line, so a
/// hundred new files would report as one. The count is meant to convey how much
/// work is sitting outside the commit graph, which makes that a lie worth
/// spending `--untracked-files=all` to avoid.
#[test]
fn uncommitted_files_counts_every_file_inside_an_untracked_directory() {
    let fixture = TestRepo::init();
    fixture.commit_file("tracked.txt", "committed\n", "base");

    let untracked = fixture.path().join("untracked-dir");
    std::fs::create_dir(&untracked).expect("create an untracked directory");
    std::fs::write(untracked.join("one.txt"), "one\n").expect("write a new file");
    std::fs::write(untracked.join("two.txt"), "two\n").expect("write a new file");

    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    assert_eq!(
        repo.uncommitted_files().expect("count uncommitted files"),
        Uncommitted::new(2),
        "an untracked directory should count its files, not itself"
    );
}
