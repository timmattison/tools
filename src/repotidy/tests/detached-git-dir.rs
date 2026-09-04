//! The guard that keeps `repotidy` blind to a repository whose git directory is
//! detached from its work tree, which is how `yadm` keeps a directory of
//! dotfiles.
//!
//! `yadm` puts the git directory at `~/.local/share/yadm/repo.git`, names
//! `$HOME` as the work tree through `core.worktree`, and leaves no `.git` entry
//! anywhere. `repowalker::find_git_repo` walks the file system upward for a
//! `.git` entry, so it finds no repository in that layout, and `repotidy`
//! refuses the run.
//!
//! That refusal is the safe answer, and this file is what keeps it. `repotidy`
//! walks the root it is given and runs `go mod tidy` in every directory that
//! holds a `go.mod`, which rewrites files. The work tree of this layout is the
//! home directory of the user. Git answers `git rev-parse --show-toplevel` with
//! that work tree from inside the git directory, so a change that made
//! `find_git_repo` ask git instead would aim `repotidy` at `$HOME`.
//!
//! The safe behaviour is spelled as an absence: a function returns `None` and a
//! tool does nothing. Nothing reports an absence, so only a test holds it. Both
//! shapes of the layout are here, because git answers `--show-toplevel` with
//! the work tree from either one.
//!
//! The fixture makes the run itself harmless. Every path lives under a
//! throwaway temporary directory that holds one tracked file and a git
//! directory, so no `go.mod` exists anywhere `repotidy` can reach.

use std::path::Path;
use std::process::Command;

use gitscratch::shed_inherited_git_environment;
use gitscratch::testing::{path_at_or_above, DetachedGitDirRepo};

/// The exit code `repotidy` leaves behind when it finds no repository.
const NO_REPOSITORY: i32 = 1;

/// The words `repotidy` uses when it finds no repository.
const NO_REPOSITORY_MESSAGE: &str = "Could not find git repository";

/// Run `repotidy` in `dir` and hand back its exit code and its whole output.
///
/// The inherited `GIT_*` family goes, through
/// [`gitscratch::shed_inherited_git_environment`]. This suite runs under a
/// pre-commit hook that exports `GIT_DIR` and its kin, and `GIT_DIR` beats the
/// working directory, so a leaked one points anything that asks git at the
/// repository being committed to.
fn run_repotidy_in(dir: &Path) -> (Option<i32>, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_repotidy"));
    shed_inherited_git_environment(&mut command);

    let output = command
        .current_dir(dir)
        .output()
        .expect("run the repotidy binary");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    (output.status.code(), combined)
}

/// Run `repotidy` in the git directory of `repo` and prove it found no
/// repository and named no path at or above the work tree.
fn assert_finds_no_repository(repo: &DetachedGitDirRepo) {
    let (code, output) = run_repotidy_in(repo.git_dir());

    assert_eq!(
        code,
        Some(NO_REPOSITORY),
        "repotidy must find no repository in a detached git directory, but it \
         exited with {code:?} and said:\n{output}"
    );
    assert!(
        output.contains(NO_REPOSITORY_MESSAGE),
        "repotidy must say it {NO_REPOSITORY_MESSAGE}, but it said:\n{output}"
    );
    assert_eq!(
        path_at_or_above(&output, repo.work_tree()),
        None,
        "repotidy must name no path at or above the work tree {}, but it \
         said:\n{output}",
        repo.work_tree().display()
    );
}

#[test]
fn a_nested_git_directory_holds_no_repository_repotidy_can_find() {
    assert_finds_no_repository(&DetachedGitDirRepo::nested());
}

#[test]
fn a_git_directory_beside_its_work_tree_holds_no_repository_repotidy_can_find() {
    assert_finds_no_repository(&DetachedGitDirRepo::beside());
}
