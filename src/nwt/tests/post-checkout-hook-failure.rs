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
//! after a `--no-checkout` add. The sparse path copies what the plain add
//! does: the same exit code, no path on stdout, and the worktree and the branch
//! kept. One helper holds those assertions for both tests, so the two paths
//! cannot drift apart.
//!
//! The plain test records the behavior that a real run measured with git
//! 2.55.0. It does not state a wish.
//!
//! Unix only: the fixture hook is a POSIX `sh` script that the Unix permission
//! bits make executable.
#![cfg(unix)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

use support::{git_stdout, init_repo, install_post_checkout_hook, nanos, nwt_command, run_git};

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

/// The tracked directory that the sparse test excludes.
const HEAVY_DIR: &str = "heavy";

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

/// Install a `post-checkout` hook that exits with [`HOOK_EXIT_STATUS`] into
/// `hooks_dir`, and point `core.hooksPath` of `repo` at that directory.
fn install_failing_post_checkout_hook(repo: &Path, hooks_dir: &Path) {
    install_post_checkout_hook(repo, hooks_dir, &format!("exit {HOOK_EXIT_STATUS}\n"));
}

/// Run `nwt -b <branch>` in `repo` with `extra` arguments, without the `.env`
/// copy and the hook bootstrap, and hand back what it wrote.
fn run_nwt(repo: &Path, branch: &str, extra: &[&str]) -> Output {
    nwt_command(repo)
        .args(["-b", branch, "--no-copy-env", "--no-bootstrap-hooks"])
        .args(extra)
        .output()
        .expect("run the nwt binary")
}

/// Demand that `output` is a run whose `post-checkout` hook failed, and that
/// the run kept what git made. Hand back the worktree path.
///
/// The contract is the same for a plain add and for a sparse run: exit 7, no
/// path on stdout, the worktree directory kept, the branch kept, and the
/// worktree still listed by git, so `git worktree remove` can remove it.
///
/// `-b <branch>` names the directory after the branch, and the branch name
/// holds no slash, so the directory name is the branch name.
fn assert_the_failed_hook_kept_the_worktree(
    temp: &Path,
    repo: &Path,
    branch: &str,
    output: &Output,
) -> PathBuf {
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

    let worktree = temp.join(format!("repo{WORKTREES_SUFFIX}")).join(branch);
    assert!(
        worktree.is_dir(),
        "git keeps the worktree that the hook ran in: {}",
        worktree.display()
    );

    let branch_ref = format!("refs/heads/{branch}");
    assert!(
        run_git(repo, &["show-ref", "--verify", "--quiet", &branch_ref]),
        "git keeps the branch that the add made"
    );

    let listing = git_stdout(repo, &["worktree", "list", "--porcelain"]);
    let entry = format!("worktree {}", canonical(&worktree).display());
    assert!(
        listing.lines().any(|line| line == entry),
        "git still lists the worktree, so 'git worktree remove' can remove it:\n{listing}"
    );

    worktree
}

/// A plain add whose `post-checkout` hook fails keeps the worktree and the
/// branch, exits 7, and prints no path.
///
/// Today the error line on stderr says that the branch already exists. That
/// line gives the wrong reason: `nwt` asks git whether the branch is there
/// after a failed add, and the failed hook left it there. Issue #495 tracks
/// that defect. The sparse path of issue #487 names the hook instead, so this
/// test does not hold the words of that line. It holds the exit code, the empty stdout, and what stays on disk
/// and in git.
#[test]
fn a_failed_post_checkout_hook_keeps_the_worktree_and_the_branch() {
    let (temp, repo) = init_repo();
    let hooks = tempfile::TempDir::new().expect("create the hooks directory");
    install_failing_post_checkout_hook(&repo, hooks.path());

    let branch = unique_branch("hook-fails");
    let output = run_nwt(&repo, &branch, &[]);

    let worktree = assert_the_failed_hook_kept_the_worktree(temp.path(), &repo, &branch, &output);
    assert!(
        worktree.join("README.md").is_file(),
        "a plain add writes the files before the hook runs"
    );
}

/// A sparse run whose `post-checkout` hook fails keeps the same contract as a
/// plain add, and its error line names the hook and its exit status.
///
/// The hook runs after the sparse files are written, so the worktree is sparse
/// and stays sparse. The plain add says "already exists", which is the wrong
/// reason. The sparse run must not repeat it.
#[test]
fn a_failed_post_checkout_hook_on_the_sparse_path_keeps_the_worktree_and_the_branch() {
    let (temp, repo) = init_repo();
    let heavy_file = repo.join(HEAVY_DIR).join("big.txt");
    std::fs::create_dir_all(repo.join(HEAVY_DIR)).expect("create the heavy directory");
    std::fs::write(&heavy_file, "big\n").expect("write the heavy file");
    assert!(run_git(&repo, &["add", "--", "."]), "git add failed");
    assert!(
        run_git(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "add heavy"]
        ),
        "git commit failed"
    );

    let hooks = tempfile::TempDir::new().expect("create the hooks directory");
    install_failing_post_checkout_hook(&repo, hooks.path());

    let branch = unique_branch("sparse-hook-fails");
    let output = run_nwt(&repo, &branch, &["--sparse-exclude", HEAVY_DIR]);

    let worktree = assert_the_failed_hook_kept_the_worktree(temp.path(), &repo, &branch, &output);
    assert!(
        worktree.join("README.md").is_file(),
        "the sparse files are written before the hook runs"
    );
    assert!(
        !worktree.join(HEAVY_DIR).exists(),
        "the kept worktree stays sparse: {}",
        worktree.display()
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let named = format!("post-checkout hook failed (exit status: {HOOK_EXIT_STATUS})");
    assert!(
        stderr.contains(&named),
        "stderr must name the hook and its exit status ({named:?}):\n{stderr}"
    );
    assert!(
        !stderr.contains("already exists"),
        "a failed hook is not a branch that already exists:\n{stderr}"
    );
}
