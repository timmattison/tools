//! What `nwt` leaves behind when the `post-checkout` hook of a repository fails.
//!
//! `git worktree add` runs the `post-checkout` hook after it writes the files.
//! When the hook exits non-zero, git exits non-zero too, but it keeps the
//! worktree and the branch it made. `nwt` then sees a failed add and stops with
//! exit code 7. It prints no path, so the shell wrapper does not change
//! directory.
//!
//! This file is the contract for that failure. Issue #487 adds
//! `--sparse-exclude`, and that path runs the hook itself with `git hook run`
//! after a `--no-checkout` add. The sparse path must copy what this file
//! records: the same exit code, no path on stdout, and the worktree and the
//! branch kept. The sparse parity test goes in this file later.
//!
//! The test records the behavior that a real run measured with git 2.55.0. It
//! does not state a wish.
//!
//! Unix only: the fixture hook is a POSIX `sh` script that the Unix permission
//! bits make executable.
#![cfg(unix)]

mod support;

use std::path::{Path, PathBuf};

use support::{git_stdout, init_repo, nanos, nwt_command, run_git};

/// The exit code `nwt` returns when git does not make the worktree.
const WORKTREE_FAILED: i32 = 7;

/// The exit status of the fixture hook.
///
/// The value is not 1, so a run that reports the status of the hook is
/// distinguishable from a run that reports a generic failure.
const HOOK_EXIT_STATUS: i32 = 3;

/// The suffix `nwt` adds to the repository name to name the directory that
/// holds every new worktree.
const WORKTREES_SUFFIX: &str = "-worktrees";

/// A branch name that no concurrent copy of this suite can also hold.
fn unique_branch(label: &str) -> String {
    format!("{label}-{}-{}", std::process::id(), nanos())
}

/// Resolve a path before a comparison reads it.
///
/// Git prints resolved paths, and macOS reaches every temporary directory
/// through a symbolic link: `/var` resolves to `/private/var`.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
}

/// Write an executable `post-checkout` hook that exits with
/// [`HOOK_EXIT_STATUS`] into `hooks_dir`, and point `core.hooksPath` of `repo`
/// at that directory.
///
/// The fixture sets `core.hooksPath` in the repository, and does not write into
/// `.git/hooks`. The host `~/.gitconfig` can set a global `core.hooksPath`, and
/// git then ignores `.git/hooks`. A value in the repository configuration
/// overrides the global value.
fn install_failing_post_checkout_hook(repo: &Path, hooks_dir: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let hook = hooks_dir.join("post-checkout");
    std::fs::write(&hook, format!("#!/bin/sh\nexit {HOOK_EXIT_STATUS}\n"))
        .expect("write the post-checkout hook");
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
        .expect("make the post-checkout hook executable");

    let hooks_dir = canonical(hooks_dir);
    let hooks_dir = hooks_dir.to_str().expect("utf-8 hooks directory");
    assert!(
        run_git(repo, &["config", "core.hooksPath", hooks_dir]),
        "git config core.hooksPath failed"
    );
}

/// A plain add whose `post-checkout` hook fails keeps the worktree and the
/// branch, exits 7, and prints no path.
///
/// Today the error line on stderr says that the branch already exists. That
/// line gives the wrong reason: `nwt` asks git whether the branch is there
/// after a failed add, and the failed hook left it there. The sparse path of
/// issue #487 names the hook instead, so this test does not hold the words of
/// that line. It holds the exit code, the empty stdout, and what stays on disk
/// and in git.
#[test]
fn a_failed_post_checkout_hook_keeps_the_worktree_and_the_branch() {
    let (temp, repo) = init_repo();
    let hooks = tempfile::TempDir::new().expect("create the hooks directory");
    install_failing_post_checkout_hook(&repo, hooks.path());

    let branch = unique_branch("hook-fails");
    let output = nwt_command(&repo)
        .args(["-b", &branch, "--no-copy-env", "--no-bootstrap-hooks"])
        .output()
        .expect("run the nwt binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(WORKTREE_FAILED),
        "a failed post-checkout hook is a worktree failure:\n{stderr}"
    );
    assert!(
        stdout.is_empty(),
        "a failed run prints no path, so the shell wrapper stays put. stdout: {stdout:?}"
    );

    // `-b <branch>` names the directory after the branch, and the branch name
    // holds no slash, so the directory name is the branch name.
    let worktree = temp
        .path()
        .join(format!("repo{WORKTREES_SUFFIX}"))
        .join(&branch);
    assert!(
        worktree.is_dir(),
        "git keeps the worktree that the hook ran in: {}",
        worktree.display()
    );
    assert!(
        worktree.join("README.md").is_file(),
        "a plain add writes the files before the hook runs"
    );

    let branch_ref = format!("refs/heads/{branch}");
    assert!(
        run_git(&repo, &["show-ref", "--verify", "--quiet", &branch_ref]),
        "git keeps the branch that the add made"
    );

    let listing = git_stdout(&repo, &["worktree", "list", "--porcelain"]);
    let entry = format!("worktree {}", canonical(&worktree).display());
    assert!(
        listing.lines().any(|line| line == entry),
        "git still lists the worktree, so 'git worktree remove' can remove it:\n{listing}"
    );
}
