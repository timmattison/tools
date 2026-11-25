//! Item 9 of the plan: a hermetic run.
//!
//! A git hook exports `GIT_DIR` and `GIT_INDEX_FILE` into its children. A test
//! command that inherits them reads another repository than the scratch
//! worktree. `bibibi` sheds them, and it keeps the variables that a person
//! states on purpose, such as `GIT_SSH_COMMAND`.

mod support;

use std::collections::BTreeMap;
use std::path::Path;

use gitscratch::testing::TestRepo;
use support::{add_test_commit_with, bibibi_command, linear_history};
use tempfile::TempDir;

const VALUES: [&str; 10] = [
    "good", "good", "good", "good", "good", "bad", "bad", "bad", "bad", "bad",
];

/// The value that the test sets for `GIT_SSH_COMMAND`, and expects to see.
const USER_SSH_COMMAND: &str = "bibibi-keeps-this";

/// The test script. It reads `value.txt` through git, so that an inherited
/// `GIT_DIR` aims it at another repository. It appends the two variables to
/// the log that the first argument names.
const SCRIPT: &str = "\
echo \"GIT_DIR=${GIT_DIR-}\" >> \"$1\"
echo \"GIT_SSH_COMMAND=${GIT_SSH_COMMAND-}\" >> \"$1\"
test \"$(git show HEAD:value.txt)\" = good
";

/// Every file under `dir`: the path relative to `dir`, and the bytes.
fn read_tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(&current).expect("read directory") {
            let path = entry.expect("read entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path
                    .strip_prefix(dir)
                    .expect("path under the directory")
                    .to_string_lossy()
                    .into_owned();
                files.insert(relative, std::fs::read(&path).expect("read file"));
            }
        }
    }
    files
}

#[test]
fn the_test_command_does_not_inherit_the_git_environment_of_a_hook() {
    let repo = TestRepo::init();
    let ids = linear_history(&repo, &VALUES);
    add_test_commit_with(&repo, SCRIPT);

    let decoy = TestRepo::init();
    decoy.commit_file("value.txt", "good\n", "decoy");
    let decoy_git = decoy.path().join(".git");
    let before = read_tree(&decoy_git);

    let logs = TempDir::new().expect("create temp dir");
    let log = logs.path().join("env.log");
    let log_arg = log.to_str().expect("path is UTF-8");

    let mut command = bibibi_command(
        repo.path(),
        &["--good", &ids[0], "--", "sh", "tests/t.sh", log_arg],
    );
    // After the sweep that the helper does.
    command.env("GIT_DIR", &decoy_git);
    command.env("GIT_INDEX_FILE", decoy_git.join("index"));
    command.env("GIT_SSH_COMMAND", USER_SSH_COMMAND);
    let output = command.output().expect("failed to run bibibi");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, format!("{} commit 6\n", ids[5]));

    let logged = std::fs::read_to_string(&log).expect("the test wrote the log");
    let dirs: Vec<&str> = logged
        .lines()
        .filter(|line| line.starts_with("GIT_DIR="))
        .collect();
    let ssh: Vec<&str> = logged
        .lines()
        .filter(|line| line.starts_with("GIT_SSH_COMMAND="))
        .collect();
    assert!(!dirs.is_empty(), "log: {logged}");
    assert!(dirs.iter().all(|line| *line == "GIT_DIR="), "log: {logged}");
    assert_eq!(dirs.len(), ssh.len(), "log: {logged}");
    let expected = format!("GIT_SSH_COMMAND={USER_SSH_COMMAND}");
    assert!(ssh.iter().all(|line| *line == expected), "log: {logged}");

    assert_eq!(read_tree(&decoy_git), before);
}
