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

mod support;

use support::{init_repo, nanos, nwt_command, run_git};

#[test]
fn ungated_worktree_warning_survives_quiet() {
    // A fresh repo with a baseline commit (everything inside a TempDir so nwt's
    // sibling `<name>-worktrees` output dir is cleaned up with it).
    let (_temp, repo) = init_repo();

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

    // nwt_command scrubs ZELLIJ/TMUX so this never touches a real multiplexer.
    let output = nwt_command(&repo)
        .args(["-b", &branch, "-q"])
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
