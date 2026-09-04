//! The guard that keeps `gitnuke` blind to a repository whose git directory is
//! detached from its work tree, which is how `yadm` keeps a directory of
//! dotfiles.
//!
//! `yadm` puts the git directory at `~/.local/share/yadm/repo.git`, names
//! `$HOME` as the work tree through `core.worktree`, and leaves no `.git` entry
//! anywhere. `repowalker::find_git_repo` walks the file system upward for a
//! `.git` entry, so it finds no repository in that layout, and `gitnuke`
//! refuses the run.
//!
//! That refusal is the safe answer, and this file is what keeps it. `gitnuke`
//! removes worktrees and deletes branches, and the work tree of this layout is
//! the home directory of the user. Git answers `git rev-parse --show-toplevel`
//! with that work tree from inside the git directory, so a change that made
//! `find_git_repo` ask git instead would aim `gitnuke` at `$HOME`.
//!
//! The safe behaviour is spelled as an absence: a function returns `None` and a
//! tool does nothing. Nothing reports an absence, so only a test holds it. Both
//! shapes of the layout are here, because git answers `--show-toplevel` with
//! the work tree from either one.

use std::path::Path;
use std::process::Command;

use gitscratch::shed_inherited_git_environment;
use gitscratch::testing::{path_at_or_above, DetachedGitDirRepo};

/// The exit code `gitnuke` leaves behind when it finds no repository.
///
/// Spelled out here rather than read from the binary, the way
/// `tests/integration.rs` spells out the whole set: the number is a contract
/// callers script against, and a copy that followed the binary around would pin
/// nothing.
const NOT_IN_REPO: i32 = 1;

/// The words `gitnuke` uses when it finds no repository.
///
/// The prefix `gitnuke:` is painted and this phrase is not, so the assertion
/// reads plain text whatever the child decides about colour.
const NOT_IN_REPO_MESSAGE: &str = "not in a git repository";

/// The target the run names.
///
/// Nothing in the fixture carries this name, so a run that reached a repository
/// would still remove nothing.
const TARGET: &str = "no-such-worktree";

/// Run `gitnuke` in `dir` and hand back its exit code and its whole output.
///
/// `--dry-run` runs every check a real run runs and removes nothing. The guard
/// cannot rest on the refusal it is testing for: a `gitnuke` that found this
/// repository is exactly the `gitnuke` that would delete inside it.
///
/// The inherited `GIT_*` family goes, through
/// [`gitscratch::shed_inherited_git_environment`]. This suite runs under a
/// pre-commit hook that exports `GIT_DIR` and its kin, and `GIT_DIR` beats the
/// working directory, so a leaked one points `gitnuke` at the repository being
/// committed to.
fn run_gitnuke_in(dir: &Path) -> (Option<i32>, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gitnuke"));
    shed_inherited_git_environment(&mut command);

    let output = command
        .args(["--dry-run", TARGET])
        .current_dir(dir)
        .output()
        .expect("run the gitnuke binary");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    (output.status.code(), combined)
}

/// Run `gitnuke` in the git directory of `repo` and prove it found no
/// repository and named no path at or above the work tree.
fn assert_finds_no_repository(repo: &DetachedGitDirRepo) {
    let (code, output) = run_gitnuke_in(repo.git_dir());

    assert_eq!(
        code,
        Some(NOT_IN_REPO),
        "gitnuke must find no repository in a detached git directory, but it \
         exited with {code:?} and said:\n{output}"
    );
    assert!(
        output.contains(NOT_IN_REPO_MESSAGE),
        "gitnuke must say it is {NOT_IN_REPO_MESSAGE}, but it said:\n{output}"
    );
    assert_eq!(
        path_at_or_above(&output, repo.work_tree()),
        None,
        "gitnuke must name no path at or above the work tree {}, but it \
         said:\n{output}",
        repo.work_tree().display()
    );
}

#[test]
fn a_nested_git_directory_holds_no_repository_gitnuke_can_find() {
    assert_finds_no_repository(&DetachedGitDirRepo::nested());
}

#[test]
fn a_git_directory_beside_its_work_tree_holds_no_repository_gitnuke_can_find() {
    assert_finds_no_repository(&DetachedGitDirRepo::beside());
}
