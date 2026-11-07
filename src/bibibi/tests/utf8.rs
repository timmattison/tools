//! Item 10 of the plan: UTF-8 subjects and a narrow terminal.
//!
//! The progress lines and the pre-flight lines never get wider than the
//! terminal. Only the subject gets shorter, and it is cut between characters,
//! measured in display columns. The result on standard output is never cut.

mod support;

use gitscratch::testing::TestRepo;
use support::{bibibi_command, preflight_lines, search_lines, DEFAULT_SCRIPT};
use unicode_width::UnicodeWidthStr;

/// The width that this test states.
const NARROW: usize = 60;

const VALUES: [&str; 10] = [
    "good", "good", "good", "good", "good", "bad", "bad", "bad", "bad", "bad",
];

const SUBJECTS: [&str; 10] = [
    "commit 1",
    "commit 2 plain text that is quite a bit longer than the width",
    "commit 3 日本語のテストの長い件名がここにあります",
    "commit 4",
    "commit 5 🎊🎁🎈🎂🎄🎃🎆🎇🎉🎊🎁🎈🎂🎄🎃🎆🎇",
    "commit 6 🎉 die Regression 🎉 mit einem sehr langen Rest",
    "commit 7",
    "commit 8 café au lait, crème brûlée, et encore plus de texte",
    "commit 9",
    "commit 10 日本語のテストの長い件名がここにあります",
];

#[test]
fn lines_fit_a_narrow_terminal_and_the_result_is_never_cut() {
    let repo = TestRepo::init();
    let ids: Vec<String> = SUBJECTS
        .iter()
        .zip(VALUES)
        .enumerate()
        .map(|(index, (subject, value))| {
            repo.commit_files(
                &[
                    ("n.txt", &format!("{}\n", index + 1)),
                    ("value.txt", &format!("{value}\n")),
                ],
                subject,
            );
            repo.rev_parse("HEAD")
        })
        .collect();
    {
        std::fs::create_dir_all(repo.path().join("tests")).expect("create tests directory");
        repo.commit_file("tests/t.sh", DEFAULT_SCRIPT, "add the test");
    }

    let mut command = bibibi_command(repo.path(), &["--good", &ids[0], "--", "sh", "tests/t.sh"]);
    // After the helper, which states a wide terminal.
    command.env("COLUMNS", NARROW.to_string());
    let output = command.output().expect("failed to run bibibi");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, format!("{} {}\n", ids[5], SUBJECTS[5]));

    let search = search_lines(&stderr);
    let preflight = preflight_lines(&stderr);
    assert!(!search.is_empty(), "stderr: {stderr}");
    assert!(!preflight.is_empty(), "stderr: {stderr}");
    for line in search.iter().chain(preflight.iter()) {
        assert!(
            line.width() <= NARROW,
            "'{line}' is {} columns wide",
            line.width()
        );
    }
    assert!(
        search
            .iter()
            .chain(preflight.iter())
            .any(|line| line.contains('…')),
        "no line was cut. stderr: {stderr}"
    );
}
