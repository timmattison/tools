//! The questions worth asking *before* a scratch worktree is ever built.
//!
//! Creating a [`Scratch`](crate::Scratch) is not free: a temporary directory,
//! a real `git worktree add`, and administrative state in the developer's
//! repository that has to be cleaned up afterwards. Paying all of that only to
//! discover the branch name was a typo is both slow and, worse, misleading —
//! the failure arrives looking like a failed simulation rather than a bad
//! argument.
//!
//! [`Repo`] is the pre-flight: open the repository, resolve the revisions, see
//! whether the tree is dirty, and only then decide whether a replay is worth
//! starting. It exists here rather than in each consuming tool because
//! [`Git`](crate::Git) is deliberately crate-private — nothing outside this
//! crate may build a git runner rooted at a real repository — so the queries a
//! caller legitimately needs have to be part of this crate's public door.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// A git repository, opened for the read-only questions that precede a replay.
pub struct Repo {
    path: PathBuf,
}

impl Repo {
    /// Open the git repository containing `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned, or if `path` is not inside
    /// a git repository.
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    /// The directory the repository was opened at — what
    /// [`Scratch::create`](crate::Scratch::create) wants.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Resolve a revision to a full commit id, without creating a scratch
    /// worktree.
    ///
    /// # Errors
    ///
    /// Returns an error if the revision does not name a commit; the message
    /// names the revision that could not be resolved.
    pub fn resolve(&self, _revision: &str) -> Result<String> {
        anyhow::bail!("not implemented")
    }

    /// How many files are uncommitted — staged, unstaged, or untracked.
    ///
    /// # Errors
    ///
    /// Returns an error if git could not be spawned or reported a failure.
    pub fn uncommitted_files(&self) -> Result<usize> {
        anyhow::bail!("not implemented")
    }
}
