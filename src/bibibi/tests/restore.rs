//! Item 7 of the plan: the restore.

mod support;

use gitscratch::testing::TestRepo;
use support::{add_test_commit, bibibi};

#[test]
fn a_build_that_changes_a_tracked_file_does_not_stop_the_next_checkout() {
    let repo = TestRepo::init();
    let ids: Vec<String> = (1..=10)
        .map(|number| {
            let value = if number >= 6 { "bad" } else { "good" };
            repo.commit_files(
                &[
                    ("lock.txt", &format!("lock {number}\n")),
                    ("value.txt", &format!("{value}\n")),
                ],
                &format!("commit {number}"),
            );
            repo.rev_parse("HEAD")
        })
        .collect();
    add_test_commit(&repo);

    let output = bibibi(
        repo.path(),
        &[
            "--good",
            &ids[0],
            "--build",
            "echo rebuilt >> lock.txt",
            "--",
            "sh",
            "tests/t.sh",
        ],
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, format!("{} commit 6\n", ids[5]));
}
