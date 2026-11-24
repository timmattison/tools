//! Item 5 of the plan: the patch.
//!
//! In these tests the test commit changes `tests/t.sh`, a file that exists in
//! every commit. Line 1 of that file changes in each commit (`# revision <n>`),
//! so the plain apply of the patch fails where the context differs. Line 2 is
//! unchanged, and line 3 is the line that the test commit replaces. One
//! unchanged line separates the two changes, so `--3way` can merge them.

mod support;

use gitscratch::testing::TestRepo;
use support::{bibibi, search_lines};

const VALUES: [&str; 10] = [
    "good", "good", "good", "good", "good", "bad", "bad", "bad", "bad", "bad",
];

/// The line of `tests/t.sh` that the test commit replaces.
const PLACEHOLDER: &str = "# placeholder";

/// The line that the test commit puts in its place.
const REAL_CHECK: &str = "grep -qx good value.txt";

/// Commit `n` of ten. Its `tests/t.sh` has the `# revision <n>` line, a line
/// that prints, and `placeholder`. Returns the ids in order.
fn history_with_script(repo: &TestRepo, placeholder_at: impl Fn(usize) -> String) -> Vec<String> {
    std::fs::create_dir_all(repo.path().join("tests")).expect("create tests directory");
    VALUES
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let number = index + 1;
            let script = format!(
                "# revision {number}\necho 't.sh ran'\n{}\n",
                placeholder_at(number)
            );
            repo.commit_files(
                &[
                    ("n.txt", &format!("{number}\n")),
                    ("value.txt", &format!("{value}\n")),
                    ("tests/t.sh", &script),
                ],
                &format!("commit {number}"),
            );
            repo.rev_parse("HEAD")
        })
        .collect()
}

/// The test commit: it keeps line 1 and line 2 of the script of commit 10 and
/// replaces the placeholder with the real check.
fn commit_real_check(repo: &TestRepo) {
    repo.commit_file(
        "tests/t.sh",
        &format!("# revision 10\necho 't.sh ran'\n{REAL_CHECK}\n"),
        "add the check",
    );
}

#[test]
fn three_way_applies_where_the_plain_apply_does_not() {
    let repo = TestRepo::init();
    let ids = history_with_script(&repo, |_| PLACEHOLDER.to_string());
    commit_real_check(&repo);

    let output = bibibi(repo.path(), &["--good", &ids[0], "--", "sh", "tests/t.sh"]);
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, format!("{} commit 6\n", ids[5]));
}

#[test]
fn a_patch_that_cannot_apply_makes_a_skip_with_the_reason() {
    let repo = TestRepo::init();
    // At commits 3, 4, 7 and 8 the placeholder differs, so the two changes
    // conflict. The search meets one of them (git breaks ties by id), and
    // never commit 5 or 6.
    let ids = history_with_script(&repo, |number| match number {
        3 | 4 | 7 | 8 => format!("{PLACEHOLDER} {number}"),
        _ => PLACEHOLDER.to_string(),
    });
    commit_real_check(&repo);

    let output = bibibi(repo.path(), &["--good", &ids[0], "--", "sh", "tests/t.sh"]);
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, format!("{} commit 6\n", ids[5]));
    assert!(
        search_lines(&stderr)
            .iter()
            .any(|line| line.contains(" skip ") && line.contains("- the patch does not apply -")),
        "stderr: {stderr}"
    );
}
