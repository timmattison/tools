//! End-to-end coverage for `nwt.worktreesDir`, the git configuration key that
//! names where a repository keeps its worktrees.
//!
//! `nwt` puts every new worktree in `<main worktree>-worktrees`, beside the main
//! worktree. That default is right for a normal repository and for a repository
//! whose git directory is detached from its work tree, but it is not right for
//! every layout. A repository states its own answer with this key, and the key
//! lives in the repository configuration, which `yadm` already tracks.
//!
//! The default itself stays under test in `tests/detached-git-dir.rs`, which
//! runs `nwt` with the key unset in both repository shapes.

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

use gitscratch::testing::DetachedGitDirRepo;
use support::{init_repo, nanos, nwt_command, run_git};
use tempfile::TempDir;

/// The git configuration key that names the worktrees directory.
const WORKTREES_DIR_KEY: &str = "nwt.worktreesDir";

/// Resolve a path before an assertion reads it.
///
/// Git prints resolved paths, and every fixture lives under a temporary
/// directory that macOS reaches through a symbolic link: `/var` resolves to
/// `/private/var`.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
}

/// A branch name that no concurrent copy of this suite can also hold.
///
/// Two `cargo test` runs share one machine, and a branch name is a shared
/// resource. The process id and a nanosecond clock reading keep them apart.
fn unique_branch(label: &str) -> String {
    format!("{label}-{}-{}", std::process::id(), nanos())
}

/// Run `nwt -b <branch>` in `from`, and hand back everything the run produced.
///
/// The two flags keep the run short and silent. `--no-copy-env` stops a walk of
/// the repository for `.env` files, and `--no-bootstrap-hooks` stops a package
/// manager install. Neither one has anything to do with where the worktree
/// lands.
fn run_nwt(from: &Path, branch: &str) -> Output {
    nwt_command(from)
        .args(["-b", branch, "--no-copy-env", "--no-bootstrap-hooks"])
        .output()
        .expect("run the nwt binary")
}

/// Run `nwt -b <branch>` in `from`, and hand back the directory it made.
fn created_worktree(from: &Path, branch: &str) -> PathBuf {
    let output = run_nwt(from, branch);

    assert!(
        output.status.success(),
        "nwt -b {branch} failed in {}:\n{}\n{}",
        from.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let printed = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string());
    assert!(
        printed.is_dir(),
        "nwt printed {}, which is no directory",
        printed.display()
    );

    printed
}

/// Point the repository at `worktrees_dir` through the configuration key.
fn set_override(repo: &Path, worktrees_dir: &str) {
    assert!(
        run_git(repo, &["config", WORKTREES_DIR_KEY, worktrees_dir]),
        "git config {WORKTREES_DIR_KEY} failed in {}",
        repo.display()
    );
}

#[test]
fn an_absolute_value_places_the_worktree_in_a_detached_repository() {
    let repo = DetachedGitDirRepo::nested();
    let elsewhere = TempDir::new().expect("create the directory that holds the override");
    let worktrees_dir = elsewhere.path().join("stated-worktrees");
    repo.git(&[
        "config",
        WORKTREES_DIR_KEY,
        worktrees_dir.to_str().expect("utf-8 override path"),
    ]);
    let branch = unique_branch("detached-absolute");

    let created = created_worktree(repo.git_dir(), &branch);

    assert_eq!(
        canonical(&created),
        canonical(elsewhere.path())
            .join("stated-worktrees")
            .join(&branch),
        "nwt must obey {WORKTREES_DIR_KEY} in a repository with a detached git directory"
    );
}

#[test]
fn an_absolute_value_places_the_worktree_in_a_normal_repository() {
    let (temp, repo) = init_repo();
    let worktrees_dir = temp.path().join("elsewhere").join("stated-worktrees");
    set_override(&repo, worktrees_dir.to_str().expect("utf-8 override path"));
    let branch = unique_branch("normal-absolute");

    let created = created_worktree(&repo, &branch);

    assert_eq!(
        canonical(&created),
        canonical(temp.path())
            .join("elsewhere")
            .join("stated-worktrees")
            .join(&branch),
        "nwt must obey {WORKTREES_DIR_KEY} in a normal repository"
    );
}
