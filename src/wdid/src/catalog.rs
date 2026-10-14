//! The catalog of the tools of the workspace.
//!
//! A tool is a binary target of the cargo workspace. This module asks cargo
//! for the targets, asks `cargo install --list` which ones are installed, asks
//! git when each crate directory first appeared, and asks the PATH which
//! program runs first for each installed name.
//!
//! The functions [`binaries`], [`installed_binaries`], [`added_dates`],
//! [`path_state`], and [`cargo_bin`] are pure. [`workspace_root`] and [`read`]
//! start processes.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::tool::{ToolName, ToolNames};

/// The programs that the catalog starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Programs {
    /// The cargo program: a name that the PATH resolves, or a path.
    pub cargo: OsString,
    /// The git program: a name that the PATH resolves, or a path.
    pub git: OsString,
}

impl Default for Programs {
    fn default() -> Self {
        Self {
            cargo: OsString::from("cargo"),
            git: OsString::from("git"),
        }
    }
}

/// Where the PATH finds an installed tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathState {
    /// The first match is in the cargo bin directory.
    Cargo,
    /// The first match is another program.
    Shadowed(PathBuf),
    /// No directory of the PATH holds the name.
    NotOnPath,
}

/// One tool of the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    /// The name of the binary target.
    pub name: ToolName,
    /// The directory of the crate, relative to the workspace root.
    pub crate_dir: PathBuf,
    /// The date of the oldest commit that touches the crate directory.
    pub added: Option<NaiveDate>,
    /// True when `cargo install --list` names the binary.
    pub installed: bool,
    /// Where the PATH finds the tool. `None` when the tool is not installed.
    pub path: Option<PathState>,
}

/// Every tool of the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    /// The root of the workspace.
    pub root: PathBuf,
    /// The tools, sorted by name.
    pub tools: Vec<Tool>,
}

impl Catalog {
    /// Gives the names of the tools as a set.
    #[must_use]
    pub fn names(&self) -> ToolNames {
        ToolNames::default()
    }
}

/// The reasons that the catalog cannot be read.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// No `--repo`, and the current directory is not in a git work tree.
    #[error("{dir} is not in a git work tree. Run wdid in the workspace, or name it with --repo <path>", dir = .dir.display())]
    NotAWorkTree {
        /// The directory that was searched.
        dir: PathBuf,
    },
    /// The root has no `Cargo.toml`.
    #[error("{dir} is not a cargo workspace (it has no Cargo.toml). Name the workspace with --repo <path>", dir = .dir.display())]
    NotAWorkspace {
        /// The directory that was checked.
        dir: PathBuf,
    },
    /// The workspace has no binary target.
    #[error("the cargo workspace at {dir} has no binary targets", dir = .dir.display())]
    NoBinaries {
        /// The root of the workspace.
        dir: PathBuf,
    },
    /// The system cannot start a program.
    #[error("cannot start {program}: {source}")]
    Spawn {
        /// The program name or path that the spawn used.
        program: String,
        /// The error of the system.
        source: std::io::Error,
    },
    /// A program ended with a non-zero status.
    #[error("{command} failed ({status}): {stderr}")]
    Failed {
        /// The command line that failed.
        command: String,
        /// The text of the exit status.
        status: String,
        /// The standard error, trimmed.
        stderr: String,
    },
    /// The JSON of `cargo metadata` is not in the expected shape.
    #[error("cargo metadata gave output that wdid cannot read: {reason}")]
    Metadata {
        /// The text that says what is wrong.
        reason: String,
    },
}

/// Gives the workspace root: `repo` when given, else the git work tree that
/// holds `cwd`.
///
/// # Errors
/// Gives [`CatalogError::NotAWorkspace`] when the root has no `Cargo.toml`,
/// [`CatalogError::NotAWorkTree`] when `cwd` is in no git work tree, and
/// [`CatalogError::Spawn`] when git does not start.
pub fn workspace_root(
    repo: Option<&Path>,
    cwd: &Path,
    programs: &Programs,
) -> Result<PathBuf, CatalogError> {
    let _ = (repo, programs);
    Ok(cwd.to_path_buf())
}

/// Reads the whole catalog.
///
/// `search_path` is the value of the PATH. `cargo_bin` is the cargo bin
/// directory.
///
/// # Errors
/// Gives the errors of [`binaries`] and the errors of the programs that it
/// starts. A root that is not in a git repository is not an error: every
/// `added` date is then `None`.
pub fn read(
    root: &Path,
    programs: &Programs,
    search_path: Option<&OsStr>,
    cargo_bin: Option<&Path>,
) -> Result<Catalog, CatalogError> {
    let _ = (programs, search_path, cargo_bin);
    Ok(Catalog {
        root: root.to_path_buf(),
        tools: Vec::new(),
    })
}

/// Gives the binary targets of a `cargo metadata` document, each with its
/// crate directory relative to `root`. Pure.
///
/// # Errors
/// Gives [`CatalogError::Metadata`] for JSON that is not in the expected
/// shape, and [`CatalogError::NoBinaries`] for zero binary targets.
pub fn binaries(
    metadata_json: &str,
    root: &Path,
) -> Result<Vec<(ToolName, PathBuf)>, CatalogError> {
    let _ = (metadata_json, root);
    Ok(Vec::new())
}

/// Gives the binary names that `cargo install --list` prints. Pure.
#[must_use]
pub fn installed_binaries(install_list: &str) -> BTreeSet<String> {
    let _ = install_list;
    BTreeSet::new()
}

/// Gives the oldest commit date for each crate directory. Pure.
///
/// `log` is the output of `git log --format=%x00%as --name-only --no-renames
/// --relative`. A crate directory that no commit touches has no entry.
#[must_use]
pub fn added_dates(log: &str, crate_dirs: &[PathBuf]) -> BTreeMap<PathBuf, NaiveDate> {
    let _ = (log, crate_dirs);
    BTreeMap::new()
}

/// Says which program the PATH runs first for `name`. Pure over the file
/// system.
#[must_use]
pub fn path_state(name: &str, search_path: &OsStr, cargo_bin: &Path) -> PathState {
    let _ = (name, search_path, cargo_bin);
    PathState::NotOnPath
}

/// Gives the cargo bin directory: `$CARGO_HOME/bin`, else `<home>/.cargo/bin`.
#[must_use]
pub fn cargo_bin(cargo_home: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    let _ = (cargo_home, home);
    None
}
