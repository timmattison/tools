//! The cargo workspace that `wdid` reports on, and the step that found it.
//!
//! [`find`] tries these steps, in this order:
//!
//! 1. `--repo <path>`.
//! 2. The git work tree of the current directory, when it holds a
//!    `Cargo.toml`.
//! 3. The source of the installed binary. `cargo install` keeps a record of
//!    each install in `.crates2.json` in its install root. The key of the
//!    record of a `cargo install --git` names the repository and the full
//!    commit, and cargo keeps a checkout of that commit in
//!    `<cargo home>/git/checkouts/<repository name>-<hash>/<short commit>`.
//!    The record of a `cargo install --path` names the crate directory, and
//!    the workspace is the cargo workspace that holds that directory.
//! 4. An error. It says why step 2 failed, it gives the reason that step 3
//!    failed, and it names `--repo`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::catalog::{self, CatalogError, InstallLocation, Programs};

/// The number of characters of a commit in the short form.
const SHORT_COMMIT_LENGTH: usize = 7;

/// The lengths of a full commit id: SHA-1 and SHA-256.
const FULL_COMMIT_LENGTHS: [usize; 2] = [40, 64];

/// The file in the install root that holds the records of the installs.
const RECORDS_FILE: &str = ".crates2.json";

/// What starts the source of a record of `cargo install --git`.
const GIT_SOURCE_PREFIX: &str = "git+";

/// What starts the source of a record of `cargo install --path`, before the
/// file URL.
const PATH_SOURCE_PREFIX: &str = "path+";

/// The scheme of the URL in a `path+` source.
const FILE_SCHEME: &str = "file";

/// The arguments of `cargo locate-project`, before the manifest path. The
/// command gives the manifest of the workspace that holds the manifest.
const LOCATE_ARGUMENTS: [&str; 5] = [
    "locate-project",
    "--workspace",
    "--message-format",
    "plain",
    "--manifest-path",
];

/// What starts the source in the key of a record, after the name and the
/// version of the package.
const SOURCE_START: &str = " (";

/// What ends the key of a record.
const SOURCE_END: char = ')';

/// What comes between the URL and the commit in a `git+` source.
const COMMIT_SEPARATOR: char = '#';

/// What starts the query of a URL.
const QUERY_SEPARATOR: char = '?';

/// The suffix that cargo removes from a repository URL.
const GIT_SUFFIX: &str = ".git";

/// The name that cargo gives a repository whose URL has no path.
const EMPTY_REPOSITORY_NAME: &str = "_empty";

/// What comes between the repository name and the hash in the name of a
/// checkout directory.
const NAME_SEPARATOR: char = '-';

/// The directories in the cargo home that hold the git checkouts.
const CHECKOUTS_DIRECTORIES: [&str; 2] = ["git", "checkouts"];

/// The arguments that give the commit of a checkout.
const HEAD_ARGUMENTS: [&str; 2] = ["rev-parse", "HEAD"];

/// The variable that stops git at a directory when it looks for a repository.
const CEILING_VARIABLE: &str = "GIT_CEILING_DIRECTORIES";

/// The full id of a commit, as the record of an install gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit(String);

impl Commit {
    /// Gives the commit of `text` when it is a full commit id: 40 or 64
    /// hexadecimal digits. The id is in lowercase.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let full = FULL_COMMIT_LENGTHS.contains(&text.len())
            && text.chars().all(|character| character.is_ascii_hexdigit());
        full.then(|| Self(text.to_ascii_lowercase()))
    }

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
    /// A step failed in a way that stops the search: `--repo` is not a
    /// workspace, or git does not start.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// Step 2 and step 3 found no workspace.
    #[error("{miss}, and the installed {binary} gives no workspace: {reason}. Run {binary} in a cargo workspace, or name one with --repo <path>")]
    NotFound {
        /// Why step 2 found no workspace.
        miss: WorkTreeMiss,
        /// The name of the installed binary.
        binary: String,
        /// Why step 3 found no workspace. The box keeps the error small.
        reason: Box<SourceError>,
    },
}

/// The reasons that step 2 finds no workspace.
#[derive(Debug, thiserror::Error)]
pub enum WorkTreeMiss {
    /// The current directory is in no git work tree.
    #[error("{dir} is not in a git work tree", dir = .dir.display())]
    NotAWorkTree {
        /// The current directory.
        dir: PathBuf,
    },
    /// The git work tree of the current directory has no `Cargo.toml`.
    #[error("the git work tree {dir} has no Cargo.toml", dir = .dir.display())]
    NoManifest {
        /// The top of the git work tree.
        dir: PathBuf,
    },
}

/// The reasons that step 3 finds no workspace.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// The location of the records is not known.
    #[error("neither CARGO_HOME nor HOME is set")]
    NoCargoHome,
    /// The records file cannot be read.
    #[error("there is no record of the install, because wdid cannot read {path} ({source})", path = .path.display())]
    NoRecords {
        /// The records file.
        path: PathBuf,
        /// The error of the system.
        source: std::io::Error,
    },
    /// The records file is not in the shape of cargo.
    #[error("{path} is not a records file that wdid can read ({reason})", path = .path.display())]
    UnreadableRecords {
        /// The records file.
        path: PathBuf,
        /// The text that says what is wrong.
        reason: String,
    },
    /// No record names the binary.
    #[error("{path} holds no record of the binary {binary}", path = .path.display())]
    NoRecord {
        /// The records file.
        path: PathBuf,
        /// The name of the binary.
        binary: String,
    },
    /// The record names a source that is not `git+` and not `path+file`.
    #[error("the record \"{key}\" names a source that is not git+ or path+file")]
    OtherSource {
        /// The key of the record.
        key: String,
    },
    /// The `git+` record names no full commit.
    #[error("the record \"{key}\" names no full commit")]
    NoCommit {
        /// The key of the record.
        key: String,
    },
    /// No directory under the checkouts holds the commit.
    #[error("{checkouts} holds no checkout of commit {short} of {name}. cargo cache --autoclean removes the checkouts", checkouts = .checkouts.display(), short = .commit.short())]
    NoCheckout {
        /// The directory of the checkouts.
        checkouts: PathBuf,
        /// The name of the repository.
        name: String,
        /// The commit of the record.
        commit: Commit,
    },
    /// The checkout is at a different commit.
    #[error("the checkout {checkout} is at commit {found}, not at commit {expected} of the record", checkout = .checkout.display())]
    OtherCommit {
        /// The checkout.
        checkout: PathBuf,
        /// The commit of the record.
        expected: Commit,
        /// The commit that git gives for the checkout.
        found: String,
    },
    /// git cannot give the commit of the checkout.
    #[error("git cannot read the checkout {checkout} ({stderr})", checkout = .checkout.display())]
    NotACheckout {
        /// The checkout.
        checkout: PathBuf,
        /// The standard error of git, trimmed.
        stderr: String,
    },
    /// The crate directory of a `path+file` record does not exist.
    #[error("the crate directory {crate_dir} of the record does not exist", crate_dir = .crate_dir.display())]
    NoCrateDirectory {
        /// The crate directory of the record.
        crate_dir: PathBuf,
    },
    /// cargo finds no workspace that holds the crate directory.
    #[error("cargo locate-project finds no workspace for {crate_dir} ({stderr})", crate_dir = .crate_dir.display())]
    NoPathWorkspace {
        /// The crate directory of the record.
        crate_dir: PathBuf,
        /// The standard error of cargo, trimmed.
        stderr: String,
    },
    /// The checkout has no `Cargo.toml` at its top.
    #[error("the checkout {checkout} has no Cargo.toml", checkout = .checkout.display())]
    CheckoutWithoutManifest {
        /// The checkout.
        checkout: PathBuf,
    },
    /// A program that step 3 starts does not start.
    #[error(transparent)]
    Program(#[from] CatalogError),
}

/// Finds the workspace that `wdid` reports on.
///
/// `repo` is the value of `--repo`. `cwd` is the current directory.
/// `location` says where cargo keeps the records of the installs and its
/// checkouts. `binary` is the name of the installed binary whose source is
/// the workspace of step 3.
///
/// # Errors
/// Gives [`WorkspaceError::Catalog`] when `repo` is given and it is not a
/// cargo workspace, and when git does not start for step 2. Gives
/// [`WorkspaceError::NotFound`] when step 2 and step 3 find no workspace.
pub fn find(
    repo: Option<&Path>,
    cwd: &Path,
    programs: &Programs,
    location: &InstallLocation<'_>,
    binary: &str,
) -> Result<Workspace, WorkspaceError> {
    if let Some(repo) = repo {
        let root = catalog::workspace_root(Some(repo), cwd, programs)?;
        return Ok(Workspace {
            root,
            found_by: FoundBy::RepoFlag,
        });
    }
    let miss = match catalog::workspace_root(None, cwd, programs) {
        Ok(root) => {
            return Ok(Workspace {
                root,
                found_by: FoundBy::WorkTree,
            })
        }
        Err(CatalogError::NotAWorkTree { dir }) => WorkTreeMiss::NotAWorkTree { dir },
        Err(CatalogError::NotAWorkspace { dir }) => WorkTreeMiss::NoManifest { dir },
        Err(error) => return Err(error.into()),
    };
    installed_source(programs, location, binary).map_err(|reason| WorkspaceError::NotFound {
        miss,
        binary: binary.to_string(),
        reason: Box::new(reason),
    })
}

/// Step 3: gives the workspace that the installed `binary` came from.
fn installed_source(
    programs: &Programs,
    location: &InstallLocation<'_>,
    binary: &str,
) -> Result<Workspace, SourceError> {
    let install_root = catalog::install_root(location).ok_or(SourceError::NoCargoHome)?;
    let records = install_root.join(RECORDS_FILE);
    let key = record_key(&records, binary)?;
    let source = source_of(&key).ok_or_else(|| SourceError::OtherSource { key: key.clone() })?;
    if let Some(file_url) = source.strip_prefix(PATH_SOURCE_PREFIX) {
        let crate_dir =
            file_path(file_url).ok_or_else(|| SourceError::OtherSource { key: key.clone() })?;
        return Ok(Workspace {
            root: path_workspace(programs, &crate_dir)?,
            found_by: FoundBy::InstalledPath { crate_dir },
        });
    }
    let Some(git) = source.strip_prefix(GIT_SOURCE_PREFIX) else {
        return Err(SourceError::OtherSource { key });
    };
    let (url, commit) = git
        .rsplit_once(COMMIT_SEPARATOR)
        .and_then(|(url, commit)| Some((url, Commit::parse(commit)?)))
        .ok_or_else(|| SourceError::NoCommit { key: key.clone() })?;
    let cargo_home = catalog::cargo_home(location).ok_or(SourceError::NoCargoHome)?;
    let checkout = checkout(programs, &cargo_home, url, &commit)?;
    if !checkout.join(catalog::MANIFEST).is_file() {
        return Err(SourceError::CheckoutWithoutManifest { checkout });
    }
    Ok(Workspace {
        root: checkout,
        found_by: FoundBy::InstalledGit {
            url: url.to_string(),
            commit,
        },
    })
}

/// The part of `.crates2.json` that `wdid` reads.
#[derive(serde::Deserialize)]
struct Records {
    installs: BTreeMap<String, Record>,
}

/// One record of `.crates2.json`.
#[derive(serde::Deserialize)]
struct Record {
    bins: Vec<String>,
}

/// Gives the key of the record whose binaries hold `binary`.
fn record_key(path: &Path, binary: &str) -> Result<String, SourceError> {
    let text = std::fs::read_to_string(path).map_err(|source| SourceError::NoRecords {
        path: path.to_path_buf(),
        source,
    })?;
    let records: Records =
        serde_json::from_str(&text).map_err(|error| SourceError::UnreadableRecords {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    records
        .installs
        .into_iter()
        .find(|(_, record)| record.bins.iter().any(|name| name == binary))
        .map(|(key, _)| key)
        .ok_or_else(|| SourceError::NoRecord {
            path: path.to_path_buf(),
            binary: binary.to_string(),
        })
}

/// Gives the source in the key of a record. A key is
/// `<package> <version> (<source>)`.
fn source_of(key: &str) -> Option<&str> {
    key.split_once(SOURCE_START)?.1.strip_suffix(SOURCE_END)
}

/// Gives the path of a `file:` URL, with each percent escape read back.
fn file_path(file_url: &str) -> Option<PathBuf> {
    url::Url::parse(file_url)
        .ok()
        .filter(|url| url.scheme() == FILE_SCHEME)?
        .to_file_path()
        .ok()
}

/// Gives the root of the cargo workspace that holds `crate_dir`, as
/// `cargo locate-project --workspace` gives it.
fn path_workspace(programs: &Programs, crate_dir: &Path) -> Result<PathBuf, SourceError> {
    if !crate_dir.is_dir() {
        return Err(SourceError::NoCrateDirectory {
            crate_dir: crate_dir.to_path_buf(),
        });
    }
    let mut command = childenv::command(&programs.cargo);
    command
        .args(LOCATE_ARGUMENTS)
        .arg(crate_dir.join(catalog::MANIFEST))
        .stdin(Stdio::null());
    let output = catalog::run(&mut command, &programs.cargo)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let manifest = Path::new(stdout.trim_end_matches(['\n', '\r']));
    match manifest.parent() {
        Some(root) if output.status.success() && !root.as_os_str().is_empty() => {
            Ok(root.to_path_buf())
        }
        _ => Err(SourceError::NoPathWorkspace {
            crate_dir: crate_dir.to_path_buf(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        }),
    }
}

/// Gives the name that cargo gives the repository of `url`: the last part of
/// the path, without a query and without the suffix `.git`.
fn repository_name(url: &str) -> &str {
    let without_query = url.split(QUERY_SEPARATOR).next().unwrap_or(url);
    let path = without_query.trim_end_matches('/');
    let last = path.rsplit('/').next().unwrap_or(path);
    let name = last.strip_suffix(GIT_SUFFIX).unwrap_or(last);
    if name.is_empty() {
        EMPTY_REPOSITORY_NAME
    } else {
        name
    }
}

/// Gives the checkout of `commit` of the repository at `url`.
///
/// The checkout is a directory under `<cargo_home>/git/checkouts`. Its parent
/// has the name of the repository, a `-`, and a hash that is internal to
/// cargo. Its own name is the start of the commit, seven characters or more.
/// git must give the full commit for the checkout.
fn checkout(
    programs: &Programs,
    cargo_home: &Path,
    url: &str,
    commit: &Commit,
) -> Result<PathBuf, SourceError> {
    let checkouts: PathBuf = CHECKOUTS_DIRECTORIES
        .iter()
        .fold(cargo_home.to_path_buf(), |path, part| path.join(part));
    let name = repository_name(url);
    let prefix = format!("{name}{NAME_SEPARATOR}").to_lowercase();
    let mut first_miss = None;
    for repository in subdirectories(&checkouts) {
        let Some(directory) = repository.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !directory.to_lowercase().starts_with(&prefix) {
            continue;
        }
        for candidate in subdirectories(&repository) {
            let holds_commit = candidate
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.len() >= SHORT_COMMIT_LENGTH && commit.as_str().starts_with(name)
                });
            if !holds_commit {
                continue;
            }
            match head_matches(programs, &candidate, commit) {
                Ok(()) => return Ok(candidate),
                Err(error @ SourceError::Program(_)) => return Err(error),
                Err(miss) => {
                    first_miss.get_or_insert(miss);
                }
            }
        }
    }
    Err(first_miss.unwrap_or_else(|| SourceError::NoCheckout {
        checkouts,
        name: name.to_string(),
        commit: commit.clone(),
    }))
}

/// Gives the subdirectories of `directory`, sorted. A directory that cannot
/// be read has none.
fn subdirectories(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    found.sort();
    found
}

/// Says whether git gives `commit` for the checkout. The error says why the
/// checkout does not match, or that git does not start.
///
/// git does not look above the checkout for a repository, because a cargo
/// home inside another work tree would then give the commit of that work
/// tree.
fn head_matches(programs: &Programs, checkout: &Path, commit: &Commit) -> Result<(), SourceError> {
    let mut command = childenv::command(&programs.git);
    command
        .args(HEAD_ARGUMENTS)
        .current_dir(checkout)
        .stdin(Stdio::null());
    if let Some(parent) = checkout.parent() {
        command.env(CEILING_VARIABLE, parent);
    }
    let output = catalog::run(&mut command, &programs.git)?;
    if !output.status.success() {
        return Err(SourceError::NotACheckout {
            checkout: checkout.to_path_buf(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let found = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if found.eq_ignore_ascii_case(commit.as_str()) {
        Ok(())
    } else {
        Err(SourceError::OtherCommit {
            checkout: checkout.to_path_buf(),
            expected: commit.clone(),
            found,
        })
    }
}
