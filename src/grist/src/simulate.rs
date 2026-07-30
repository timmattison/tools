//! Replays a candidate merge ordering against throwaway git state.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::metrics::{BranchName, OrderingScore};

/// Measures what a candidate ordering would cost to carry out for real.
pub struct Simulator {
    repo: PathBuf,
    base: String,
}

impl Simulator {
    /// Simulate against `repo`, landing branches on top of `base`.
    #[must_use]
    pub fn new(repo: impl Into<PathBuf>, base: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            base: base.into(),
        }
    }

    /// The repository being simulated against.
    #[must_use]
    pub fn repo(&self) -> &Path {
        &self.repo
    }

    /// The base ref that branches land on top of.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Replay `order` and report what resolving it would cost.
    ///
    /// # Errors
    ///
    /// Returns an error if git is unavailable, the scratch worktree cannot be
    /// created, or a branch in `order` does not resolve.
    pub fn score(&self, _order: &[BranchName]) -> Result<OrderingScore> {
        todo!("replay the ordering and count what conflicts")
    }
}
