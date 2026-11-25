//! Item 6 of the plan, first part: the refusals before the search.
//!
//! Each refusal exits 2, names the commit with its short id, and leaves the
//! checkout of the user as it was. No scratch worktree exists at that point.

mod support;

use gitscratch::testing::TestRepo;
use support::{add_test_commit, bibibi, linear_history, snapshot};

const VALUES: [&str; 3] = ["good", "good", "bad"];

/// The first seven characters of `id`.
fn short(id: &str) -> String {
    id.chars().take(7).collect()
}

/// Run `bibibi` in `repo` with `args` and check the refusal: exit 2, exactly
/// `message` on standard error, nothing on standard output, and the same
/// snapshot of the checkout before and after.
fn assert_refusal(repo: &TestRepo, args: &[&str], message: &str) {
    let before = snapshot(repo);

    let output = bibibi(repo.path(), args);
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(2), "stderr: {stderr}");
    assert_eq!(stderr, format!("bibibi: {message}\n"));
    assert_eq!(stdout, "");
    assert_eq!(snapshot(repo), before);
}

#[test]
fn a_merge_commit_as_the_test_commit_is_refused() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    repo.git(&["checkout", "-q", "-b", "side", &ids[0]]);
    repo.commit_file("side.txt", "side\n", "side");
    repo.git(&["checkout", "-q", "-"]);
    repo.git(&["merge", "-q", "--no-ff", "-m", "merge", "side"]);
    let merge = repo.rev_parse("HEAD");

    assert_refusal(
        &repo,
        &["--good", &ids[0], "--", "sh", "tests/t.sh"],
        &format!(
            "the test commit {} is a merge commit. Give a commit that has one parent.",
            short(&merge)
        ),
    );
}

#[test]
fn a_root_commit_as_the_test_commit_is_refused() {
    let repo = TestRepo::init();
    std::fs::create_dir_all(repo.path().join("tests")).expect("create tests directory");
    repo.commit_file("tests/t.sh", "true\n", "root");
    let root = repo.rev_parse("HEAD");

    assert_refusal(
        &repo,
        &[&root, "--good", &root, "--", "sh", "tests/t.sh"],
        &format!(
            "the test commit {} is a root commit. It has no parent to take the change from.",
            short(&root)
        ),
    );
}

#[test]
fn a_test_commit_that_changes_no_file_is_refused() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    repo.git(&["commit", "-q", "--allow-empty", "-m", "nothing"]);
    let empty = repo.rev_parse("HEAD");

    assert_refusal(
        &repo,
        &["--good", &ids[0], "--", "sh", "tests/t.sh"],
        &format!("the test commit {} changes no file.", short(&empty)),
    );
}

#[test]
fn a_good_commit_that_is_the_bad_commit_is_refused() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit(&repo);

    assert_refusal(
        &repo,
        &[
            "--good",
            &ids[2],
            "--bad",
            &ids[2],
            "--",
            "sh",
            "tests/t.sh",
        ],
        &format!(
            "the good commit {} is the same commit as the bad commit.",
            short(&ids[2])
        ),
    );
}

#[test]
fn a_good_commit_that_is_not_an_ancestor_of_the_bad_commit_is_refused() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    repo.git(&["checkout", "-q", "-b", "side", &ids[0]]);
    repo.commit_file("side.txt", "side\n", "side");
    let side = repo.rev_parse("HEAD");
    repo.git(&["checkout", "-q", "-"]);
    add_test_commit(&repo);

    assert_refusal(
        &repo,
        &["--good", &side, "--", "sh", "tests/t.sh"],
        &format!(
            "the good commit {} is not an ancestor of the bad commit {}.",
            short(&side),
            short(&ids[2])
        ),
    );
}
