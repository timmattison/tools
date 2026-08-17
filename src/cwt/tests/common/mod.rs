//! Fixtures shared by the `cwt` end-to-end tests.
//!
//! Every end-to-end test needs the same three things: a throwaway repository,
//! a worktree beside it, and a way to run the binary somewhere inside them.
//! They live here so that a second test target does not copy them.
//!
//! Cargo compiles this module separately into each test binary, so a helper
//! that only one target calls looks unused to the other. That is what the
//! blanket `dead_code` allow is for; it is not hiding a helper nobody calls.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

/// Exit code that `cwt` returns when it finds no matching worktree.
pub const WORKTREE_NOT_FOUND: i32 = 3;

/// Exit code that `cwt` returns when more than one worktree matches.
pub const MULTIPLE_MATCHES: i32 = 6;

/// Runs a git command in `dir` and returns whether it succeeded.
///
/// Output is nulled so that concurrent test runs do not interleave noise. The
/// git environment of the parent is scrubbed because a run started from a git
/// hook inherits `GIT_DIR` and friends, which would point every command here at
/// the wrong repository.
pub fn run_git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_PREFIX")
        .status()
        .is_ok_and(|s| s.success())
}

/// Creates a throwaway repository whose first branch is `branch`, with one commit.
///
/// The repository is a subdirectory of the [`TempDir`] (keep it alive) so that
/// the worktrees added beside it are removed with it. gpg signing is disabled
/// so that a globally configured signer cannot break the commit.
pub fn init_repo(branch: &str) -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("failed to create temp dir");
    let repo = temp.path().join(REPO_DIR_NAME);
    std::fs::create_dir(&repo).expect("failed to create repo subdir");

    assert!(run_git(&repo, &["init", "-b", branch]), "git init failed");
    assert!(
        run_git(&repo, &["config", "user.email", "test@example.com"]),
        "git config user.email failed"
    );
    assert!(
        run_git(&repo, &["config", "user.name", "Test User"]),
        "git config user.name failed"
    );

    std::fs::write(repo.join("README.md"), "baseline\n").expect("failed to write baseline file");
    assert!(run_git(&repo, &["add", "README.md"]), "git add failed");
    assert!(
        run_git(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "baseline"]
        ),
        "git commit failed"
    );

    (temp, repo)
}

/// Directory name that [`init_repo`] gives the repository it creates.
///
/// `cwt` lists a worktree by its directory name, so a test that asserts the
/// rendered listing has to know what this one is called.
pub const REPO_DIR_NAME: &str = "repo";

/// Adds a worktree beside `repo` on a new branch, and returns its path.
///
/// The directory is named after the branch, so a listing renders the worktree
/// as `branch [branch]`.
pub fn add_worktree(temp: &TempDir, repo: &Path, branch: &str) -> PathBuf {
    let worktree = temp.path().join(branch);
    let path = worktree.to_str().expect("worktree path is not UTF-8");
    assert!(
        run_git(repo, &["worktree", "add", "-b", branch, path]),
        "git worktree add failed"
    );
    worktree
}

/// Runs the `cwt` binary in `dir` with `args`.
///
/// The inherited git environment is scrubbed for the same reason [`run_git`]
/// scrubs it: these tests can run from inside a pre-commit hook.
pub fn cwt(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cwt"))
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_PREFIX")
        .output()
        .expect("failed to run cwt")
}

/// Runs `cwt --main` in `dir`.
pub fn cwt_main(dir: &Path) -> Output {
    cwt(dir, &["--main"])
}
