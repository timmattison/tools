//! Test-only git fixtures: throwaway repositories to run gsw's git code against.
//!
//! Every unit test that needs a real repository builds one here rather than
//! reaching for the checkout the suite happens to be running in. The helpers
//! are shared across modules ([`crate::repo`] exercises the git reads,
//! [`crate::watch`] exercises the refresh loop) so the *isolation* rules below
//! are stated and enforced in exactly one place — a copy that drifted would
//! reintroduce the fixture-writes-to-the-real-repo failure mode silently.
//!
//! Every fixture lives in its own [`tempfile::TempDir`], so the suite stays
//! parallel-safe: two concurrent runs of the same test never share a path.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// Run a git command in `dir`, isolated from the host's global/system config,
/// asserting success.
///
/// Scrubs inherited `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE` so the fixture
/// repo under `dir` is the one git operates on. Without this, when the suite
/// runs from inside this repo's own pre-commit hook (git exports those vars for
/// the hook), the fixture's commits would land in the *real* repo despite
/// `current_dir(dir)`.
///
/// # Panics
///
/// Panics if git cannot be invoked, or if it exits non-zero — use
/// [`git_allowing_failure`] for commands whose failure is part of the fixture.
pub(crate) fn git(dir: &Path, args: &[&str]) {
    let status = command(dir, args).status().expect("invoke git");
    assert!(status.success(), "git {args:?} failed");
}

/// Run a git command in `dir` with the same isolation as [`git`], but tolerate a
/// non-zero exit.
///
/// Some fixtures are *built* out of git failures: `git merge` exits non-zero
/// when it leaves conflicts behind, which is precisely the state a test of
/// conflict rendering needs. Asserting success there would fail the test on the
/// very thing it is arranging.
///
/// # Panics
///
/// Panics only if git cannot be invoked at all.
pub(crate) fn git_allowing_failure(dir: &Path, args: &[&str]) {
    let _ = command(dir, args).status().expect("invoke git");
}

/// A git invocation in `dir` with the host's config and any hook-exported git
/// location scrubbed. The single place the isolation rules are applied; both
/// [`git`] and [`git_allowing_failure`] differ only in how they treat the exit
/// status.
fn command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    cmd
}

/// A fresh repo on branch `main` with one commit (`a.txt` = `"initial\n"`).
/// Parallel-safe: unique tempdir.
///
/// The returned [`TempDir`] owns the repo — dropping it deletes the fixture, so
/// callers must hold it for as long as they read the repository.
pub(crate) fn init_repo() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    identity(p);
    std::fs::write(p.join("a.txt"), "initial\n").expect("write a.txt");
    git(p, &["add", "a.txt"]);
    git(p, &["commit", "-q", "-m", "initial"]);
    dir
}

/// Clone [`init_repo`]'s repo so the clone has a real `origin/main` upstream,
/// returning `(origin, clone)`.
///
/// Both [`TempDir`]s must be held: dropping the origin deletes the remote the
/// clone's `origin` points at, which breaks any later fetch or push.
pub(crate) fn init_repo_with_upstream() -> (TempDir, TempDir) {
    let origin = init_repo();
    let clone = tempfile::tempdir().expect("tempdir");
    // Both paths are absolute, so the cwd only has to exist; `clone` is an
    // empty directory, which `git clone` accepts as the destination.
    git(
        clone.path(),
        &[
            "clone",
            "-q",
            origin.path().to_str().expect("utf-8 tempdir path"),
            clone.path().to_str().expect("utf-8 tempdir path"),
        ],
    );
    identity(clone.path());
    (origin, clone)
}

/// An [`init_repo`] repo plus a linked worktree checked out on its own branch,
/// returning `(repo, linked_worktree_path)`.
///
/// A linked worktree is the only layout where the work-tree root holds a `.git`
/// *file* — a `gitdir:` pointer at `<repo>/.git/worktrees/<name>` — instead of a
/// `.git` directory, and following that pointer to the shared config is a
/// distinct code path from reading `<root>/.git/config` directly. It is also the
/// layout this repository mandates all development happen in, so fixtures built
/// only from [`init_repo`] and [`init_repo_with_upstream`] leave the path gsw
/// runs against most as the one path nothing covers.
///
/// The worktree is deliberately nested *inside* the repo's tempdir so a single
/// [`TempDir`] owns both halves: dropping it removes the checkout and the
/// `.git/worktrees` administrative directory that describes it together, with no
/// second directory to leak. That [`TempDir`] must therefore be held for as long
/// as the caller reads *either* repository — the returned path points inside it,
/// so dropping the [`TempDir`] invalidates the path as well.
pub(crate) fn init_repo_with_worktree() -> (TempDir, PathBuf) {
    let repo = init_repo();
    let linked = repo.path().join("linked");
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "linked",
            linked.to_str().expect("utf-8 tempdir path"),
        ],
    );
    (repo, linked)
}

/// Give the repo at `dir` a committer identity and disable signing, so commits
/// succeed no matter how the host's (scrubbed) global config is set up.
fn identity(dir: &Path) {
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}
