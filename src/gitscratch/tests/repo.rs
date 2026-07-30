//! `Repo` is the pre-flight a consumer runs before deciding a replay is worth
//! starting, so what it must get right is the *cheap rejection*: a directory
//! that is not a repository and a revision that does not resolve have to fail
//! here, clearly and by name, rather than surfacing later as a simulation that
//! mysteriously failed.
//!
//! Every fixture lives in its own `TempDir`, so concurrent `cargo test` runs
//! never share a path.

use tempfile::TempDir;

use gitscratch::testing::conflicting_repo;
use gitscratch::Repo;

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
