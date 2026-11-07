//! Item 1 of the plan: a linear history, with the defaults.

mod support;

use std::process::Command;

use gitscratch::testing::TestRepo;
use support::{add_test_commit, bibibi, linear_history, search_lines, snapshot};

#[test]
fn finds_the_first_bad_commit_and_leaves_the_checkout_alone() {
    let repo = TestRepo::init();
    let values = [
        "good", "good", "good", "good", "good", "bad", "bad", "bad", "bad", "bad",
    ];
    let ids = linear_history(&repo, &values);
    add_test_commit(&repo);

    repo.git(&["bisect", "start", "--no-checkout", &ids[9], &ids[0]]);
    repo.write_file("n.txt", "dirty\n");
    repo.write_file("staged.txt", "staged\n");
    repo.git(&["add", "staged.txt"]);
    repo.write_file("untracked.txt", "untracked\n");
    let before = snapshot(&repo);

    let output = bibibi(repo.path(), &["--good", &ids[0], "--", "sh", "tests/t.sh"]);
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, format!("{} commit 6\n", ids[5]));
    assert!(!search_lines(&stderr).is_empty(), "stderr: {stderr}");
    assert_eq!(snapshot(&repo), before);
}

#[test]
fn version_names_the_tool_and_the_build() {
    let output = Command::new(env!("CARGO_BIN_EXE_bibibi"))
        .arg("--version")
        .output()
        .expect("failed to run bibibi");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout.lines().count(), 1);
    assert!(stdout.starts_with("bibibi 0.1.0 ("), "stdout: {stdout}");
    let line = stdout.trim_end();
    assert!(
        line.ends_with("clean)") || line.ends_with("dirty)"),
        "stdout: {stdout}"
    );
}
