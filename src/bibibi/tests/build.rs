//! Item 3 of the plan: build skips.

mod support;

use gitscratch::testing::TestRepo;
use support::{add_test_commit, bibibi, linear_history, search_lines};

/// A build that prints a line, then fails where `n.txt` holds one of `broken`.
fn build_failing_at(broken: &[usize]) -> String {
    let tests: Vec<String> = broken
        .iter()
        .map(|number| format!("test \"$(cat n.txt)\" != {number}"))
        .collect();
    format!("echo building; {}", tests.join(" && "))
}

const BROKEN_AT: usize = 3;

const VALUES: [&str; 10] = [
    "good", "good", "good", "good", "good", "bad", "bad", "bad", "bad", "bad",
];

#[test]
fn a_build_that_fails_away_from_the_culprit_does_not_change_the_result() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit(&repo);
    let build = build_failing_at(&[BROKEN_AT]);

    let output = bibibi(
        repo.path(),
        &[
            "--good",
            &ids[0],
            "--build",
            &build,
            "--",
            "sh",
            "tests/t.sh",
        ],
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, format!("{} commit 6\n", ids[5]));
    assert!(
        search_lines(&stderr)
            .iter()
            .any(|line| line.contains(" skip ") && line.contains("- the build exited 1 -")),
        "stderr: {stderr}"
    );
}

#[test]
fn skips_at_the_culprit_and_its_parent_leave_three_candidates() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit(&repo);
    let build = build_failing_at(&[5, 6]);

    let output = bibibi(
        repo.path(),
        &[
            "--good",
            &ids[0],
            "--build",
            &build,
            "--",
            "sh",
            "tests/t.sh",
        ],
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    let mut lines: Vec<&str> = stdout.lines().collect();
    lines.sort_unstable();
    let mut expected = vec![
        format!("{} commit 5 - skipped: the build exited 1", ids[4]),
        format!("{} commit 6 - skipped: the build exited 1", ids[5]),
        format!("{} commit 7 - bad", ids[6]),
    ];
    expected.sort_unstable();
    assert_eq!(lines, expected, "stderr: {stderr}");
    assert!(
        stderr
            .lines()
            .any(|line| line == "bibibi: skips leave 3 candidates for the first bad commit"),
        "stderr: {stderr}"
    );
}
