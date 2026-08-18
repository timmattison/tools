//! The main worktree, and the ladder of them above the one the user stands in.
//!
//! `wtm` means "take me to my main worktree". A user whose directory already
//! is that worktree has asked for the next thing up: the repository that holds
//! theirs. This module owns both halves of that question — which branch names a
//! main worktree, and how to climb out of one.

use std::path::{Path, PathBuf};

use crate::worktree::{canonical, is_checkout, list_worktrees, paths_equal, Worktree};

/// The branch names that identify the main worktree, in order of priority.
///
/// `main` comes first. `master` is the fallback for a repository that never
/// renamed its first branch.
pub const MAIN_BRANCH_NAMES: [&str; 2] = ["main", "master"];

/// How strongly `branch` claims to be the main branch.
///
/// `Some(0)` for the first name of [`MAIN_BRANCH_NAMES`], `Some(1)` for the one
/// after it, and `None` for a branch that is none of them. A detached worktree
/// has no branch, so it never claims anything.
///
/// The name must match exactly. A substring match is wrong here: in a
/// repository that has no `main` branch, a branch such as `wt-main-master`
/// would capture the shortcut and send the user somewhere that is not the main
/// worktree.
pub fn main_branch_rank(branch: Option<&str>) -> Option<usize> {
    let branch = branch?;
    MAIN_BRANCH_NAMES.iter().position(|name| *name == branch)
}

/// The main worktree among `worktrees`, or `None` when none of them is on a
/// main branch.
fn main_worktree_of(worktrees: &[Worktree]) -> Option<&Worktree> {
    worktrees
        .iter()
        .filter_map(|worktree| {
            main_branch_rank(worktree.branch.as_deref()).map(|rank| (rank, worktree))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, worktree)| worktree)
}

/// The main worktree of the nearest repository above `from`.
///
/// `from` is the main worktree of the user's repository — the checkout that
/// owns the `.git` directory, which is where that repository sits on disk. The
/// repository that holds it is the one checked out in the directory directly
/// above, the same one level down that the family scan looks. So the climb goes
/// repository by repository rather than directory by directory, and the first
/// directory that is not a checkout ends it.
///
/// A repository on the ladder that has no worktree on a main branch cannot be a
/// destination, so the climb steps over it and asks the repository above it.
/// This is the one place the rule parts from the first press of `wtm`: a
/// repository the user chose to stand in owes them an error when it has no main
/// branch, and a repository they only pass through owes them nothing.
///
/// Returns `None` when no repository above `from` has a main worktree.
pub fn climb(from: &Path) -> Option<PathBuf> {
    // A repository above can be a linked worktree whose own repository sits
    // somewhere else, so the climb is not guaranteed to walk toward the root.
    // Remembering the repositories already asked is what keeps a tangle of
    // worktrees from looping forever.
    let mut asked = vec![canonical(from)];
    let mut dir = from.to_path_buf();

    loop {
        let parent = dir.parent()?;
        if !is_checkout(parent) {
            return None;
        }

        // A repository that will not be read ends the climb. The family scan
        // reports such a repository as a warning because it is missing from a
        // listing the user can see; here there is no listing, and the message
        // the caller prints already says no repository above had a main
        // worktree.
        let repo = list_worktrees(parent).ok()?;
        let key = canonical(&repo.main);
        if asked.iter().any(|seen| paths_equal(seen, &key)) {
            return None;
        }
        asked.push(key);

        if let Some(worktree) = main_worktree_of(&repo.all) {
            return Some(worktree.path.clone());
        }

        // This repository has no main worktree to offer. Where it sits on disk
        // is where the climb goes on from.
        dir = repo.main;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One worktree at `path`, with `branch` checked out.
    fn worktree(path: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            path: PathBuf::from(path),
            head: "abc1234567890".to_string(),
            branch: branch.map(str::to_string),
        }
    }

    #[test]
    fn main_ranks_ahead_of_master() {
        assert_eq!(main_branch_rank(Some("main")), Some(0));
        assert_eq!(main_branch_rank(Some("master")), Some(1));
    }

    #[test]
    fn every_name_of_the_constant_is_ranked() {
        // The ranking is built from the constant, so a name added to it cannot
        // leave the search behind.
        for (position, name) in MAIN_BRANCH_NAMES.iter().enumerate() {
            assert_eq!(
                main_branch_rank(Some(name)),
                Some(position),
                "{name} is in the constant, so it must have a rank"
            );
        }
    }

    #[test]
    fn a_branch_that_merely_contains_a_main_name_is_not_ranked() {
        assert_eq!(main_branch_rank(Some("wt-main-master")), None);
        assert_eq!(main_branch_rank(Some("mainline")), None);
        assert_eq!(main_branch_rank(Some("Main")), None);
    }

    #[test]
    fn a_detached_worktree_is_not_ranked() {
        assert_eq!(main_branch_rank(None), None);
    }

    #[test]
    fn the_main_worktree_of_a_list_prefers_main_over_master() {
        let worktrees = [
            worktree("/repo", Some("master")),
            worktree("/repo-wt/new", Some("main")),
        ];
        assert_eq!(
            main_worktree_of(&worktrees).map(|found| found.path.clone()),
            Some(PathBuf::from("/repo-wt/new")),
            "main wins wherever it sits in the list"
        );
    }

    #[test]
    fn the_main_worktree_of_a_list_falls_back_to_master() {
        let worktrees = [
            worktree("/repo", Some("trunk")),
            worktree("/repo-wt/old", Some("master")),
        ];
        assert_eq!(
            main_worktree_of(&worktrees).map(|found| found.path.clone()),
            Some(PathBuf::from("/repo-wt/old"))
        );
    }

    #[test]
    fn a_list_without_a_main_branch_has_no_main_worktree() {
        let worktrees = [
            worktree("/repo", Some("trunk")),
            worktree("/repo-wt/x", None),
        ];
        assert_eq!(
            main_worktree_of(&worktrees).map(|found| found.path.clone()),
            None
        );
    }

    #[test]
    fn the_climb_stops_where_the_directories_run_out() {
        // The root of the filesystem has no parent, so the climb has nothing to
        // ask and must not loop looking for one.
        assert_eq!(climb(Path::new("/")), None);
    }

    #[test]
    fn the_climb_stops_at_a_directory_that_is_not_a_checkout() {
        let root = std::env::temp_dir();
        let repo = root.join("no-parent-checkout-should-exist-here");
        assert_eq!(climb(&repo), None);
    }
}
