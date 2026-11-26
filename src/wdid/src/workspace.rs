//! The cargo workspace that `wdid` reports on, and the step that found it.
//!
//! [`find`] tries these steps, in this order:
//!
//! 1. `--repo <path>`.
//! 2. The git work tree of the current directory, when it holds a
//!    `Cargo.toml`.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::catalog::{self, CatalogError, InstallLocation, Programs};

/// The number of characters of a commit in the short form.
const SHORT_COMMIT_LENGTH: usize = 7;

/// The full id of a commit, as the record of an install gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit(String);

impl Commit {
    /// Gives the full id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Gives the first seven characters of the id, the form that git and the
    /// directory names of cargo use.
    #[must_use]
    pub fn short(&self) -> &str {
        self.0.get(..SHORT_COMMIT_LENGTH).unwrap_or(&self.0)
    }
}

impl fmt::Display for Commit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The step that found the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FoundBy {
    /// `--repo <path>`.
    RepoFlag,
    /// The git work tree of the current directory.
    WorkTree,
    /// The checkout of the commit that `cargo install --git` installed.
    InstalledGit {
        /// The URL of the repository, as the record gives it.
        url: String,
        /// The commit of the record.
        commit: Commit,
    },
    /// The workspace that holds the crate directory of `cargo install --path`.
    InstalledPath {
        /// The crate directory that the record names.
        crate_dir: PathBuf,
    },
}

/// The workspace that `wdid` reports on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// The root of the workspace: the directory that holds its `Cargo.toml`.
    pub root: PathBuf,
    /// The step that found the workspace.
    pub found_by: FoundBy,
}

/// The reasons that [`find`] gives no workspace.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// A step failed in a way that stops the search.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
}

/// Finds the workspace that `wdid` reports on.
///
/// `repo` is the value of `--repo`. `cwd` is the current directory.
/// `location` says where cargo keeps the records of the installs and its
/// checkouts. `binary` is the name of the installed binary whose source is
/// the workspace of step 3.
///
/// # Errors
/// Gives the error of `--repo` when `repo` is given and it is not a cargo
/// workspace. Gives [`CatalogError::NotAWorkTree`] and
/// [`CatalogError::NotAWorkspace`] when no step finds a workspace, and
/// [`CatalogError::Spawn`] when git does not start.
pub fn find(
    repo: Option<&Path>,
    cwd: &Path,
    programs: &Programs,
    _location: &InstallLocation<'_>,
    _binary: &str,
) -> Result<Workspace, WorkspaceError> {
    let found_by = if repo.is_some() {
        FoundBy::RepoFlag
    } else {
        FoundBy::WorkTree
    };
    let root = catalog::workspace_root(repo, cwd, programs)?;
    Ok(Workspace { root, found_by })
}
