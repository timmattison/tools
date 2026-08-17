//! End-to-end coverage for `cwt --main`, the flag behind the `wtm` alias.
//!
//! The unit tests in `main.rs` prove how `find_main_worktree` ranks branch
//! names. These tests prove that the binary applies that ranking to a real
//! repository: a repository that never renamed `master` still gets a main
//! worktree, `main` wins wherever both branches exist, and a branch that merely
//! contains the text "main" never captures the shortcut.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

/// Exit code that `cwt` returns when it finds no matching worktree.
const WORKTREE_NOT_FOUND: i32 = 3;

/// Runs a git command in `dir` and returns whether it succeeded.
///
/// Output is nulled so that concurrent test runs do not interleave noise. The
/// git environment of the parent is scrubbed because a run started from a git
/// hook inherits `GIT_DIR` and friends, which would point every command here at
/// the wrong repository.
fn run_git(dir: &Path, args: &[&str]) -> bool {
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
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Creates a throwaway repository whose first branch is `branch`, with one commit.
///
/// The repository is a subdirectory of the [`TempDir`] (keep it alive) so that
/// the worktrees added beside it are removed with it. gpg signing is disabled
/// so that a globally configured signer cannot break the commit.
fn init_repo(branch: &str) -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("failed to create temp dir");
    let repo = temp.path().join("repo");
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

/// Adds a worktree beside `repo` on a new branch, and returns its path.
fn add_worktree(temp: &TempDir, repo: &Path, branch: &str) -> PathBuf {
    let worktree = temp.path().join(branch);
    let path = worktree.to_str().expect("worktree path is not UTF-8");
    assert!(
        run_git(repo, &["worktree", "add", "-b", branch, path]),
        "git worktree add failed"
    );
    worktree
}

/// Runs `cwt --main` in `dir`.
fn cwt_main(dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cwt"))
        .arg("--main")
        .current_dir(dir)
        .stdin(Stdio::null())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_PREFIX")
        .output()
        .expect("failed to run cwt")
}

/// Asserts that `cwt --main`, run in `from`, printed the path of `expected`.
///
/// Both paths are canonicalized because macOS reports the temporary directory
/// through the `/var` symlink.
fn assert_main_worktree_is(from: &Path, expected: &Path) {
    let output = cwt_main(from);
    assert!(
        output.status.success(),
        "cwt --main failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let printed = String::from_utf8(output.stdout).expect("cwt printed invalid UTF-8");
    let printed = printed.trim();
    let actual = std::fs::canonicalize(printed)
        .unwrap_or_else(|e| panic!("cwt printed {printed:?}, which is not a path: {e}"));
    assert_eq!(
        actual,
        std::fs::canonicalize(expected).expect("the expected worktree does not exist"),
    );
}

#[test]
fn master_only_repository_resolves_to_master() {
    let (temp, repo) = init_repo("master");
    let feature = add_worktree(&temp, &repo, "feature");

    assert_main_worktree_is(&feature, &repo);
}

#[test]
fn main_wins_when_both_branches_have_worktrees() {
    let (temp, repo) = init_repo("main");
    let master = add_worktree(&temp, &repo, "master");

    assert_main_worktree_is(&master, &repo);
}

#[test]
fn branch_containing_main_does_not_capture_the_main_worktree() {
    let (temp, repo) = init_repo("master");
    let decoy = add_worktree(&temp, &repo, "wt-main-master");

    assert_main_worktree_is(&decoy, &repo);
}

#[test]
fn repository_without_main_or_master_reports_not_found() {
    let (temp, repo) = init_repo("trunk");
    let feature = add_worktree(&temp, &repo, "feature");

    let output = cwt_main(&feature);
    assert_eq!(output.status.code(), Some(WORKTREE_NOT_FOUND));
    assert!(
        output.stdout.is_empty(),
        "cwt must print no path when it finds no main worktree"
    );
}
