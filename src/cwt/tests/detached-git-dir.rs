//! End-to-end coverage for a repository whose git directory is detached from
//! its work tree, which is how `yadm` keeps a directory of dotfiles.
//!
//! `yadm` puts the git directory at `~/.local/share/yadm/repo.git`, names
//! `$HOME` as the work tree, and leaves no `.git` entry anywhere. A search that
//! walks the file system upward for a `.git` entry therefore finds no
//! repository, so `cwt` has to ask git instead.
//!
//! Git names the main worktree of that layout as the git directory itself. It
//! builds the name from the common git directory with a trailing `/.git`
//! removed, and this layout carries no such suffix. So the git directory is the
//! path `cwt` lists, and the path `--main` navigates to.
//!
//! The tests here prove the whole round trip. Each one that navigates runs
//! `cwt` again in the path the first run printed, because a shortcut that lands
//! the user where no worktree command works is a one-way trip.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use std::path::{Path, PathBuf};

use gitscratch::testing::DetachedGitDirRepo;
use support::{code, combined, cwt, parse_listing, stdout, target_path};

/// The exit code `cwt` uses when it finds no worktree.
const WORKTREE_NOT_FOUND: i32 = 3;

/// The branches of the two linked worktrees each fixture adds.
///
/// Two, and not one, so the cycling flags have more than one step to make and a
/// wrap that skips a worktree shows.
const LINKED_BRANCHES: [&str; 2] = ["wt-a-branch", "wt-b-branch"];

/// Resolve a path before an assertion reads it.
///
/// Git prints resolved paths, and the fixture lives under a temporary directory
/// that macOS reaches through a symbolic link: `/var` resolves to
/// `/private/var`.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
}

/// A detached-git-directory repository with two linked worktrees beside the
/// tracked dotfile.
///
/// The two shapes differ only in where the git directory sits, so every test
/// runs against both. [`Dotfiles::nested`] is the shape `yadm` builds, with the
/// git directory inside the work tree. [`Dotfiles::beside`] puts it outside,
/// where git reports the directory as inside the git directory and outside the
/// work tree.
struct Dotfiles {
    repo: DetachedGitDirRepo,
    linked: Vec<PathBuf>,
}

impl Dotfiles {
    /// The shape `yadm` builds: the git directory inside the work tree.
    fn nested() -> Self {
        Self::with_linked_worktrees(DetachedGitDirRepo::nested())
    }

    /// The git directory outside the work tree, so neither one contains the
    /// other.
    fn beside() -> Self {
        Self::with_linked_worktrees(DetachedGitDirRepo::beside())
    }

    /// Add one linked worktree per branch in [`LINKED_BRANCHES`].
    ///
    /// They go inside the work tree, which keeps them inside the fixture's own
    /// temporary directory in both shapes. That directory removes everything
    /// under it when the fixture drops.
    fn with_linked_worktrees(repo: DetachedGitDirRepo) -> Self {
        let linked = LINKED_BRANCHES
            .iter()
            .map(|branch| repo.add_worktree(&repo.work_tree().join(branch), branch))
            .collect();

        Self { repo, linked }
    }

    /// The git directory, which is the main worktree of this layout.
    fn git_dir(&self) -> &Path {
        self.repo.git_dir()
    }

    /// The linked worktree on `LINKED_BRANCHES[index]`.
    fn linked(&self, index: usize) -> &Path {
        &self.linked[index]
    }

    /// Every worktree of the repository, resolved, in no particular order.
    fn all_worktrees(&self) -> Vec<PathBuf> {
        let mut all = vec![canonical(self.git_dir())];
        all.extend(self.linked.iter().map(|path| canonical(path)));
        all
    }
}

/// Assert that a plain `cwt` run in `from` lists every worktree of `dotfiles`,
/// and marks `from` as the current one.
fn assert_lists_every_worktree(dotfiles: &Dotfiles, from: &Path) {
    let output = cwt(from, &[]);
    assert_eq!(
        code(&output),
        0,
        "cwt failed in {}: {}",
        from.display(),
        combined(&output)
    );

    let listing = parse_listing(&stdout(&output));
    let mut listed: Vec<PathBuf> = listing
        .iter()
        .map(|entry| canonical(Path::new(&entry.path)))
        .collect();
    listed.sort();

    let mut expected = dotfiles.all_worktrees();
    expected.sort();
    assert_eq!(listed, expected, "cwt listed the wrong set of worktrees");

    let current: Vec<PathBuf> = listing
        .iter()
        .filter(|entry| entry.current)
        .map(|entry| canonical(Path::new(&entry.path)))
        .collect();
    assert_eq!(
        current,
        vec![canonical(from)],
        "cwt marked the wrong worktree as the current one"
    );
}

/// The path a navigating `cwt` run printed, after proving the run succeeded and
/// that `cwt` works in the directory it named.
///
/// The second half is what this whole file is about. A shortcut that prints a
/// directory where every worktree command fails is a one-way trip, so every
/// target is tested by running `cwt` there.
fn navigate(dotfiles: &Dotfiles, from: &Path, args: &[&str]) -> PathBuf {
    let output = cwt(from, args);
    assert_eq!(
        code(&output),
        0,
        "cwt {args:?} failed in {}: {}",
        from.display(),
        combined(&output)
    );

    let target = PathBuf::from(target_path(&output));
    assert_lists_every_worktree(dotfiles, &target);
    target
}

/// Assert that `flag` visits every worktree once and comes back to the start.
///
/// Each step runs from the directory the step before it named, which is the way
/// a user meets the flag: `wtf` moves them, and then they press it again.
fn assert_cycles_through_every_worktree(dotfiles: &Dotfiles, flag: &str) {
    let start = canonical(dotfiles.git_dir());
    let all = dotfiles.all_worktrees();

    let mut here = start.clone();
    let mut visited = Vec::new();
    for _ in 0..all.len() {
        here = canonical(&navigate(dotfiles, &here, &[flag]));
        visited.push(here.clone());
    }

    assert_eq!(
        visited.last(),
        Some(&start),
        "cwt {flag} must come back to where it started"
    );

    let mut sorted = visited.clone();
    sorted.sort();
    sorted.dedup();
    let mut expected = all;
    expected.sort();
    assert_eq!(
        sorted, expected,
        "cwt {flag} must visit every worktree exactly once"
    );
}

#[test]
fn a_nested_git_directory_lists_every_worktree() {
    let dotfiles = Dotfiles::nested();

    assert_lists_every_worktree(&dotfiles, dotfiles.git_dir());
}

#[test]
fn a_git_directory_beside_its_work_tree_lists_every_worktree() {
    let dotfiles = Dotfiles::beside();

    assert_lists_every_worktree(&dotfiles, dotfiles.git_dir());
}

#[test]
fn a_worktree_of_a_nested_repository_lists_every_worktree() {
    let dotfiles = Dotfiles::nested();

    assert_lists_every_worktree(&dotfiles, dotfiles.linked(0));
}

#[test]
fn a_worktree_of_a_beside_repository_lists_every_worktree() {
    let dotfiles = Dotfiles::beside();

    assert_lists_every_worktree(&dotfiles, dotfiles.linked(0));
}

#[test]
fn forward_cycles_through_every_worktree_of_a_nested_repository() {
    let dotfiles = Dotfiles::nested();

    assert_cycles_through_every_worktree(&dotfiles, "-f");
}

#[test]
fn forward_cycles_through_every_worktree_of_a_beside_repository() {
    let dotfiles = Dotfiles::beside();

    assert_cycles_through_every_worktree(&dotfiles, "-f");
}

#[test]
fn previous_cycles_through_every_worktree_of_a_nested_repository() {
    let dotfiles = Dotfiles::nested();

    assert_cycles_through_every_worktree(&dotfiles, "-p");
}

#[test]
fn previous_cycles_through_every_worktree_of_a_beside_repository() {
    let dotfiles = Dotfiles::beside();

    assert_cycles_through_every_worktree(&dotfiles, "-p");
}

#[test]
fn main_from_a_worktree_of_a_nested_repository_is_the_git_directory() {
    let dotfiles = Dotfiles::nested();

    let target = navigate(&dotfiles, dotfiles.linked(0), &["--main"]);

    assert_eq!(canonical(&target), canonical(dotfiles.git_dir()));
}

#[test]
fn main_from_a_worktree_of_a_beside_repository_is_the_git_directory() {
    let dotfiles = Dotfiles::beside();

    let target = navigate(&dotfiles, dotfiles.linked(0), &["--main"]);

    assert_eq!(canonical(&target), canonical(dotfiles.git_dir()));
}

#[test]
fn a_name_selects_the_worktree_that_carries_it() {
    let dotfiles = Dotfiles::nested();

    let target = navigate(&dotfiles, dotfiles.git_dir(), &[LINKED_BRANCHES[1]]);

    assert_eq!(canonical(&target), canonical(dotfiles.linked(1)));
}

#[test]
fn main_from_the_git_directory_itself_reports_the_top_of_the_climb() {
    // The user stands at the main worktree, so `--main` climbs instead. Nothing
    // above the git directory is a repository, so the climb ends there. This is
    // what a top-level repository of any shape does.
    let dotfiles = Dotfiles::nested();

    let output = cwt(dotfiles.git_dir(), &["--main"]);

    assert_eq!(code(&output), WORKTREE_NOT_FOUND);
    assert!(
        combined(&output).contains("has a main worktree"),
        "cwt must say that no repository above this one has a main worktree, got: {}",
        combined(&output)
    );
}
