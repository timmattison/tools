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
use std::process::{Command, Output, Stdio};

use chrono::NaiveDate;

use crate::tool::{ToolName, ToolNames};

/// The file that marks a cargo workspace.
const MANIFEST: &str = "Cargo.toml";

/// The arguments that give the top level of the git work tree.
const TOP_LEVEL_ARGUMENTS: [&str; 2] = ["rev-parse", "--show-toplevel"];

/// The arguments of `cargo metadata`, before the manifest path.
const METADATA_ARGUMENTS: [&str; 5] = [
    "metadata",
    "--no-deps",
    "--format-version",
    "1",
    "--manifest-path",
];

/// The text that names `cargo metadata` in an error.
const METADATA_COMMAND: &str = "cargo metadata";

/// The arguments that list the installed binaries.
const INSTALL_ARGUMENTS: [&str; 2] = ["install", "--list"];

/// The text that names `cargo install --list` in an error.
const INSTALL_COMMAND: &str = "cargo install --list";

/// The arguments of the one `git log` that gives the added dates. A record
/// starts with NUL and a date, then the files of the commit.
///
/// `core.quotepath=false` comes first. Without it, git writes a path that
/// holds a byte outside ASCII as a quoted octal text (`"drei-\303\274/x"`),
/// and that text matches no crate directory.
const LOG_ARGUMENTS: [&str; 7] = [
    "-c",
    "core.quotepath=false",
    "log",
    "--format=%x00%as",
    "--name-only",
    "--no-renames",
    "--relative",
];

/// The mode bits that make a file executable for its owner, group, or others.
#[cfg(unix)]
const EXECUTE_BITS: u32 = 0o111;

/// The name of the directory that holds the binaries of cargo.
const BIN_DIRECTORY: &str = "bin";

/// The name of the cargo directory in the home directory.
const CARGO_DIRECTORY: &str = ".cargo";

/// The kind of a cargo target that is a binary.
const BINARY_KIND: &str = "bin";

/// The byte that starts each record of the git log.
const LOG_RECORD_START: char = '\0';

/// The format of the date that `%as` prints.
const LOG_DATE_FORMAT: &str = "%Y-%m-%d";

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
        self.tools.iter().map(|tool| tool.name.clone()).collect()
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
    if let Some(repo) = repo {
        let canonical = repo
            .canonicalize()
            .map_err(|_| CatalogError::NotAWorkspace {
                dir: repo.to_path_buf(),
            })?;
        return require_manifest(canonical);
    }
    let mut command = Command::new(&programs.git);
    gitscratch::shed_inherited_git_environment_keeping_user_intent(&mut command);
    command
        .args(TOP_LEVEL_ARGUMENTS)
        .current_dir(cwd)
        .stdin(Stdio::null());
    let output = run(&mut command, &programs.git)?;
    if !output.status.success() {
        return Err(CatalogError::NotAWorkTree {
            dir: cwd.to_path_buf(),
        });
    }
    let top_level = String::from_utf8_lossy(&output.stdout);
    require_manifest(PathBuf::from(top_level.trim_end_matches(['\n', '\r'])))
}

/// Gives `root` when it holds a `Cargo.toml`, else
/// [`CatalogError::NotAWorkspace`].
fn require_manifest(root: PathBuf) -> Result<PathBuf, CatalogError> {
    if root.join(MANIFEST).is_file() {
        Ok(root)
    } else {
        Err(CatalogError::NotAWorkspace { dir: root })
    }
}

/// Runs `command` to its end with both output pipes read. `program` names the
/// program in an error.
fn run(command: &mut Command, program: &OsStr) -> Result<Output, CatalogError> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|source| CatalogError::Spawn {
            program: program.to_string_lossy().into_owned(),
            source,
        })
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
    let manifest = root.join(MANIFEST);
    let mut metadata_command = Command::new(&programs.cargo);
    metadata_command
        .args(METADATA_ARGUMENTS)
        .arg(&manifest)
        .stdin(Stdio::null());
    let metadata = run_checked(&mut metadata_command, &programs.cargo, METADATA_COMMAND)?;
    let found = binaries(&String::from_utf8_lossy(&metadata.stdout), root)?;

    let mut install_command = Command::new(&programs.cargo);
    install_command.args(INSTALL_ARGUMENTS).stdin(Stdio::null());
    let install = run_checked(&mut install_command, &programs.cargo, INSTALL_COMMAND)?;
    let installed = installed_binaries(&String::from_utf8_lossy(&install.stdout));

    let crate_dirs: Vec<PathBuf> = found.iter().map(|(_, dir)| dir.clone()).collect();
    let dates = read_added_dates(root, programs, &crate_dirs)?;

    let tools = found
        .into_iter()
        .map(|(name, crate_dir)| {
            let is_installed = installed.contains(name.as_str());
            let path = is_installed.then(|| match (search_path, cargo_bin) {
                (Some(search_path), Some(cargo_bin)) => {
                    path_state(name.as_str(), search_path, cargo_bin)
                }
                (Some(search_path), None) => path_state(name.as_str(), search_path, Path::new("")),
                (None, _) => PathState::NotOnPath,
            });
            Tool {
                added: dates.get(&crate_dir).copied(),
                name,
                crate_dir,
                installed: is_installed,
                path,
            }
        })
        .collect();
    Ok(Catalog {
        root: root.to_path_buf(),
        tools,
    })
}

/// Runs `command` and gives its output. A non-zero status is
/// [`CatalogError::Failed`]. `label` names the command in that error.
fn run_checked(
    command: &mut Command,
    program: &OsStr,
    label: &str,
) -> Result<Output, CatalogError> {
    let output = run(command, program)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(CatalogError::Failed {
            command: label.to_string(),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

/// Asks git for the added date of each crate directory. A root that is not in
/// a git repository gives an empty map, because the dates are optional.
fn read_added_dates(
    root: &Path,
    programs: &Programs,
    crate_dirs: &[PathBuf],
) -> Result<BTreeMap<PathBuf, NaiveDate>, CatalogError> {
    let mut command = Command::new(&programs.git);
    gitscratch::shed_inherited_git_environment_keeping_user_intent(&mut command);
    command
        .args(LOG_ARGUMENTS)
        .current_dir(root)
        .stdin(Stdio::null());
    let output = run(&mut command, &programs.git)?;
    if !output.status.success() {
        return Ok(BTreeMap::new());
    }
    Ok(added_dates(
        &String::from_utf8_lossy(&output.stdout),
        crate_dirs,
    ))
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
    let document: MetadataDocument =
        serde_json::from_str(metadata_json).map_err(|error| CatalogError::Metadata {
            reason: error.to_string(),
        })?;
    let canonical_root = canonical_or_plain(root);
    let mut found: Vec<(ToolName, PathBuf)> = Vec::new();
    for package in &document.packages {
        let manifest = Path::new(&package.manifest_path);
        let directory = manifest.parent().unwrap_or(manifest);
        let crate_dir = relative_to(&canonical_root, directory);
        for target in &package.targets {
            if target.kind.iter().any(|kind| kind == BINARY_KIND) {
                found.push((ToolName::new(target.name.as_str()), crate_dir.clone()));
            }
        }
    }
    if found.is_empty() {
        return Err(CatalogError::NoBinaries {
            dir: root.to_path_buf(),
        });
    }
    found.sort();
    Ok(found)
}

/// The part of the `cargo metadata` document that wdid reads.
#[derive(serde::Deserialize)]
struct MetadataDocument {
    packages: Vec<MetadataPackage>,
}

/// One package of the `cargo metadata` document.
#[derive(serde::Deserialize)]
struct MetadataPackage {
    manifest_path: String,
    targets: Vec<MetadataTarget>,
}

/// One target of a package.
#[derive(serde::Deserialize)]
struct MetadataTarget {
    name: String,
    kind: Vec<String>,
}

/// Gives the canonical form of a path, or the path itself when the file system
/// cannot canonicalize it.
fn canonical_or_plain(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Gives `directory` relative to the canonical `root`. Both sides are
/// canonical before the cut, because a symlink can name one directory in two
/// ways. A directory outside the root keeps its absolute path.
fn relative_to(canonical_root: &Path, directory: &Path) -> PathBuf {
    let canonical_directory = canonical_or_plain(directory);
    canonical_directory
        .strip_prefix(canonical_root)
        .map_or_else(|_| directory.to_path_buf(), Path::to_path_buf)
}

/// Gives the binary names that `cargo install --list` prints. Pure.
#[must_use]
pub fn installed_binaries(install_list: &str) -> BTreeSet<String> {
    install_list
        .lines()
        .filter(|line| line.starts_with(char::is_whitespace))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(String::from)
        .collect()
}

/// Gives the oldest commit date for each crate directory. Pure.
///
/// `log` is the output of `git log --format=%x00%as --name-only --no-renames
/// --relative`. A crate directory that no commit touches has no entry.
#[must_use]
pub fn added_dates(log: &str, crate_dirs: &[PathBuf]) -> BTreeMap<PathBuf, NaiveDate> {
    let mut oldest: BTreeMap<PathBuf, NaiveDate> = BTreeMap::new();
    for record in log.split(LOG_RECORD_START) {
        let mut lines = record.lines();
        let Some(date) = lines
            .next()
            .and_then(|line| NaiveDate::parse_from_str(line.trim(), LOG_DATE_FORMAT).ok())
        else {
            continue;
        };
        let files: Vec<&Path> = lines
            .filter(|line| !line.is_empty())
            .map(Path::new)
            .collect();
        for crate_dir in crate_dirs {
            if files.iter().any(|file| file.starts_with(crate_dir)) {
                oldest
                    .entry(crate_dir.clone())
                    .and_modify(|known| *known = (*known).min(date))
                    .or_insert(date);
            }
        }
    }
    oldest
}

/// Says which program the PATH runs first for `name`. Pure over the file
/// system.
#[must_use]
pub fn path_state(name: &str, search_path: &OsStr, cargo_bin: &Path) -> PathState {
    let cargo_bin_canonical = canonical_or_plain(cargo_bin);
    for directory in std::env::split_paths(search_path) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        let candidate = directory.join(name);
        if !is_executable_file(&candidate) {
            continue;
        }
        let same_directory =
            directory == cargo_bin || canonical_or_plain(&directory) == cargo_bin_canonical;
        return if same_directory {
            PathState::Cargo
        } else {
            PathState::Shadowed(candidate)
        };
    }
    PathState::NotOnPath
}

/// The values that say where `cargo install` puts binaries.
///
/// Each field holds the value that the caller read. A field that is `None`
/// or empty counts as not set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InstallLocation<'a> {
    /// The value of `CARGO_INSTALL_ROOT`.
    pub install_root: Option<&'a OsStr>,
    /// The value of `CARGO_HOME`.
    pub cargo_home: Option<&'a OsStr>,
    /// The home directory.
    pub home: Option<&'a Path>,
}

/// Gives the cargo bin directory, which is the directory where `cargo install`
/// puts binaries.
///
/// The order is the order of cargo. A `install_root` that is set gives
/// `<install_root>/bin`. Else a `cargo_home` that is set gives
/// `<cargo_home>/bin`. Else `home` gives `<home>/.cargo/bin`. Else the result
/// is `None`.
///
/// This function does not read `install.root` from the cargo config file. It
/// does not know the `--root` flag of a past install.
#[must_use]
pub fn cargo_bin(location: &InstallLocation<'_>) -> Option<PathBuf> {
    match location.cargo_home {
        Some(cargo_home) if !cargo_home.is_empty() => {
            Some(Path::new(cargo_home).join(BIN_DIRECTORY))
        }
        _ => location
            .home
            .map(|home| home.join(CARGO_DIRECTORY).join(BIN_DIRECTORY)),
    }
}

/// True when `path` is a file (a symlink is followed) with an execute bit.
fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && has_execute_bit(&metadata))
}

/// True when one of the execute bits of the mode is on.
#[cfg(unix)]
fn has_execute_bit(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & EXECUTE_BITS != 0
}

/// A file system without execute bits runs any file that the PATH finds.
#[cfg(not(unix))]
fn has_execute_bit(_metadata: &std::fs::Metadata) -> bool {
    true
}
