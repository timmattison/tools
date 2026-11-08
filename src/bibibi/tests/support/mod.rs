//! Fixtures and command builders that the test files of `bibibi` share.
//!
//! Each test binary compiles this module on its own, so a helper that one file
//! does not use is dead code in that binary.

#![allow(
    dead_code,
    reason = "each test binary compiles this module alone and uses a part of it"
)]

use std::path::Path;
use std::process::{Command, Output};

use gitscratch::testing::TestRepo;

/// The width that each run of `bibibi` states.
pub const STATED_WIDTH: &str = "200";

/// The default test script. It prints a line, then decides from `value.txt`.
pub const DEFAULT_SCRIPT: &str = "echo 't.sh ran'\ngrep -qx good value.txt\n";

/// Build a linear history. Commit `n` (from 1) writes `n.txt` with its number
/// and `value.txt` with `values[n - 1]`. The subject is `commit <n>`.
/// Returns the 40-character id of each commit, in order.
pub fn linear_history(repo: &TestRepo, values: &[&str]) -> Vec<String> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let number = index + 1;
            repo.commit_files(
                &[
                    ("n.txt", &format!("{number}\n")),
                    ("value.txt", &format!("{value}\n")),
                ],
                &format!("commit {number}"),
            );
            repo.rev_parse("HEAD")
        })
        .collect()
}

/// Add the test commit with the default script. Returns its id.
pub fn add_test_commit(repo: &TestRepo) -> String {
    add_test_commit_with(repo, DEFAULT_SCRIPT)
}

/// Add a commit that creates `tests/t.sh` with `script`. Returns its id.
pub fn add_test_commit_with(repo: &TestRepo, script: &str) -> String {
    std::fs::create_dir_all(repo.path().join("tests")).expect("create tests directory");
    repo.commit_file("tests/t.sh", script, "add the test");
    repo.rev_parse("HEAD")
}

/// A `bibibi` run in `repo`, built and not yet started.
pub fn bibibi_command(repo: &Path, args: &[&str]) -> Command {
    let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_bibibi"));
    command.args(args).current_dir(repo);
    // After the sweep, because the sweep removes every `GIT_` name.
    command.env("COLUMNS", STATED_WIDTH);
    command.env("LC_ALL", "C");
    command.env("LANG", "C");
    command
}

/// Run `bibibi` in `repo` and return everything it left behind.
pub fn bibibi(repo: &Path, args: &[&str]) -> Output {
    bibibi_command(repo, args)
        .output()
        .expect("failed to run bibibi")
}

/// The state of the checkout of the user that a run must not change.
#[derive(Debug, PartialEq)]
pub struct Snapshot {
    pub head: String,
    pub index: String,
    pub status: String,
    pub worktrees: String,
    pub bisect_refs: String,
    pub bisect_log: Option<Vec<u8>>,
}

/// Read the [`Snapshot`] of `repo`.
pub fn snapshot(repo: &TestRepo) -> Snapshot {
    let log_path = repo.git(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "BISECT_LOG",
    ]);
    Snapshot {
        head: repo.git(&["rev-parse", "HEAD"]),
        index: repo.git(&["ls-files", "--stage"]),
        status: repo.git(&["status", "--porcelain=v1", "--untracked-files=all"]),
        worktrees: repo.git(&["worktree", "list", "--porcelain"]),
        bisect_refs: repo.git(&["for-each-ref", "refs/bisect"]),
        bisect_log: std::fs::read(log_path.trim()).ok(),
    }
}

/// The search lines of `stderr`: the lines that end in `step left` or
/// `steps left`.
pub fn search_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| line.ends_with("step left") || line.ends_with("steps left"))
        .collect()
}

/// The pre-flight lines of `stderr`: the lines that end in `- pre-flight`.
pub fn preflight_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| line.ends_with("- pre-flight"))
        .collect()
}
