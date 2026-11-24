//! Item 2 of the plan: explicit arguments.

mod support;

use gitscratch::testing::TestRepo;
use support::{add_test_commit, bibibi, linear_history, search_lines, snapshot};

/// The first 7 characters of a full object id.
fn short(id: &str) -> String {
    id.chars().take(7).collect()
}

#[test]
fn an_explicit_bad_commit_bounds_the_search() {
    let repo = TestRepo::init();
    let values = [
        "good", "good", "good", "good", "good", "good", "bad", "good", "good", "good",
    ];
    let ids = linear_history(&repo, &values);
    add_test_commit(&repo);

    let output = bibibi(
        repo.path(),
        &[
            "--good",
            &ids[0],
            "--bad",
            &ids[6],
            "--",
            "sh",
            "tests/t.sh",
        ],
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, format!("{} commit 7\n", ids[6]));
}

#[test]
fn a_test_commit_that_is_not_head_is_the_source_of_the_patch() {
    let repo = TestRepo::init();
    let values = [
        "good", "good", "good", "good", "good", "bad", "bad", "bad", "bad", "bad",
    ];
    let ids = linear_history(&repo, &values);
    repo.branch("with-test");
    let test_commit = add_test_commit(&repo);
    repo.checkout("main");
    let before = snapshot(&repo);

    let output = bibibi(
        repo.path(),
        &[&test_commit, "--good", &ids[0], "--", "sh", "tests/t.sh"],
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, format!("{} commit 6\n", ids[5]));
    assert_eq!(snapshot(&repo), before);
}

#[test]
fn two_good_commits_on_two_merged_branches_both_bound_the_search() {
    let repo = TestRepo::init();
    repo.commit_file("value.txt", "good\n", "base");
    let base = repo.rev_parse("HEAD");

    repo.branch("left");
    repo.commit_file("left.txt", "1\n", "left 1");
    let left_1 = repo.rev_parse("HEAD");
    repo.commit_file("left.txt", "2\n", "left 2");
    let left_2 = repo.rev_parse("HEAD");

    repo.checkout("main");
    repo.branch("right");
    // A long right branch: with only the first good commit, the search
    // probes it, and the second good commit must rule those probes out.
    let mut right = Vec::new();
    for number in 1..=8 {
        repo.commit_file(
            "right.txt",
            &format!("{number}\n"),
            &format!("right {number}"),
        );
        right.push(repo.rev_parse("HEAD"));
    }
    let right_2 = right[7].clone();

    repo.checkout("main");
    repo.git(&["merge", "--no-ff", "-q", "-m", "merge left", "left"]);
    repo.git(&["merge", "--no-ff", "-q", "-m", "merge right", "right"]);

    let mut after = Vec::new();
    for (number, value) in [(1, "good"), (2, "bad"), (3, "bad"), (4, "bad")] {
        repo.commit_files(
            &[
                ("value.txt", &format!("{value}\n")),
                ("after.txt", &format!("{number}\n")),
            ],
            &format!("after {number}"),
        );
        after.push(repo.rev_parse("HEAD"));
    }
    add_test_commit(&repo);

    let output = bibibi(
        repo.path(),
        &[
            "--good",
            &left_2,
            "--good",
            &right_2,
            "--",
            "sh",
            "tests/t.sh",
        ],
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, format!("{} after 2\n", after[1]));
    let searched = search_lines(&stderr);
    assert!(!searched.is_empty(), "stderr: {stderr}");
    for reachable in [&base, &left_1, &left_2].into_iter().chain(&right) {
        let id = short(reachable);
        assert!(
            searched.iter().all(|line| !line.contains(&id)),
            "a search line names the good commit {id}: {searched:?}"
        );
    }
}
