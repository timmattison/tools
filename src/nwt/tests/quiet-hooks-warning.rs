//! Integration test pinning the deliberate quiet-bypass of the ungated-worktree
//! safety-net warning (issue #275).
//!
//! When a worktree's effective `core.hooksPath` points at a directory that does
//! not exist, git silently runs NO hooks and every commit is ungated. nwt emits
//! a loud warning in that case. The whole point of #275 is that this failure must
//! never be invisible — so even `--quiet` (which suppresses ordinary non-error
//! output) must NOT swallow this warning. This test runs the real `nwt` binary
//! with `-q` against a repo whose `core.hooksPath` is missing and asserts the
//! warning still reaches stderr.

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

#[test]
fn ungated_worktree_warning_survives_quiet() {
    // Everything lives inside this TempDir. `repo` is a SUBDIR so nwt's sibling
    // `<name>-worktrees` output directory also lands inside the TempDir.
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

    // Create + commit a baseline file. Commit exactly once (no retry loop) and
    // disable gpg signing so a globally-configured signer can't break the test.
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
    // directory we deliberately do NOT create. Worktrees share repo-local config,
    // so the new worktree's effective value is the same and the dir is missing
    // there too. No package.json exists, so hook bootstrap is a no-op.
    assert!(
        run_git(&repo, &["config", "core.hooksPath", ".husky/_"]),
        "git config core.hooksPath failed"
    );

    // Process-unique branch name for parallel safety: a background bacon loop
    // runs these tests concurrently with the pre-commit hook's own run.
    let branch = format!("quiet-warn-{}-{}", std::process::id(), nanos());

    let output = Command::new(env!("CARGO_BIN_EXE_nwt"))
        .args(["-b", &branch, "-q"])
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

    // Sanity: the worktree path is printed to stdout (the shell wrapper captures
    // this; the warning must not corrupt it, which is why the warning goes to
    // stderr).
    assert!(
        stdout.contains(&branch) || stdout.contains("-worktrees"),
        "stdout should contain the worktree path.\nstdout: {stdout}\nstderr: {stderr}"
    );

    // The crux of #275: with -q, the ungated-worktree warning MUST still appear
    // on stderr. The two halves of the warning are checked independently.
    assert!(
        stderr.contains("core.hooksPath"),
        "ungated-worktree warning must survive --quiet (missing 'core.hooksPath').\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("commits are ungated"),
        "ungated-worktree warning must survive --quiet (missing 'commits are ungated').\nstderr: {stderr}"
    );
}
