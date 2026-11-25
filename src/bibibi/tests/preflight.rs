//! Item 6 of the plan, second part: the pre-flight.
//!
//! Before the search, `bibibi` tests each good commit and then the bad
//! commit. A run that fails the pre-flight exits 2 with the reason, prints a
//! pre-flight line for the commit that failed, and leaves the checkout of the
//! user as it was.

mod support;

use gitscratch::testing::TestRepo;
use support::{
    add_test_commit, add_test_commit_with, bibibi, linear_history, preflight_lines, snapshot,
    DEFAULT_SCRIPT,
};

const VALUES: [&str; 10] = [
    "good", "good", "good", "good", "good", "bad", "bad", "bad", "bad", "bad",
];

/// The first seven characters of `id`.
fn short(id: &str) -> String {
    id.chars().take(7).collect()
}

/// Run `bibibi` in `repo` with `args` and check the refusal: exit 2, the
/// message as the last line of standard error, nothing on standard output, and
/// the same snapshot of the checkout before and after. Returns standard error.
fn assert_refusal(repo: &TestRepo, args: &[&str], message: &str) -> String {
    let before = snapshot(repo);

    let output = bibibi(repo.path(), args);
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(2), "stderr: {stderr}");
    assert_eq!(
        stderr.lines().last(),
        Some(format!("bibibi: {message}").as_str()),
        "stderr: {stderr}"
    );
    assert_eq!(stdout, "");
    assert_eq!(snapshot(repo), before);
    stderr
}

/// Check that `stderr` holds a pre-flight line for `id` that ends as `ending`.
fn assert_preflight_line(stderr: &str, id: &str, ending: &str) {
    let prefix = format!("bibibi: {} ", short(id));
    assert!(
        preflight_lines(stderr)
            .iter()
            .any(|line| line.starts_with(&prefix) && line.ends_with(ending)),
        "no pre-flight line for {id} that ends in '{ending}'. stderr: {stderr}"
    );
}

/// A history in which `f.txt` reads `f A` at commits 1 to 5 and 8 to 10, and
/// `f B` at commits 6 and 7. The test commit changes `f.txt` from `f A`, so
/// its patch applies at commits 1 to 5 and 8 to 10 and nowhere else.
fn history_with_f(repo: &TestRepo) -> Vec<String> {
    VALUES
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let number = index + 1;
            let f = if number == 6 || number == 7 {
                "f B"
            } else {
                "f A"
            };
            repo.commit_files(
                &[
                    ("n.txt", &format!("{number}\n")),
                    ("value.txt", &format!("{value}\n")),
                    ("f.txt", &format!("{f}\n")),
                ],
                &format!("commit {number}"),
            );
            repo.rev_parse("HEAD")
        })
        .collect()
}

/// Add the test commit of [`history_with_f`]: it changes `f.txt` and adds the
/// default script.
fn add_test_commit_changing_f(repo: &TestRepo) {
    std::fs::create_dir_all(repo.path().join("tests")).expect("create tests directory");
    repo.commit_files(
        &[("f.txt", "f new\n"), ("tests/t.sh", DEFAULT_SCRIPT)],
        "add the test",
    );
}

#[test]
fn a_test_that_fails_at_a_good_commit_is_refused_with_its_output() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit(&repo);

    let stderr = assert_refusal(
        &repo,
        &["--good", &ids[7], "--", "sh", "tests/t.sh"],
        &format!(
            "the test does not pass at the good commit {}: the test exited 1.",
            short(&ids[7])
        ),
    );

    assert!(stderr.contains("t.sh ran"), "stderr: {stderr}");
    assert_preflight_line(&stderr, &ids[7], "- pre-flight");
}

#[test]
fn a_build_that_fails_at_a_good_commit_is_refused() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit(&repo);

    let stderr = assert_refusal(
        &repo,
        &[
            "--good",
            &ids[0],
            "--build",
            "test \"$(cat n.txt)\" != 1",
            "--",
            "sh",
            "tests/t.sh",
        ],
        &format!(
            "the test does not pass at the good commit {}: the build exited 1.",
            short(&ids[0])
        ),
    );

    assert_preflight_line(&stderr, &ids[0], "- the build exited 1 - pre-flight");
}

#[test]
fn a_patch_that_does_not_apply_at_a_good_commit_is_refused() {
    let repo = TestRepo::init();
    let ids = history_with_f(&repo);
    add_test_commit_changing_f(&repo);

    let stderr = assert_refusal(
        &repo,
        &["--good", &ids[5], "--", "sh", "tests/t.sh"],
        &format!(
            "the test does not pass at the good commit {}: the patch does not apply.",
            short(&ids[5])
        ),
    );

    assert_preflight_line(&stderr, &ids[5], "- the patch does not apply - pre-flight");
}

#[test]
fn a_test_that_passes_at_the_bad_commit_is_refused() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit(&repo);

    let stderr = assert_refusal(
        &repo,
        &[
            "--good",
            &ids[0],
            "--bad",
            &ids[2],
            "--",
            "sh",
            "tests/t.sh",
        ],
        &format!(
            "the test does not fail at the bad commit {}: the test exited 0.",
            short(&ids[2])
        ),
    );

    assert_preflight_line(&stderr, &ids[0], "- pre-flight");
    assert_preflight_line(&stderr, &ids[2], "- pre-flight");
}

#[test]
fn a_test_that_exits_125_at_the_bad_commit_is_refused() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit_with(
        &repo,
        "echo 't.sh ran'\ntest \"$(cat n.txt)\" = 10 && exit 125\ngrep -qx good value.txt\n",
    );

    let stderr = assert_refusal(
        &repo,
        &["--good", &ids[0], "--", "sh", "tests/t.sh"],
        &format!(
            "the test does not fail at the bad commit {}: the test exited 125.",
            short(&ids[9])
        ),
    );

    assert_preflight_line(&stderr, &ids[9], "- the test exited 125 - pre-flight");
}

#[test]
fn a_build_that_fails_at_the_bad_commit_is_refused() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit(&repo);

    let stderr = assert_refusal(
        &repo,
        &[
            "--good",
            &ids[0],
            "--build",
            "test \"$(cat n.txt)\" != 10",
            "--",
            "sh",
            "tests/t.sh",
        ],
        &format!(
            "the test does not fail at the bad commit {}: the build exited 1.",
            short(&ids[9])
        ),
    );

    assert_preflight_line(&stderr, &ids[9], "- the build exited 1 - pre-flight");
}

#[test]
fn a_patch_that_does_not_apply_at_the_bad_commit_is_refused() {
    let repo = TestRepo::init();
    let ids = history_with_f(&repo);
    add_test_commit_changing_f(&repo);

    let stderr = assert_refusal(
        &repo,
        &[
            "--good",
            &ids[0],
            "--bad",
            &ids[6],
            "--",
            "sh",
            "tests/t.sh",
        ],
        &format!(
            "the test does not fail at the bad commit {}: the patch does not apply.",
            short(&ids[6])
        ),
    );

    assert_preflight_line(&stderr, &ids[6], "- the patch does not apply - pre-flight");
}

#[test]
fn every_good_commit_is_tested_in_the_pre_flight_and_the_search_still_runs() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit(&repo);

    let output = bibibi(
        repo.path(),
        &[
            "--good",
            &ids[0],
            "--good",
            &ids[2],
            "--",
            "sh",
            "tests/t.sh",
        ],
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, format!("{} commit 6\n", ids[5]));
    let lines = preflight_lines(&stderr);
    assert_eq!(lines.len(), 3, "stderr: {stderr}");
    assert_eq!(
        lines,
        vec![
            format!("bibibi: {} good commit 1 - pre-flight", short(&ids[0])),
            format!("bibibi: {} good commit 3 - pre-flight", short(&ids[2])),
            format!("bibibi: {} bad  commit 10 - pre-flight", short(&ids[9])),
        ]
    );
}
