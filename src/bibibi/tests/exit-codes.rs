//! Item 4 of the plan: test exit codes.

mod support;

use gitscratch::testing::TestRepo;
use support::{add_test_commit_with, bibibi, linear_history, search_lines};

const VALUES: [&str; 10] = [
    "good", "good", "good", "good", "good", "bad", "bad", "bad", "bad", "bad",
];

/// A test script that exits 125 at commits 3, 4, 7 and 8. The search tests
/// commit 5 or 6 first, then one of those four (git breaks ties by id), so
/// every run skips at least one commit and never commit 5 or 6.
const SKIPPING_SCRIPT: &str = "echo 't.sh ran'
case \"$(cat n.txt)\" in 3|4|7|8) exit 125;; esac
grep -qx good value.txt
";

/// A test script that stops itself with SIGKILL where `value.txt` is not good.
const KILLED_SCRIPT: &str = "echo 't.sh ran'
grep -qx good value.txt && exit 0
kill -KILL $$
";

#[test]
fn a_test_that_exits_125_is_a_skip() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit_with(&repo, SKIPPING_SCRIPT);

    let output = bibibi(repo.path(), &["--good", &ids[0], "--", "sh", "tests/t.sh"]);
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, format!("{} commit 6\n", ids[5]));
    assert!(
        search_lines(&stderr)
            .iter()
            .any(|line| line.contains(" skip ") && line.contains("- the test exited 125 -")),
        "stderr: {stderr}"
    );
}

#[test]
fn a_test_that_a_signal_stops_is_bad_and_the_search_continues() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit_with(&repo, KILLED_SCRIPT);

    let output = bibibi(repo.path(), &["--good", &ids[0], "--", "sh", "tests/t.sh"]);
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, format!("{} commit 6\n", ids[5]));
    assert!(
        search_lines(&stderr)
            .iter()
            .any(|line| line.contains(" bad ")),
        "stderr: {stderr}"
    );
}
