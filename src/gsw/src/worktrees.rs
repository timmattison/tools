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
