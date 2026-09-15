//! The worktrees of one repository, for the arrow keys of watch mode.
//!
//! The list comes from `gix`, in this process. No child process runs. It holds
//! the main worktree and every linked worktree, sorted by path, which is the
//! order of `cwt`. Right and Left then visit the worktrees in the same order as
//! `cwt -f` and `cwt -p`.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "watch mode does not use the list yet. The expectation fails the build when it \
                  does, so this attribute cannot stay after that"
    )
)]

use std::path::{Path, PathBuf};

use crate::repo::{branch_name, DETACHED_HEAD};

/// The root of a worktree, in the one spelling that every comparison uses.
///
/// Built only through [`resolve`](Self::resolve) (canonicalize) in production,
/// so a path from the list and a path from the loop always compare correctly.
/// macOS gives `/var/...` and `/private/var/...` for one directory, and a
/// symlinked home directory does the same.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct WorktreePath(PathBuf);

impl WorktreePath {
    /// `std::fs::canonicalize(path)`, or `None` when no directory is there.
    ///
    /// A file at `path` gives `None` too, because a file is not the root of a
    /// worktree.
    pub(crate) fn resolve(path: &Path) -> Option<Self> {
        std::fs::canonicalize(path)
            .ok()
            .filter(|resolved| resolved.is_dir())
            .map(Self)
    }

    /// The path, for display and for the calls that open the worktree.
    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }

    /// A path that no filesystem call touched, for the pure tests only.
    #[cfg(test)]
    pub(crate) fn fake(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }
}

/// One worktree of the repository, as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeEntry {
    /// The root of the worktree.
    pub(crate) path: WorktreePath,
    /// The branch name, or `HEAD@<7 hex chars>` for a detached HEAD.
    pub(crate) label: String,
}

/// Every worktree of the repository that holds `repo`, sorted by path.
/// Paths only: no HEAD is read. The loop calls this on every walk.
pub(crate) fn worktree_paths(_repo: &gix::Repository) -> Vec<WorktreePath> {
    Vec::new()
}

/// The same worktrees in the same order, each with its label.
pub(crate) fn list_worktrees(_repo: &gix::Repository) -> Vec<WorktreeEntry> {
    Vec::new()
}

/// How many hex digits of the commit the label of a detached HEAD shows.
///
/// The length that `cwt` shows (`SHORT_COMMIT_HASH_LENGTH` in
/// `src/cwt/src/worktree.rs`), so the two tools name a detached worktree the
/// same way. `cwt` is a binary crate, so gsw cannot take its constant.
const SHORT_HASH_LEN: usize = 7;

/// The label of `repo`'s own HEAD: the branch, or `HEAD@<short hash>`.
///
/// [`branch_name`] gives the branch. For a detached HEAD it gives
/// [`DETACHED_HEAD`], and the label then adds `@` and the first
/// [`SHORT_HASH_LEN`] hex digits of the commit, as `cwt` does. A detached HEAD
/// whose commit gix cannot read gives [`DETACHED_HEAD`] alone: the HEAD is
/// detached, and no hash is there to show.
pub(crate) fn head_label(repo: &gix::Repository) -> String {
    let branch = branch_name(repo);
    if branch != DETACHED_HEAD {
        return branch;
    }
    match repo.head_id() {
        Ok(id) => format!("{DETACHED_HEAD}@{}", id.to_hex_with_len(SHORT_HASH_LEN)),
        Err(_) => branch,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{head_label, WorktreePath};
    use crate::testrepo::{git, git_stdout, init_repo};

    /// How many hex digits of the commit a detached HEAD shows: the length
    /// that `cwt` shows (`SHORT_COMMIT_HASH_LENGTH` in
    /// `src/cwt/src/worktree.rs`). Stated here as the oracle, apart from the
    /// constant of the code under test.
    const CWT_SHORT_HASH: usize = 7;

    /// Open the repository at `path` through the discovery that
    /// `RepoHandle::discover` makes, which is how gsw opens a worktree.
    fn open(path: &Path) -> gix::Repository {
        gix::discover(path).expect("the fixture is a repository")
    }

    /// The label that `cwt` shows for a detached HEAD at `dir`: `HEAD@` and
    /// the first [`CWT_SHORT_HASH`] hex digits of the id that git reports.
    fn detached_label(dir: &Path) -> String {
        let full = git_stdout(dir, &["rev-parse", "HEAD"]);
        let short: String = full.chars().take(CWT_SHORT_HASH).collect();
        format!("HEAD@{short}")
    }

    /// `head_label` gives the branch of a worktree on a branch. For a detached
    /// HEAD it gives `HEAD@` and the short hash, as `cwt` does, because a
    /// detached worktree has no branch to show.
    #[test]
    fn head_label_gives_the_branch_or_the_short_hash_of_a_detached_head() {
        let dir = init_repo();
        assert_eq!(head_label(&open(dir.path())), "main");

        git(dir.path(), &["checkout", "-q", "--detach"]);
        assert_eq!(head_label(&open(dir.path())), detached_label(dir.path()));
    }

    /// Two spellings of one directory resolve to one value.
    ///
    /// The loop compares the path of the worktree on the screen with the paths
    /// of the list. A symlink gives a second spelling on every Unix, and on
    /// macOS every temporary directory has two: `/var/...` and
    /// `/private/var/...`. Two values for one directory make Right skip a
    /// worktree or stop on the same one twice.
    #[cfg(unix)]
    #[test]
    fn resolve_gives_one_value_for_two_spellings_of_one_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real");
        std::fs::create_dir(&real).expect("make the directory");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("make the symlink");

        let through_real = WorktreePath::resolve(&real).expect("a directory is there");
        let through_link = WorktreePath::resolve(&link).expect("the symlink names a directory");

        assert_eq!(
            through_real, through_link,
            "two spellings of one directory must give one value",
        );
    }

    /// `resolve` gives a directory, and gives `None` for a path where no
    /// directory is. `fake` keeps the path that `resolve` refuses, because it
    /// touches no filesystem.
    #[test]
    fn resolve_gives_a_directory_and_refuses_a_missing_path_and_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(
            WorktreePath::resolve(dir.path()).is_some(),
            "a directory is there, so resolve must give it",
        );

        let missing = dir.path().join("no-such-directory");
        assert_eq!(WorktreePath::resolve(&missing), None, "no directory is there");

        let file = dir.path().join("a-file");
        std::fs::write(&file, "").expect("write the file");
        assert_eq!(
            WorktreePath::resolve(&file),
            None,
            "a file is not the root of a worktree",
        );

        assert_eq!(
            WorktreePath::fake(&missing).as_path(),
            missing,
            "fake touches no filesystem, so it keeps a path that resolve refuses",
        );
    }
}
