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
use gitscratch::testing::{canonical, path_at_or_above, DetachedGitDirRepo};

/// The words `nodenuke` uses when `find_git_repo` answered.
///
/// The whole line names the root as well, and the guard reads the words alone,
/// because the absence of the report is the thing being pinned.
const FOUND_REPOSITORY_REPORT: &str = "Found git repository";

/// The start of the line that names the directory `nodenuke` scans.
const SCAN_ROOT_PREFIX: &str = "Starting to scan from: ";

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

#[test]
fn a_nested_git_directory_holds_no_repository_nodenuke_can_find() {
    assert_finds_no_repository(&DetachedGitDirRepo::nested());
}

#[test]
fn a_git_directory_beside_its_work_tree_holds_no_repository_nodenuke_can_find() {
    assert_finds_no_repository(&DetachedGitDirRepo::beside());
}
