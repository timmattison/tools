//! The main worktree, and the ladder of them above the one the user stands in.
//!
//! `wtm` means "take me to my main worktree". A user whose directory already
//! is that worktree has asked for the next thing up: the repository that holds
//! theirs. This module owns both halves of that question — which branch names a
//! main worktree, and how to climb out of one.

use std::path::{Path, PathBuf};

use crate::worktree::{
    canonical, is_checkout, list_worktrees, paths_equal, RepoWorktrees, Worktree,
};

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
    climb_with(from, |dir| {
        is_checkout(dir).then(|| list_worktrees(dir).ok()).flatten()
    })
}

/// The climb itself, over whatever ladder `list` describes.
///
/// `list` answers "the repository checked out at this directory", and `None` is
/// its answer for every directory the climb cannot go on from. That folds two
/// cases the climb has always treated alike: a directory that is not a checkout
/// at all, and a checkout whose worktrees will not list. Neither can name a
/// destination, and the message the caller prints — that no repository above had
/// a main worktree — is true of both, so one `Option`-returning reader says
/// exactly what the climb needs to know. The family scan reports an unreadable
/// repository as a warning because it is missing from a listing the user can
/// see; here there is no listing.
///
/// Taking the reader as an argument is also the only seam a test has: the guard
/// below ends a climb that revisits a repository, and git will not build the
/// on-disk tangle that would exercise it.
fn climb_with(from: &Path, list: impl Fn(&Path) -> Option<RepoWorktrees>) -> Option<PathBuf> {
    // A repository above can be a linked worktree whose own repository sits
    // somewhere else, so the climb is not guaranteed to walk toward the root.
    // Remembering the repositories already asked is what keeps a tangle of
    // worktrees from looping forever.
    let mut asked = vec![canonical(from)];
    let mut dir = from.to_path_buf();

    loop {
        let parent = dir.parent()?;
        let repo = list(parent)?;
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
    use std::cell::Cell;

    use super::*;

    /// One worktree at `path`, with `branch` checked out.
    fn worktree(path: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            path: PathBuf::from(path),
            head: "abc1234567890".to_string(),
            branch: branch.map(str::to_string),
        }
    }

    /// One rung of a synthetic ladder: the repository whose main worktree sits
    /// at `main`, holding one worktree per entry of `worktrees`.
    fn rung(main: &str, worktrees: &[(&str, Option<&str>)]) -> RepoWorktrees {
        RepoWorktrees {
            main: PathBuf::from(main),
            all: worktrees
                .iter()
                .map(|(path, branch)| worktree(path, *branch))
                .collect(),
        }
    }

    /// How many listings a climb over a synthetic ladder may ask for before the
    /// test calls it a runaway. Every ladder here is a few rungs long, so this
    /// sits far above a healthy climb and far below a hang.
    const LOOKUP_LIMIT: usize = 32;

    /// A reader over the synthetic ladder `rungs`, which pairs a directory with
    /// the repository checked out there. A directory no rung names is off the
    /// ladder, and the climb ends there.
    ///
    /// It counts its calls and panics past [`LOOKUP_LIMIT`]. That bound is what
    /// turns a broken cycle guard into a loud, fast test failure instead of a
    /// hang that takes the whole suite with it.
    fn ladder<'a>(
        rungs: &'a [(&'a str, RepoWorktrees)],
        lookups: &'a Cell<usize>,
    ) -> impl Fn(&Path) -> Option<RepoWorktrees> + 'a {
        move |dir| {
            lookups.set(lookups.get() + 1);
            assert!(
                lookups.get() <= LOOKUP_LIMIT,
                "the climb asked for {} listings over a ladder of {} rungs, so it is not terminating",
                lookups.get(),
                rungs.len()
            );
            rungs
                .iter()
                .find(|(at, _)| Path::new(at) == dir)
                .map(|(_, repo)| repo.clone())
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
        // The directory above the one the climb starts from is one this test
        // just made and owns, so the answer says something about the climb
        // rather than about wherever the temporary directory happens to live.
        let root = tempfile::TempDir::new().expect("a temporary directory to climb out of");
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).expect("a directory for the climb to start from");
        assert_eq!(climb(&repo), None);
    }

    #[test]
    fn the_climb_answers_with_the_first_repository_above_that_has_a_main_worktree() {
        let lookups = Cell::new(0);
        let rungs = [(
            "/nest",
            rung(
                "/nest/holder",
                &[
                    ("/nest/holder", Some("main")),
                    ("/nest/holder-wt/feature", Some("feature")),
                ],
            ),
        )];

        assert_eq!(
            climb_with(Path::new("/nest/held"), ladder(&rungs, &lookups)),
            Some(PathBuf::from("/nest/holder")),
            "the repository above is on main, so the climb ends at its main worktree"
        );
        assert_eq!(
            lookups.get(),
            1,
            "one rung answered, so one listing was read"
        );
    }

    #[test]
    fn the_climb_steps_over_a_repository_with_no_main_branch() {
        let lookups = Cell::new(0);
        let rungs = [
            ("/one/two", rung("/one/two", &[("/one/two", Some("trunk"))])),
            ("/one", rung("/one", &[("/one", Some("main"))])),
        ];

        assert_eq!(
            climb_with(Path::new("/one/two/repo"), ladder(&rungs, &lookups)),
            Some(PathBuf::from("/one")),
            "the rung in between is on trunk, so the climb passes through it"
        );
    }

    #[test]
    fn the_climb_stops_when_the_ladder_leads_back_to_where_it_started() {
        // Two repositories that hold each other. Neither is on a main branch, so
        // nothing but the record of what has been asked ends this.
        let lookups = Cell::new(0);
        let rungs = [
            (
                "/tangle/first",
                rung(
                    "/tangle/second/repo",
                    &[("/tangle/second/repo", Some("trunk"))],
                ),
            ),
            (
                "/tangle/second",
                rung(
                    "/tangle/first/repo",
                    &[("/tangle/first/repo", Some("trunk"))],
                ),
            ),
        ];

        assert_eq!(
            climb_with(Path::new("/tangle/first/repo"), ladder(&rungs, &lookups)),
            None,
            "the second rung names the repository the climb started from"
        );
        assert_eq!(
            lookups.get(),
            2,
            "the climb stopped at the rung that repeated, not one rung later"
        );
    }

    #[test]
    fn the_climb_stops_when_the_ladder_revisits_a_repository_it_already_asked() {
        // The revisited repository is one the climb reached on the way, not the
        // one it started from, so this is the record the loop itself keeps.
        let lookups = Cell::new(0);
        let rungs = [
            (
                "/tangle/start",
                rung(
                    "/tangle/first/repo",
                    &[("/tangle/first/repo", Some("trunk"))],
                ),
            ),
            (
                "/tangle/first",
                rung(
                    "/tangle/second/repo",
                    &[("/tangle/second/repo", Some("trunk"))],
                ),
            ),
            (
                "/tangle/second",
                rung(
                    "/tangle/first/repo",
                    &[("/tangle/first/repo", Some("trunk"))],
                ),
            ),
        ];

        assert_eq!(
            climb_with(Path::new("/tangle/start/repo"), ladder(&rungs, &lookups)),
            None,
            "the third rung names a repository the climb has already asked"
        );
        assert_eq!(
            lookups.get(),
            3,
            "the climb stopped at the rung that repeated, not one rung later"
        );
    }

    #[test]
    fn the_climb_stops_where_the_ladder_ends() {
        // A directory the reader will not answer for — one that is not a
        // checkout, or a checkout whose worktrees will not list — ends the
        // climb rather than being stepped over.
        let lookups = Cell::new(0);
        let rungs: [(&str, RepoWorktrees); 0] = [];

        assert_eq!(
            climb_with(Path::new("/off/the/ladder"), ladder(&rungs, &lookups)),
            None
        );
        assert_eq!(lookups.get(), 1, "the climb asked once and took the answer");
    }
}
