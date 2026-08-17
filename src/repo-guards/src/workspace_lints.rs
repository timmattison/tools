//! Guard: every workspace member must inherit the repo-wide lint set.
//!
//! The root `Cargo.toml` declares `[workspace.lints.rust]` and
//! `[workspace.lints.clippy]`, but cargo hands those lints only to member
//! crates that opt in with:
//!
//! ```toml
//! [lints]
//! workspace = true
//! ```
//!
//! A member that omits the stanza is silently exempt from the repo's lint
//! policy. Nothing warns and nothing fails — the crate simply never gets linted
//! the way the rest of the workspace is. The exemption is invisible precisely
//! because it is spelled as an *absence*, which is also why it spreads: a new
//! crate that never types the stanza is born exempt.
//!
//! [`audit`] closes that hole. Two design rules make it a real guard rather
//! than a comfortable one:
//!
//! 1. **Enumerate, never allowlist.** Members come from `workspace.members` in
//!    the root manifest, glob patterns expanded on disk. A hardcoded list of
//!    crate names would make the next new crate invisible to the guard, which
//!    is the exact failure this exists to prevent.
//! 2. **Parse, never text-match.** "The manifest's `lints` table has key
//!    `workspace` set to `true`" names a syntactic category, so it is answered
//!    with a TOML parser. A regex would answer it only for the spellings
//!    someone happened to think of — `lints = { workspace = true }` is the
//!    same declaration written differently, and a missed spelling reports
//!    *clean*, which is indistinguishable from a guard doing real work.
//!
//! Everything that could shrink the audited set is a hard error rather than a
//! clean verdict; see [`WorkspaceLintsError`].
//!
//! # Relationship to cargo's own member expansion
//!
//! Member expansion here mirrors cargo: glob-expand each pattern and keep the
//! matches that are directories (cargo ignores stray files next to crate
//! directories, so `src/README.md` or a Finder-dropped `src/.DS_Store` is not a
//! member). It is deliberately *stricter* than cargo in one place: a pattern
//! that contributes no members at all is an error here, because a typo'd
//! pattern silently shrinking the audited set is the false-green disease this
//! guard exists to avoid.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;
use toml::Value;

/// Manifest filename, used for both the workspace root and every member.
pub(crate) const CARGO_TOML: &str = "Cargo.toml";

/// The exact stanza a member manifest needs. Rendered verbatim (indented) into
/// [`Report`]'s failure message so a reader can paste it without retyping.
const REQUIRED_STANZA: &str = "[lints]\nworkspace = true";

/// Manifest table that carries lint configuration.
const LINTS_KEY: &str = "lints";

/// Key inside `[lints]` that opts a member into the workspace lint set.
const WORKSPACE_KEY: &str = "workspace";

/// Everything that can stop the audit from reaching a verdict.
///
/// Every variant is a *refusal*. A guard that cannot enumerate the workspace
/// must say so loudly, because "I found no violations" and "I looked at
/// nothing" are the same sentence to a CI log and only one of them is good
/// news.
#[derive(Debug, Error)]
pub enum WorkspaceLintsError {
    /// A manifest could not be read from disk.
    #[error("cannot read the manifest {}: {source}", path.display())]
    ReadManifest {
        /// The manifest that could not be read.
        path: PathBuf,
        /// The underlying I/O failure.
        source: io::Error,
    },

    /// A manifest was read but is not valid TOML.
    #[error("cannot parse {} as TOML: {source}", path.display())]
    ParseManifest {
        /// The manifest that failed to parse.
        path: PathBuf,
        /// The underlying parse failure.
        source: toml::de::Error,
    },

    /// The root manifest declares no `workspace.members` array.
    #[error(
        "{} declares no `workspace.members`; refusing to report a clean workspace that cannot be enumerated",
        path.display()
    )]
    NoMembersKey {
        /// The root manifest.
        path: PathBuf,
    },

    /// `workspace.members` is present but empty.
    #[error(
        "`workspace.members` in {} is empty; refusing to report a clean workspace with no members",
        path.display()
    )]
    EmptyMembers {
        /// The root manifest.
        path: PathBuf,
    },

    /// An entry of `workspace.members` or `workspace.exclude` is not a string.
    #[error("`workspace.{key}` entry #{index} in {} is not a string", path.display())]
    NonStringEntry {
        /// The root manifest.
        path: PathBuf,
        /// Which array the bad entry came from: `members` or `exclude`.
        key: &'static str,
        /// Zero-based position of the bad entry.
        index: usize,
    },

    /// A pattern is not valid glob syntax.
    #[error("member pattern `{pattern}` is not a valid glob: {source}")]
    InvalidPattern {
        /// The offending pattern, as written in the root manifest.
        pattern: String,
        /// The underlying glob syntax failure.
        source: glob::PatternError,
    },

    /// A path matched by a pattern could not be read while walking.
    #[error("cannot read a path matched by pattern `{pattern}`: {source}")]
    UnreadableMatch {
        /// The pattern being expanded.
        pattern: String,
        /// The underlying filesystem failure.
        source: glob::GlobError,
    },

    /// A member pattern contributed no crate directories at all.
    #[error(
        "member pattern `{pattern}` matched no crate directories; a typo here would silently shrink the audited set"
    )]
    PatternMatchedNothing {
        /// The pattern that matched nothing.
        pattern: String,
    },

    /// Every expanded member was removed by `workspace.exclude`.
    #[error(
        "every workspace member is excluded by `workspace.exclude`; there is nothing to audit"
    )]
    NoMembersRemain,

    /// A member directory exists but holds no `Cargo.toml`.
    #[error("workspace member {} has no {CARGO_TOML}", dir.display())]
    MissingMemberManifest {
        /// The member directory.
        dir: PathBuf,
    },

    /// The repository root path is not valid UTF-8, so it cannot be used as a
    /// glob prefix.
    #[error("the repository root {} is not valid UTF-8", path.display())]
    NonUtf8RepoRoot {
        /// The repository root that was handed to [`audit`].
        path: PathBuf,
    },
}

/// The verdict of one audit: how many members were examined, and which of their
/// manifests fail to inherit the workspace lint set.
///
/// The remediation text lives here rather than at the call site, so every
/// caller — test, CI job, or CLI — reports the same thing.
#[derive(Debug, Clone)]
pub struct Report {
    /// Number of member crates whose manifests were actually parsed.
    members_examined: usize,
    /// Manifest paths, relative to the repo root, that do not inherit the
    /// workspace lint set. Sorted, so the message is stable run to run.
    offenders: Vec<PathBuf>,
}

impl Report {
    /// True when every examined member inherits the workspace lint set.
    #[must_use]
    pub fn is_compliant(&self) -> bool {
        self.offenders.is_empty()
    }

    /// Manifest paths that do not inherit the workspace lint set, relative to
    /// the repo root and sorted.
    #[must_use]
    pub fn offenders(&self) -> &[PathBuf] {
        &self.offenders
    }

    /// How many member manifests the audit parsed.
    ///
    /// A caller should assert this is non-zero: a guard that scans nothing
    /// reports clean for the wrong reason.
    #[must_use]
    pub fn members_examined(&self) -> usize {
        self.members_examined
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.offenders.is_empty() {
            return write!(
                f,
                "Checked {} workspace members; all inherit the workspace lint set.",
                self.members_examined
            );
        }

        writeln!(
            f,
            "{} of {} workspace members do not inherit the workspace lint set.",
            self.offenders.len(),
            self.members_examined
        )?;
        writeln!(
            f,
            "The root {CARGO_TOML} declares [workspace.lints.*], but cargo applies those lints only to members that opt in."
        )?;

        for offender in &self.offenders {
            writeln!(f)?;
            writeln!(
                f,
                "{} is missing workspace lint inheritance.",
                offender.display()
            )?;
            writeln!(f, "Add:")?;
            writeln!(f)?;
            for line in REQUIRED_STANZA.lines() {
                writeln!(f, "    {line}")?;
            }
        }

        Ok(())
    }
}

/// Audit every member of the workspace rooted at `repo_root`.
///
/// Members are enumerated from `workspace.members` in the root manifest and
/// glob-expanded on disk, minus anything `workspace.exclude` removes. Each
/// member manifest is parsed and checked for `lints.workspace = true`.
///
/// # Errors
///
/// Returns [`WorkspaceLintsError`] — never a clean [`Report`] — when the
/// workspace cannot be enumerated with confidence:
///
/// - the root manifest is absent, unreadable, or not valid TOML
///   ([`ReadManifest`](WorkspaceLintsError::ReadManifest),
///   [`ParseManifest`](WorkspaceLintsError::ParseManifest));
/// - `workspace.members` is missing ([`NoMembersKey`](WorkspaceLintsError::NoMembersKey)),
///   empty ([`EmptyMembers`](WorkspaceLintsError::EmptyMembers)), or holds a
///   non-string entry ([`NonStringEntry`](WorkspaceLintsError::NonStringEntry));
/// - a pattern is invalid glob syntax
///   ([`InvalidPattern`](WorkspaceLintsError::InvalidPattern)) or a matched path
///   cannot be read ([`UnreadableMatch`](WorkspaceLintsError::UnreadableMatch));
/// - a pattern contributes no members
///   ([`PatternMatchedNothing`](WorkspaceLintsError::PatternMatchedNothing)) or
///   `workspace.exclude` removes them all
///   ([`NoMembersRemain`](WorkspaceLintsError::NoMembersRemain));
/// - a member directory has no `Cargo.toml`
///   ([`MissingMemberManifest`](WorkspaceLintsError::MissingMemberManifest));
/// - `repo_root` is not valid UTF-8
///   ([`NonUtf8RepoRoot`](WorkspaceLintsError::NonUtf8RepoRoot)).
pub fn audit(repo_root: &Path) -> Result<Report, WorkspaceLintsError> {
    let member_dirs = members(repo_root)?;

    let mut offenders = Vec::new();
    for dir in &member_dirs {
        let manifest_path = dir.join(CARGO_TOML);
        if !inherits_workspace_lints(&parse_manifest(&manifest_path)?) {
            offenders.push(
                manifest_path
                    .strip_prefix(repo_root)
                    .unwrap_or(&manifest_path)
                    .to_path_buf(),
            );
        }
    }
    offenders.sort();

    Ok(Report {
        members_examined: member_dirs.len(),
        offenders,
    })
}

/// Enumerate the member crate directories of the workspace rooted at
/// `repo_root`, sorted and deduplicated.
///
/// This is the workspace-enumeration half of [`audit`], lifted out so every
/// repo guard that must walk "each member crate" walks the *same* set. A second
/// guard with its own copy of this logic would drift from this one, and a guard
/// that audits a smaller set than it believes reports clean for the wrong
/// reason — the exact failure this module exists to prevent, reintroduced by
/// duplication.
///
/// Every returned directory is guaranteed to hold a `Cargo.toml`, so a caller
/// may join and parse it without re-checking.
///
/// # Errors
///
/// Returns [`WorkspaceLintsError`] — never a short list — when the workspace
/// cannot be enumerated with confidence: an unreadable or unparsable root
/// manifest, a missing/empty/ill-typed `workspace.members`, an invalid or
/// empty-matching pattern, an exclude list that removes every member, or a
/// member directory with no manifest. See [`audit`] for the variant-by-variant
/// breakdown.
pub fn members(repo_root: &Path) -> Result<Vec<PathBuf>, WorkspaceLintsError> {
    let root_manifest = repo_root.join(CARGO_TOML);
    let root = parse_manifest(&root_manifest)?;
    let workspace = root.get("workspace");

    let patterns = workspace
        .and_then(|table| table.get("members"))
        .and_then(Value::as_array)
        .ok_or_else(|| WorkspaceLintsError::NoMembersKey {
            path: root_manifest.clone(),
        })?;
    if patterns.is_empty() {
        return Err(WorkspaceLintsError::EmptyMembers {
            path: root_manifest,
        });
    }

    let excluded = excluded_paths(repo_root, workspace, &root_manifest)?;

    let mut member_dirs = Vec::new();
    for (index, entry) in patterns.iter().enumerate() {
        let pattern = string_entry(entry, &root_manifest, "members", index)?;
        let matched = expand_pattern(repo_root, pattern)?;
        if matched.is_empty() {
            return Err(WorkspaceLintsError::PatternMatchedNothing {
                pattern: pattern.to_owned(),
            });
        }
        member_dirs.extend(
            matched
                .into_iter()
                .filter(|dir| !excluded.iter().any(|skip| dir.starts_with(skip))),
        );
    }
    member_dirs.sort();
    member_dirs.dedup();
    if member_dirs.is_empty() {
        return Err(WorkspaceLintsError::NoMembersRemain);
    }

    for dir in &member_dirs {
        if !dir.join(CARGO_TOML).is_file() {
            return Err(WorkspaceLintsError::MissingMemberManifest { dir: dir.clone() });
        }
    }

    Ok(member_dirs)
}

/// True when `manifest` opts into the workspace lint set.
///
/// Parsed, not text-matched, so every TOML spelling of the same declaration is
/// recognized: the `[lints]` table form, the inline `lints = { workspace = true }`
/// form, and the dotted `lints.workspace = true` form all reduce to the same
/// value. Anything else — no `lints` table, `workspace = false`, or a
/// `[lints.clippy]` table with no `workspace` key — is not inheritance.
fn inherits_workspace_lints(manifest: &Value) -> bool {
    manifest
        .get(LINTS_KEY)
        .and_then(|lints| lints.get(WORKSPACE_KEY))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Read and parse a manifest, attributing both failure modes to its path.
///
/// Shared with the sibling guards so "unreadable" and "unparsable" are refusals
/// everywhere, spelled the same way, rather than each guard inventing its own.
pub(crate) fn parse_manifest(path: &Path) -> Result<Value, WorkspaceLintsError> {
    let text = fs::read_to_string(path).map_err(|source| WorkspaceLintsError::ReadManifest {
        path: path.to_path_buf(),
        source,
    })?;
    text.parse::<Value>()
        .map_err(|source| WorkspaceLintsError::ParseManifest {
            path: path.to_path_buf(),
            source,
        })
}

/// Borrow an array entry as a string, or refuse.
fn string_entry<'a>(
    entry: &'a Value,
    root_manifest: &Path,
    key: &'static str,
    index: usize,
) -> Result<&'a str, WorkspaceLintsError> {
    entry
        .as_str()
        .ok_or_else(|| WorkspaceLintsError::NonStringEntry {
            path: root_manifest.to_path_buf(),
            key,
            index,
        })
}

/// Expand `workspace.exclude` into absolute directory paths.
///
/// An exclude pattern that matches nothing is *not* an error, unlike a member
/// pattern: excluding a path that is not there removes nothing from the audited
/// set, so it cannot hide a crate. A member pattern that matches nothing can.
fn excluded_paths(
    repo_root: &Path,
    workspace: Option<&Value>,
    root_manifest: &Path,
) -> Result<Vec<PathBuf>, WorkspaceLintsError> {
    let Some(entries) = workspace
        .and_then(|table| table.get("exclude"))
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };

    let mut excluded = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let pattern = string_entry(entry, root_manifest, "exclude", index)?;
        excluded.extend(expand_pattern(repo_root, pattern)?);
    }
    Ok(excluded)
}

/// Glob-expand one manifest pattern, relative to `repo_root`, keeping only the
/// directories — which is what cargo counts as a member candidate.
///
/// `repo_root` is glob-escaped before it is joined, so a repository checked out
/// under a path containing `*`, `?`, or `[` is matched literally instead of
/// being reinterpreted as pattern syntax.
fn expand_pattern(repo_root: &Path, pattern: &str) -> Result<Vec<PathBuf>, WorkspaceLintsError> {
    let root = repo_root
        .to_str()
        .ok_or_else(|| WorkspaceLintsError::NonUtf8RepoRoot {
            path: repo_root.to_path_buf(),
        })?;
    let anchored = format!(
        "{}/{pattern}",
        glob::Pattern::escape(root.trim_end_matches('/'))
    );

    let matches = glob::glob(&anchored).map_err(|source| WorkspaceLintsError::InvalidPattern {
        pattern: pattern.to_owned(),
        source,
    })?;

    let mut dirs = Vec::new();
    for entry in matches {
        let path = entry.map_err(|source| WorkspaceLintsError::UnreadableMatch {
            pattern: pattern.to_owned(),
            source,
        })?;
        if path.is_dir() {
            dirs.push(path);
        }
    }
    Ok(dirs)
}
