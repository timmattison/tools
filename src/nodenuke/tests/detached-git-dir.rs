//! The guard that keeps `nodenuke` blind to a repository whose git directory is
//! detached from its work tree, which is how `yadm` keeps a directory of
//! dotfiles.
//!
//! `yadm` puts the git directory at `~/.local/share/yadm/repo.git`, names
//! `$HOME` as the work tree through `core.worktree`, and leaves no `.git` entry
//! anywhere. `repowalker::find_git_repo` walks the file system upward for a
//! `.git` entry, so it finds no repository in that layout.
//!
//! `nodenuke` answers that differently from its siblings. It does not stop: it
//! falls back to the directory it runs in and scans there. So the guard reads
//! two things. The line that reports a repository is absent, and the line that
//! names the scan root names the git directory the run started in.
//!
//! This is the safe answer, and this file is what keeps it. `nodenuke` deletes
//! every `node_modules`, `.next`, `.open-next` and `.turbo` directory it walks
//! into, plus the lock files beside them, and it asks nobody first. The work
//! tree of this layout is the home directory of the user. Git answers
//! `git rev-parse --show-toplevel` with that work tree from inside the git
//! directory, so a change that made `find_git_repo` ask git instead would turn
//! a run in `repo.git` into a run over `$HOME`.
//!
//! The safe behaviour is spelled as an absence: a function returns `None` and a
//! tool stays where it is. Nothing reports an absence, so only a test holds it.
//! Both shapes of the layout are here, because git answers `--show-toplevel`
//! with the work tree from either one.
//!
//! The fixture makes the run itself harmless. Every path lives under a
//! throwaway temporary directory that holds one tracked file and a git
//! directory, so nothing `nodenuke` deletes exists anywhere it can reach.

use std::path::{Path, PathBuf};
use std::process::Command;

use gitscratch::shed_inherited_git_environment;
use gitscratch::testing::DetachedGitDirRepo;

/// The words `nodenuke` uses when `find_git_repo` answered.
///
/// The whole line names the root as well, and the guard reads the words alone,
/// because the absence of the report is the thing being pinned.
const FOUND_REPOSITORY_REPORT: &str = "Found git repository";

/// The start of the line that names the directory `nodenuke` scans.
const SCAN_ROOT_PREFIX: &str = "Starting to scan from: ";

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
/// `$HOME`, and its ancestors hold every other user of the machine, so a
/// destructive tool must name neither. `starts_with` on the work tree is true
/// for exactly that set: the work tree itself and each directory above it.
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

/// Run `nodenuke` in `dir` and hand back its whole output.
///
/// The run takes no flags. `--no-root` would skip `find_git_repo` altogether,
/// which is the one call this file is about.
///
/// The inherited `GIT_*` family goes, through
/// [`gitscratch::shed_inherited_git_environment`]. This suite runs under a
/// pre-commit hook that exports `GIT_DIR` and its kin, and `GIT_DIR` beats the
/// working directory, so a leaked one points anything that asks git at the
/// repository being committed to.
fn run_nodenuke_in(dir: &Path) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nodenuke"));
    shed_inherited_git_environment(&mut command);

    let output = command
        .current_dir(dir)
        .output()
        .expect("run the nodenuke binary");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    assert!(
        output.status.success(),
        "nodenuke failed in {}:\n{combined}",
        dir.display()
    );

    combined
}

/// The directory `nodenuke` said it would scan.
///
/// # Panics
///
/// Panics when the run printed no such line, because every run prints one.
fn scan_root(output: &str) -> PathBuf {
    let line = output
        .lines()
        .find_map(|line| line.strip_prefix(SCAN_ROOT_PREFIX))
        .unwrap_or_else(|| panic!("nodenuke must say where it scans, but it said:\n{output}"));

    PathBuf::from(line.trim())
}

/// Run `nodenuke` in the git directory of `repo` and prove it found no
/// repository, scanned the directory it ran in, and named no path at or above
/// the work tree.
fn assert_finds_no_repository(repo: &DetachedGitDirRepo) {
    let output = run_nodenuke_in(repo.git_dir());

    assert!(
        !output.contains(FOUND_REPOSITORY_REPORT),
        "nodenuke must report no repository in a detached git directory, but it \
         said:\n{output}"
    );
    assert_eq!(
        canonical(&scan_root(&output)),
        canonical(repo.git_dir()),
        "nodenuke must scan the directory it ran in, and it said:\n{output}"
    );
    assert_eq!(
        path_at_or_above(&output, repo.work_tree()),
        None,
        "nodenuke must name no path at or above the work tree {}, but it \
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
fn a_nested_git_directory_holds_no_repository_nodenuke_can_find() {
    assert_finds_no_repository(&DetachedGitDirRepo::nested());
}

#[test]
fn a_git_directory_beside_its_work_tree_holds_no_repository_nodenuke_can_find() {
    assert_finds_no_repository(&DetachedGitDirRepo::beside());
}
