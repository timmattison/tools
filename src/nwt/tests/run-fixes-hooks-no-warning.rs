//! Integration test pinning the deferred placement of the ungated-worktree
//! safety-net warning when a synchronous `--run` command is given (issue #275).
//!
//! The safety net warns when a worktree's effective `core.hooksPath` points at a
//! directory that does not exist, because git then silently runs NO hooks and
//! every commit is ungated. But a `--run` command can be the very thing that
//! creates that directory (e.g. `pnpm install` regenerating `.husky/_`). If the
//! check fires BEFORE the run, it's a false alarm: the run is about to fix it.
//!
//! So for a synchronous `--run` (no --tmux), the check must run AFTER the command
//! completes. These tests pin both halves of that contract against the real
//! binary:
//!   * a run that DOES create the missing dir must produce NO warning, and
//!   * a run that does NOT create the dir must still warn (so the false-positive
//!     fix can't be "delete the check").

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::TempDir;

/// Runs a git command in `dir` with stdout/stderr nulled, returning success.
/// Stdout/stderr are nulled so concurrent test runs don't interleave noise.
fn run_git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Nanosecond timestamp for building process-unique, parallel-safe names.
fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos()
}

/// Creates a git repo (inside a TempDir subdir so the sibling `<name>-worktrees`
/// output directory also lands inside the TempDir), commits a baseline file, and
/// configures `core.hooksPath` repo-locally to a directory that does NOT exist.
/// No package.json exists, so hook bootstrap is a no-op. Returns the TempDir
/// (keep it alive) and the repo path.
fn repo_with_missing_hooks_dir() -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new().expect("Failed to create temp dir");
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).expect("Failed to create repo subdir");

    assert!(run_git(&repo, &["init"]), "git init failed");
    assert!(
        run_git(&repo, &["config", "user.email", "test@example.com"]),
        "git config user.email failed"
    );
    assert!(
        run_git(&repo, &["config", "user.name", "Test User"]),
        "git config user.name failed"
    );

    // Baseline commit. Commit exactly once (no retry loop); disable gpg signing
    // so a globally-configured signer can't break the test.
    std::fs::write(repo.join("README.md"), "baseline\n").expect("Failed to write baseline file");
    assert!(run_git(&repo, &["add", "README.md"]), "git add failed");
    assert!(
        run_git(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "baseline"]
        ),
        "git commit failed"
    );

    // The silent-no-hooks trap: configure core.hooksPath repo-locally to a
    // directory we deliberately do NOT create. Worktrees share repo-local config.
    assert!(
        run_git(&repo, &["config", "core.hooksPath", ".husky/_"]),
        "git config core.hooksPath failed"
    );

    (temp, repo)
}

#[test]
fn run_command_that_creates_hooks_dir_suppresses_warning() {
    let (_temp, repo) = repo_with_missing_hooks_dir();

    // Process-unique branch name for parallel safety: a background bacon loop
    // runs these tests concurrently with the pre-commit hook's own run.
    let branch = format!("run-fixes-{}-{}", std::process::id(), nanos());

    // The --run command creates the missing hooks dir. Because the safety-net
    // check is deferred until AFTER the run completes, it must see the dir now
    // exists and stay silent.
    let output = Command::new(env!("CARGO_BIN_EXE_nwt"))
        .args(["-b", &branch, "--run", "mkdir -p .husky/_"])
        .current_dir(&repo)
        .stdin(Stdio::null())
        .output()
        .expect("Failed to run nwt binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "nwt should succeed.\nstdout: {stdout}\nstderr: {stderr}"
    );

    // The crux: the --run command fixed the hooks dir before the deferred check,
    // so NO ungated-worktree warning should appear. Both halves are checked.
    assert!(
        !stderr.contains("core.hooksPath"),
        "run command created the hooks dir, so no warning should mention 'core.hooksPath'.\nstderr: {stderr}"
    );
    assert!(
        !stderr.contains("commits are ungated"),
        "run command created the hooks dir, so no 'commits are ungated' warning should appear.\nstderr: {stderr}"
    );
}

#[test]
fn run_command_that_leaves_hooks_dir_missing_still_warns() {
    let (_temp, repo) = repo_with_missing_hooks_dir();

    let branch = format!("run-nofix-{}-{}", std::process::id(), nanos());

    // The --run command does nothing to the hooks dir. The deferred check must
    // still fire — this guards against "fixing" the false positive by deleting
    // the check entirely.
    let output = Command::new(env!("CARGO_BIN_EXE_nwt"))
        .args(["-b", &branch, "--run", "true"])
        .current_dir(&repo)
        .stdin(Stdio::null())
        .output()
        .expect("Failed to run nwt binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "nwt should succeed.\nstdout: {stdout}\nstderr: {stderr}"
    );

    assert!(
        stderr.contains("core.hooksPath"),
        "run command left the hooks dir missing, so the warning must still appear (missing 'core.hooksPath').\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("commits are ungated"),
        "run command left the hooks dir missing, so the warning must still appear (missing 'commits are ungated').\nstderr: {stderr}"
    );
}
