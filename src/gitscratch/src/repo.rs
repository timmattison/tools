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

use anyhow::{Context, Result};

use crate::git::Git;

/// Where hook lookups are pointed while answering a pre-flight question.
///
/// [`Scratch`](crate::Scratch) creates a real, empty directory for this because
/// a replay runs commands - `rebase`, `commit`, `checkout` - that genuinely
/// fire hooks, and an empty `core.hooksPath` is not "hooks off": git still
/// resolves hook lookups against it. Nothing here is such a command. Every
/// query on a [`Repo`] is a read (`rev-parse`, `status`), and reads fire no
/// hook at all, so the redirect only has to name somewhere that will never hold
/// an executable.
///
/// That distinction is worth the const rather than a `TempDir`, because it is
/// what makes the pre-flight unconditionally cheap: telling a developer they
/// typo'd a branch name must not be able to fail for want of a writable
/// temporary directory, and must not leave a scratch worktree behind on the way
/// to saying so. A relative path this crate never creates keeps the redirect
/// pointed somewhere harmless without touching the filesystem at all.
const PREFLIGHT_HOOKS_PATH: &str = ".git/gitscratch-preflight-no-hooks";

/// A git repository, opened for the read-only questions that precede a replay.
#[derive(Debug)]
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
        let repo = Self {
            path: path.to_path_buf(),
        };

        // Asking git where the repository is proves one exists, and proves it
        // from `path` itself - so a subdirectory of a repository opens fine,
        // while somewhere outside every repository fails now rather than
        // halfway through a simulation.
        repo.git()
            .run(&["rev-parse", "--git-dir"])
            .with_context(|| format!("{} is not inside a git repository", path.display()))?;

        Ok(repo)
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

    /// A runner rooted in the real repository, carrying the crate's safety
    /// configuration like every other git call made from here.
    fn git(&self) -> Git {
        Git::new(&self.path, PREFLIGHT_HOOKS_PATH)
    }
}
