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
    pub(crate) fn resolve(_path: &Path) -> Option<Self> {
        None
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

/// The label of `repo`'s own HEAD: the branch, or `HEAD@<short hash>`.
pub(crate) fn head_label(_repo: &gix::Repository) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::WorktreePath;

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
