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

use std::path::{Path, PathBuf};
use std::process::Command;

use gitscratch::shed_inherited_git_environment;
use gitscratch::testing::DetachedGitDirRepo;

/// The exit code `repotidy` leaves behind when it finds no repository.
const NO_REPOSITORY: i32 = 1;

/// The words `repotidy` uses when it finds no repository.
const NO_REPOSITORY_MESSAGE: &str = "Could not find git repository";

/// Punctuation that a printed path can carry on either end.
///
/// Trimmed before a token is read as a path, so a path inside quotes or before
/// a comma still reaches the comparison.
const TRIMMED_PUNCTUATION: &str = "\"'`,;:()[]{}";

/// Resolve a path before an assertion reads it.
///
/// Every fixture lives under a temporary directory that macOS reaches through a
/// symbolic link: `/var` resolves to `/private/var`. Git and the tools print
/// the resolved form.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
}

/// Resolve `path` when the file system can, and hand it back as it is when it
/// cannot.
///
/// A path the tool printed can name something that no longer exists, and such a
/// path still has to reach the comparison.
fn resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The first path in `output` that is `work_tree` or an ancestor of it.
///
/// This is the whole hazard in one function. The work tree stands in for
/// `$HOME`, and its ancestors hold every other user of the machine, so a tool
/// that rewrites files must name neither. `starts_with` on the work tree is
/// true for exactly that set: the work tree itself and each directory above it.
fn path_at_or_above(output: &str, work_tree: &Path) -> Option<PathBuf> {
    let work_tree = canonical(work_tree);

    output
        .split_whitespace()
        .map(|token| token.trim_matches(|c| TRIMMED_PUNCTUATION.contains(c)))
        .map(Path::new)
        .filter(|token| token.is_absolute())
        .map(resolved)
        .find(|candidate| work_tree.starts_with(candidate))
}

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

/// Prove the path check can fail, before a clean answer from it is trusted.
///
/// The run assertions rest on `path_at_or_above` answering `None`, and a
/// matcher that never matches answers `None` for every input. A guard that
/// reports clean for the wrong reason is the defect this whole file exists to
/// stop, so the check gets the same treatment it gives the tool.
///
/// Four plants: the work tree, the directory above it, the same work tree
/// inside quotes and before a comma, and the git directory, which the nested
/// shape keeps under the work tree. The first three must match and the last one
/// must not.
#[test]
fn the_path_check_flags_the_work_tree_and_the_directory_above_it() {
    let repo = DetachedGitDirRepo::nested();
    let work_tree = canonical(repo.work_tree());
    let above = work_tree
        .parent()
        .expect("the work tree has a parent")
        .to_path_buf();

    assert_eq!(
        path_at_or_above(&format!("root: {}", work_tree.display()), repo.work_tree()),
        Some(work_tree.clone()),
        "the check must flag the work tree itself"
    );
    assert_eq!(
        path_at_or_above(&format!("root: {}", above.display()), repo.work_tree()),
        Some(above),
        "the check must flag a directory above the work tree"
    );
    assert_eq!(
        path_at_or_above(
            &format!("root: \"{}\", and more", work_tree.display()),
            repo.work_tree()
        ),
        Some(work_tree),
        "the check must flag a path that carries punctuation on either end"
    );
    assert_eq!(
        path_at_or_above(
            &format!("root: {}", repo.git_dir().display()),
            repo.work_tree()
        ),
        None,
        "the check must pass a directory under the work tree"
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
