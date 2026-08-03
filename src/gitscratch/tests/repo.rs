//! `Repo` is the pre-flight a consumer runs before deciding a replay is worth
//! starting, so what it must get right is the *cheap rejection*: a directory
//! that is not a repository and a revision that does not resolve have to fail
//! here, clearly and by name, rather than surfacing later as a simulation that
//! mysteriously failed.
//!
//! Every fixture lives in its own `TempDir`, so concurrent `cargo test` runs
//! never share a path.

use tempfile::TempDir;

use gitscratch::testing::{conflicting_repo, TestRepo};
use gitscratch::{Repo, Uncommitted};

/// The whole point of opening a repository up front is that "you pointed me at
/// somewhere that is not a repository" is a different, cheaper answer than "the
/// simulation failed" - so it has to be said in those words.
#[test]
fn open_rejects_a_directory_that_is_not_a_git_repository() {
    let outside = TempDir::new().expect("create a directory outside any repository");

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

/// The path a `Repo` was opened at is what a consumer hands to
/// `Scratch::create`, so it must come back exactly as given.
#[test]
fn open_succeeds_inside_a_repository_and_reports_where_it_was_opened() {
    let fixture = conflicting_repo();

    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    assert_eq!(
        repo.path(),
        fixture.path(),
        "a Repo should report the directory it was opened at"
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
