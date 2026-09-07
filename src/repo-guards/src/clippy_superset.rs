//! Guard: a crate-local `clippy.toml` must restate every ban the workspace
//! declares.
//!
//! Clippy reads the **nearest** configuration file and no other. Starting at
//! the crate directory it walks up until it finds a `clippy.toml` or a
//! `.clippy.toml`, reads that one, and stops. It never merges the file it found
//! with the ones further up. So a crate that drops its own file beside its
//! manifest does not *extend* the workspace configuration — it *replaces* it,
//! and every ban the root file declared stops applying to that crate.
//!
//! The replacement is spelled as an absence, which is the false-green shape
//! this repository refuses everywhere else. Nothing warns. `cargo clippy` keeps
//! exiting 0, and it would keep exiting 0 on the day somebody wrote the banned
//! call the root file exists to stop.
//!
//! `ufa` is how this arrived. It needed bans of its own — clap's parse entry
//! points read the process environment, so a test that calls one races the
//! tests that write it — and adding `src/ufa/clippy.toml` silently exempted the
//! crate from the workspace's ban on the `colored` crate's process-global
//! override and on the two window-size calls that report a zero. There was no
//! live effect, because `ufa` depends on none of those crates. There would have
//! been one the first time it did.
//!
//! [`audit`] closes that hole. Three design rules make it a real guard rather
//! than a comfortable one:
//!
//! 1. **Enumerate, never allowlist.** Configuration files come from a walk of
//!    the repository, not from a list of the crates that have one today. A
//!    hardcoded list would make the next crate-local file invisible to the
//!    guard, which is the exact failure this exists to prevent.
//! 2. **Parse, never text-match.** "The crate file declares this ban" names a
//!    syntactic category, so a TOML parser answers it. A text search would pass
//!    on a ban named inside a comment — and every one of these files carries
//!    paragraphs of comments that name the bans they are about.
//! 3. **Refuse rather than shrink.** An unreadable file, an unparsable file, a
//!    missing root configuration, a directory that cannot be walked, and an
//!    entry shaped in a way the guard does not model are all errors rather than
//!    clean verdicts. See [`ClippySupersetError`].
//!
//! # What "superset" means key by key
//!
//! Clippy's configuration is a flat table whose values are either arrays of
//! bans or plain settings, so the requirement is read off the root file rather
//! than written down here:
//!
//! - An **array** value contributes one requirement per element. A table
//!   element is identified by its `path` string, which is how every
//!   `disallowed-*` list names the item it bans; a bare string element is
//!   identified by itself, which is how `disallowed-names` and its relatives
//!   are written. The crate file satisfies the requirement when its own array
//!   under the same key carries an element with that identity. Its array may be
//!   longer: extra bans are the whole point of having a crate-local file.
//! - Any **other** value is a setting rather than a list, and a setting cannot
//!   be extended — only replaced. The crate file satisfies it by declaring the
//!   same key with an equal value. A crate that genuinely wants a different
//!   threshold has to say so out loud by failing this guard, which is the
//!   visible, reviewable form of a decision that is otherwise invisible.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;
use toml::Value;

/// The two names clippy accepts for a configuration file.
///
/// Both are checked, because a crate that spells its file `.clippy.toml`
/// shadows the root exactly as one that spells it `clippy.toml` does. A guard
/// that knew only the first name would report clean for the second, which is
/// indistinguishable from a guard doing real work.
pub const CONFIG_NAMES: [&str; 2] = ["clippy.toml", ".clippy.toml"];

/// Directories the walk never enters.
///
/// Each holds files this repository does not author: build output under
/// `target` (including vendored crates that carry configuration of their own),
/// packages under `node_modules`, and git's own store. Clippy reads none of
/// them as a crate configuration either, because it only ever walks *up* from a
/// crate directory.
const SKIPPED_DIRS: [&str; 3] = ["target", "node_modules", ".git"];

/// The key inside a ban entry that names the banned item.
const PATH_KEY: &str = "path";

/// One thing the root configuration declares that a crate-local file must also
/// declare.
///
/// `Eq` is out of reach because `toml::Value` holds floats, so equality here is
/// the partial one. Nothing compares two requirements for ordering, and a float
/// in a clippy configuration would be a threshold rather than a ban.
#[derive(Debug, Clone, PartialEq)]
pub enum Requirement {
    /// One element of an array-valued key, identified by the `path` of a table
    /// element or by the text of a string element.
    Entry {
        /// The array-valued key the element belongs to, such as
        /// `disallowed-methods`.
        key: String,
        /// The element's identity.
        identity: String,
    },
    /// A key whose value is not an array, which a crate file can only match
    /// rather than extend.
    Setting {
        /// The key, such as `msrv`.
        key: String,
        /// The value the root file gives it.
        value: Value,
    },
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Entry { key, identity } => write!(f, "{key} entry `{identity}`"),
            Self::Setting { key, value } => write!(f, "{key} = {value}"),
        }
    }
}

/// Everything that can stop the audit from reaching a verdict.
///
/// Every variant is a *refusal*. A guard that cannot read a configuration file
/// must say so loudly, because "every crate file restates the workspace bans"
/// and "I could not read one" are the same sentence to a CI log and only one of
/// them is good news.
#[derive(Debug, Error)]
pub enum ClippySupersetError {
    /// A configuration file could not be read from disk.
    #[error("cannot read the clippy configuration {}: {source}", path.display())]
    ReadConfig {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying I/O failure.
        source: io::Error,
    },

    /// A configuration file was read but is not valid TOML.
    #[error("cannot parse {} as TOML: {source}", path.display())]
    ParseConfig {
        /// The file that failed to parse.
        path: PathBuf,
        /// The underlying parse failure.
        source: toml::de::Error,
    },

    /// A configuration file parsed but is not a table at its top level.
    #[error("{} is not a TOML table", path.display())]
    NotATable {
        /// The offending file.
        path: PathBuf,
    },

    /// The repository root holds no clippy configuration at all, so there is
    /// nothing for a crate file to be a superset of.
    #[error(
        "{} holds neither {} nor {}; refusing to report clean when there is no workspace configuration to compare against",
        root.display(),
        CONFIG_NAMES[0],
        CONFIG_NAMES[1]
    )]
    NoRootConfig {
        /// The repository root that was handed to [`audit`].
        root: PathBuf,
    },

    /// One directory holds both spellings of the configuration name. Clippy
    /// itself refuses such a directory, and the guard cannot know which file
    /// would have won.
    #[error(
        "{} holds both {} and {}; clippy refuses a directory that carries both",
        dir.display(),
        CONFIG_NAMES[0],
        CONFIG_NAMES[1]
    )]
    BothSpellings {
        /// The directory holding both files.
        dir: PathBuf,
    },

    /// A directory could not be walked.
    #[error("cannot read the directory {}: {source}", path.display())]
    ReadDirectory {
        /// The directory that could not be read.
        path: PathBuf,
        /// The underlying I/O failure.
        source: io::Error,
    },

    /// An array element is neither a string nor a table carrying a string
    /// `path`, so the guard cannot say what it declares.
    #[error(
        "entry #{index} of `{key}` in {} is neither a string nor a table with a string `{PATH_KEY}`; refusing to guess what it bans",
        path.display()
    )]
    UnsupportedEntry {
        /// The configuration file holding the entry.
        path: PathBuf,
        /// The array-valued key it belongs to.
        key: String,
        /// Zero-based position of the entry.
        index: usize,
    },
}

/// One crate-local configuration file and the root requirements it drops.
#[derive(Debug, Clone)]
pub struct Offender {
    /// The crate-local file, relative to the repository root.
    config: PathBuf,
    /// The root requirements this file does not restate.
    missing: Vec<Requirement>,
}

impl Offender {
    /// The crate-local configuration file, relative to the repository root.
    #[must_use]
    pub fn config(&self) -> &Path {
        &self.config
    }

    /// The root requirements this file does not restate, in the order the root
    /// file declares them.
    #[must_use]
    pub fn missing(&self) -> &[Requirement] {
        &self.missing
    }
}

/// The verdict of one audit: how many crate-local files were examined, and
/// which of them lose part of the workspace configuration.
///
/// The remediation text lives here rather than at the call site, so every
/// caller — test, CI job, or CLI — reports the same thing.
#[derive(Debug, Clone)]
pub struct Report {
    /// Number of crate-local configuration files actually parsed.
    files_examined: usize,
    /// The files that drop something, sorted by path.
    offenders: Vec<Offender>,
}

impl Report {
    /// True when every examined file restates the whole root configuration.
    #[must_use]
    pub fn is_compliant(&self) -> bool {
        self.offenders.is_empty()
    }

    /// The files that drop part of the root configuration.
    #[must_use]
    pub fn offenders(&self) -> &[Offender] {
        &self.offenders
    }

    /// How many crate-local configuration files the audit parsed.
    ///
    /// A caller should assert this matches what it expects to exist: a guard
    /// that scans nothing reports clean for the wrong reason.
    #[must_use]
    pub fn files_examined(&self) -> usize {
        self.files_examined
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.offenders.is_empty() {
            return write!(
                f,
                "Checked {} crate-local clippy configurations; all restate the workspace configuration.",
                self.files_examined
            );
        }

        writeln!(
            f,
            "{} of {} crate-local clippy configurations drop part of the workspace configuration.",
            self.offenders.len(),
            self.files_examined
        )?;
        writeln!(
            f,
            "Clippy reads the nearest configuration file and no other, so a crate file replaces the root file rather than extending it."
        )?;

        for offender in &self.offenders {
            writeln!(f)?;
            writeln!(
                f,
                "{} is missing {} of the root's declarations:",
                offender.config.display(),
                offender.missing.len()
            )?;
            for requirement in &offender.missing {
                writeln!(f, "    {requirement}")?;
            }
        }

        Ok(())
    }
}

/// Audit every crate-local clippy configuration in the repository at
/// `repo_root`.
///
/// The root configuration is read first and turned into a list of
/// requirements; every other configuration file in the repository must satisfy
/// all of them. See the module header for what a requirement is, key by key.
///
/// # Errors
///
/// Returns [`ClippySupersetError`] — never a clean [`Report`] — when the
/// comparison cannot be made with confidence: no root configuration
/// ([`NoRootConfig`](ClippySupersetError::NoRootConfig)), a directory holding
/// both spellings of the name
/// ([`BothSpellings`](ClippySupersetError::BothSpellings)), a file that cannot
/// be read ([`ReadConfig`](ClippySupersetError::ReadConfig)), parsed
/// ([`ParseConfig`](ClippySupersetError::ParseConfig)) or that is not a table
/// ([`NotATable`](ClippySupersetError::NotATable)), a directory that cannot be
/// walked ([`ReadDirectory`](ClippySupersetError::ReadDirectory)), or an array
/// entry the guard does not model
/// ([`UnsupportedEntry`](ClippySupersetError::UnsupportedEntry)).
pub fn audit(repo_root: &Path) -> Result<Report, ClippySupersetError> {
    let root_config = root_config(repo_root)?;
    let required = requirements(&root_config)?;

    let mut offenders = Vec::new();
    let crate_configs = configs(repo_root)?;
    for config in &crate_configs {
        let table = parse_config(config)?;
        let missing: Vec<Requirement> = required
            .iter()
            .filter(|requirement| !satisfies(&table, requirement))
            .cloned()
            .collect();

        if !missing.is_empty() {
            offenders.push(Offender {
                config: config
                    .strip_prefix(repo_root)
                    .unwrap_or(config)
                    .to_path_buf(),
                missing,
            });
        }
    }
    offenders.sort_by(|left, right| left.config.cmp(&right.config));

    Ok(Report {
        files_examined: crate_configs.len(),
        offenders,
    })
}

/// Every clippy configuration file in the repository except the root's own,
/// sorted.
///
/// This is the enumeration half of [`audit`], public so a companion test can
/// cross-check it against an independent derivation. A guard that walks a
/// smaller tree than it believes reports clean for the wrong reason, and the
/// only way to notice is to derive the set twice.
///
/// # Errors
///
/// Returns [`ClippySupersetError`] when a directory cannot be read
/// ([`ReadDirectory`](ClippySupersetError::ReadDirectory)) or one directory
/// holds both spellings of the name
/// ([`BothSpellings`](ClippySupersetError::BothSpellings)).
pub fn configs(repo_root: &Path) -> Result<Vec<PathBuf>, ClippySupersetError> {
    let mut found = Vec::new();
    walk(repo_root, repo_root, &mut found)?;
    found.sort();
    Ok(found)
}

/// Collect the configuration files under `directory`, skipping the root's own.
fn walk(
    directory: &Path,
    repo_root: &Path,
    found: &mut Vec<PathBuf>,
) -> Result<(), ClippySupersetError> {
    if let Some(config) = config_in(directory)? {
        if directory != repo_root {
            found.push(config);
        }
    }

    let entries = fs::read_dir(directory).map_err(|source| ClippySupersetError::ReadDirectory {
        path: directory.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| ClippySupersetError::ReadDirectory {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();

        // `is_dir` follows symlinks, and a link that points at an ancestor
        // would walk forever. Reading the entry's own type keeps the walk to
        // the real tree.
        let kind = entry
            .file_type()
            .map_err(|source| ClippySupersetError::ReadDirectory {
                path: path.clone(),
                source,
            })?;
        if !kind.is_dir() {
            continue;
        }
        if path
            .file_name()
            .is_some_and(|name| SKIPPED_DIRS.iter().any(|skipped| name == *skipped))
        {
            continue;
        }

        walk(&path, repo_root, found)?;
    }

    Ok(())
}

/// The clippy configuration file in `directory`, if it has one.
///
/// Holding both spellings is a refusal rather than a choice, because clippy
/// itself refuses such a directory and the guard must not decide for it.
fn config_in(directory: &Path) -> Result<Option<PathBuf>, ClippySupersetError> {
    let present: Vec<PathBuf> = CONFIG_NAMES
        .iter()
        .map(|name| directory.join(name))
        .filter(|path| path.is_file())
        .collect();

    match present.len() {
        0 => Ok(None),
        1 => Ok(present.into_iter().next()),
        _ => Err(ClippySupersetError::BothSpellings {
            dir: directory.to_path_buf(),
        }),
    }
}

/// The repository's own clippy configuration.
fn root_config(repo_root: &Path) -> Result<PathBuf, ClippySupersetError> {
    config_in(repo_root)?.ok_or_else(|| ClippySupersetError::NoRootConfig {
        root: repo_root.to_path_buf(),
    })
}

/// Read and parse a configuration file as a TOML table.
fn parse_config(path: &Path) -> Result<toml::Table, ClippySupersetError> {
    let text = fs::read_to_string(path).map_err(|source| ClippySupersetError::ReadConfig {
        path: path.to_path_buf(),
        source,
    })?;
    let value = text
        .parse::<Value>()
        .map_err(|source| ClippySupersetError::ParseConfig {
            path: path.to_path_buf(),
            source,
        })?;

    match value {
        Value::Table(table) => Ok(table),
        _ => Err(ClippySupersetError::NotATable {
            path: path.to_path_buf(),
        }),
    }
}

/// Turn the root configuration into the list every crate file must satisfy.
fn requirements(root_config: &Path) -> Result<Vec<Requirement>, ClippySupersetError> {
    let table = parse_config(root_config)?;

    let mut required = Vec::new();
    for (key, value) in &table {
        let Some(entries) = value.as_array() else {
            required.push(Requirement::Setting {
                key: key.clone(),
                value: value.clone(),
            });
            continue;
        };

        for (index, entry) in entries.iter().enumerate() {
            let identity =
                identity_of(entry).ok_or_else(|| ClippySupersetError::UnsupportedEntry {
                    path: root_config.to_path_buf(),
                    key: key.clone(),
                    index,
                })?;
            required.push(Requirement::Entry {
                key: key.clone(),
                identity,
            });
        }
    }

    Ok(required)
}

/// What one array element declares: the `path` of a table element, or the text
/// of a string element.
fn identity_of(entry: &Value) -> Option<String> {
    match entry {
        Value::String(text) => Some(text.clone()),
        Value::Table(table) => table.get(PATH_KEY)?.as_str().map(ToOwned::to_owned),
        _ => None,
    }
}

/// True when `table` carries the declaration `requirement` demands.
fn satisfies(table: &toml::Table, requirement: &Requirement) -> bool {
    match requirement {
        Requirement::Entry { key, identity } => table
            .get(key)
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(identity_of)
                    .collect::<BTreeSet<String>>()
            })
            .is_some_and(|declared| declared.contains(identity)),
        Requirement::Setting { key, value } => table.get(key) == Some(value),
    }
}
