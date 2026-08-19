//! Guard: every binary the workspace builds must appear in both tool indexes.
//!
//! The repository documents its tools twice, and on purpose. `README.md`
//! carries the long entry — what the tool is for, how to run it, how to install
//! it. `TLDR.md` carries one line per tool, alphabetized, for a reader who only
//! needs to know which tool to reach for. A tool missing from either one is a
//! tool nobody finds.
//!
//! Nothing enforced this before. The omission is spelled as an *absence*, which
//! is why it spread: a crate that nobody remembers to document is born
//! undocumented, and no build step ever says so. Two of the workspace's
//! binaries had drifted out of an index by the time this guard was written.
//!
//! [`audit`] closes that hole. Three design rules make it a real guard rather
//! than a comfortable one:
//!
//! 1. **Enumerate, never allowlist.** The tools come from the binary targets
//!    cargo builds, resolved from [`workspace_lints::members`] and each member's
//!    manifest. A hardcoded list of tool names would make the next new tool
//!    invisible to the guard, which is the exact failure this exists to prevent.
//! 2. **Parse, never text-match.** "`TLDR.md` has a table row whose first cell
//!    names this tool" and "`README.md` has an entry for this tool" both name
//!    syntactic categories of Markdown, so both are answered with a Markdown
//!    parser. Searching the file for the tool's name instead would pass on a
//!    *mention*, and the indexes are full of mentions: `sirn`'s row names
//!    `portplz`, and `prgz`'s row names `prcp`. Delete either of those tools'
//!    own rows and a text search still reports clean — which is
//!    indistinguishable from a guard doing real work.
//! 3. **Refuse rather than shrink.** Everything that could quietly reduce the
//!    audited set — an unreadable index, a `TLDR.md` with no table, a `README.md`
//!    with no tools section, a workspace with no binaries, a manifest that moves
//!    cargo's target discovery — is an error rather than a clean verdict. See
//!    [`ToolIndexError`].
//!
//! # What counts as an entry
//!
//! `TLDR.md` is one table, and a tool is listed when the **first cell of a body
//! row** names it. The head row and every later cell are ignored, so the prose
//! that describes one tool can name another without documenting it.
//!
//! `README.md` uses two forms, both of which are in service today, so the guard
//! accepts either:
//!
//! - a **top-level list item** under the `## The tools` heading, whose first
//!   word is the tool name (`- zth (zero the hero)`), and
//! - a **level-2 section heading** whose first word is the tool name
//!   (`## occ (old Claude Code)`).
//!
//! Nested list items are not entries. The `- To install: …` line under a tool's
//! own entry would otherwise document a tool named `To`.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use thiserror::Error;
use toml::Value;

use crate::workspace_lints::{self, parse_manifest, WorkspaceLintsError, CARGO_TOML};

/// The long index, at the repository root.
const README: &str = "README.md";

/// The one-line index, at the repository root.
const TLDR: &str = "TLDR.md";

/// The `README.md` heading that opens the list of tools. Matched on its prefix,
/// so the heading may carry trailing words without breaking the guard.
const TOOLS_SECTION: &str = "The tools";

/// The manifest table that holds package metadata.
const PACKAGE_KEY: &str = "package";

/// The manifest key naming a package.
const NAME_KEY: &str = "name";

/// The manifest array-of-tables declaring binaries explicitly.
const BIN_KEY: &str = "bin";

/// The `[package]` key that turns cargo's binary auto-discovery on or off.
///
/// This guard models the default discovery rules only. A manifest that changes
/// them is refused rather than guessed at.
const AUTOBINS_KEY: &str = "autobins";

/// The conventional root of a package's one binary.
const SRC_MAIN_RS: &str = "src/main.rs";

/// The directory whose Rust files are each their own binary.
const SRC_BIN: &str = "src/bin";

/// The conventional entry point of a directory-shaped binary
/// (`src/bin/foo/main.rs`).
const MAIN_RS: &str = "main.rs";

/// Rust source extension, used when auto-discovering binary roots.
const RS: &str = "rs";

/// Everything that can stop the audit from reaching a verdict.
///
/// Every variant is a *refusal*. A guard that cannot enumerate the tools must
/// say so loudly, because "every tool is documented" and "I found no tools" are
/// the same sentence to a CI log and only one of them is good news.
#[derive(Debug, Error)]
pub enum ToolIndexError {
    /// The workspace members could not be enumerated, or a member manifest
    /// could not be read or parsed.
    #[error("cannot enumerate the workspace: {0}")]
    Members(#[from] WorkspaceLintsError),

    /// An index file could not be read from disk.
    #[error("cannot read the index {}: {source}", path.display())]
    ReadIndex {
        /// The index that could not be read.
        path: PathBuf,
        /// The underlying I/O failure.
        source: io::Error,
    },

    /// The workspace builds no binaries at all.
    #[error(
        "the workspace builds no binaries; refusing to report every tool documented when there are no tools"
    )]
    NoBinaries,

    /// `TLDR.md` holds no table body row, so no tool can be listed in it.
    #[error(
        "{} holds no table row; refusing to report a clean index that lists nothing",
        path.display()
    )]
    NoTableRows {
        /// The index that holds no table.
        path: PathBuf,
    },

    /// `README.md` has no `## The tools` heading, so the list index is gone.
    #[error(
        "{} has no `## {TOOLS_SECTION}` heading; refusing to audit a README whose tool list cannot be found",
        path.display()
    )]
    NoToolsSection {
        /// The index that lost its tools section.
        path: PathBuf,
    },

    /// A member manifest declares no `package.name`, so its binary cannot be
    /// named.
    #[error("{} declares no `package.name`", path.display())]
    MissingPackageName {
        /// The member manifest.
        path: PathBuf,
    },

    /// A `[[bin]]` entry has no string `name`.
    #[error("`[[bin]]` entry #{index} in {} has no string `name`", path.display())]
    NonStringBinName {
        /// The member manifest.
        path: PathBuf,
        /// Zero-based position of the bad entry.
        index: usize,
    },

    /// A member manifest moves cargo's binary auto-discovery, which this guard
    /// does not model.
    #[error(
        "{} sets `package.{AUTOBINS_KEY}`; this guard models cargo's default binary discovery only, and refuses to guess",
        path.display()
    )]
    AutoDiscoveryOverride {
        /// The member manifest.
        path: PathBuf,
    },

    /// A `src/bin` directory could not be walked.
    #[error("cannot read {}: {source}", path.display())]
    UnreadableBinDir {
        /// The directory that could not be walked.
        path: PathBuf,
        /// The underlying I/O failure.
        source: io::Error,
    },

    /// A discovered binary root has a name that is not valid UTF-8, so it
    /// cannot be compared against an index entry.
    #[error("the binary root {} has a name that is not valid UTF-8", path.display())]
    NonUtf8BinName {
        /// The offending path.
        path: PathBuf,
    },
}

/// The verdict of one audit: how many binaries were examined, and which of them
/// are absent from each index.
///
/// The remediation text lives here rather than at the call site, so every
/// caller — test, CI job, or CLI — reports the same thing.
#[derive(Debug, Clone)]
pub struct Report {
    /// Number of binary targets the audit enumerated.
    binaries_examined: usize,
    /// Binary names with no entry in `README.md`. Sorted.
    missing_from_readme: Vec<String>,
    /// Binary names with no row in `TLDR.md`. Sorted.
    missing_from_tldr: Vec<String>,
}

impl Report {
    /// True when every examined binary appears in both indexes.
    #[must_use]
    pub fn is_compliant(&self) -> bool {
        self.missing_from_readme.is_empty() && self.missing_from_tldr.is_empty()
    }

    /// Binary names with no entry in `README.md`, sorted.
    #[must_use]
    pub fn missing_from_readme(&self) -> &[String] {
        &self.missing_from_readme
    }

    /// Binary names with no row in `TLDR.md`, sorted.
    #[must_use]
    pub fn missing_from_tldr(&self) -> &[String] {
        &self.missing_from_tldr
    }

    /// How many binary targets the audit enumerated.
    ///
    /// A caller should assert this is non-zero: a guard that scans nothing
    /// reports clean for the wrong reason.
    #[must_use]
    pub fn binaries_examined(&self) -> usize {
        self.binaries_examined
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_compliant() {
            return write!(
                f,
                "Checked {} binaries; all appear in {README} and {TLDR}.",
                self.binaries_examined
            );
        }

        writeln!(
            f,
            "Of {} binaries the workspace builds, some are absent from a tool index.",
            self.binaries_examined
        )?;

        for (index, missing) in [
            (README, &self.missing_from_readme),
            (TLDR, &self.missing_from_tldr),
        ] {
            if missing.is_empty() {
                continue;
            }
            writeln!(f)?;
            writeln!(f, "Absent from {index}: {}", missing.join(", "))?;
        }

        writeln!(f)?;
        writeln!(
            f,
            "Add an entry to {README} — a top-level item under `## {TOOLS_SECTION}`, or a `## <tool>` section —"
        )?;
        write!(
            f,
            "and a row to the table in {TLDR}, whose first cell is the tool name."
        )
    }
}

/// Audit the tool indexes of the workspace rooted at `repo_root`.
///
/// Binaries come from [`binaries`]. Each index is parsed as Markdown, and a
/// binary is documented when its name is an entry as the module header defines
/// one.
///
/// # Errors
///
/// Returns [`ToolIndexError`] — never a clean [`Report`] — when the workspace
/// or its indexes cannot be read with confidence: the members cannot be
/// enumerated ([`Members`](ToolIndexError::Members)), an index is unreadable
/// ([`ReadIndex`](ToolIndexError::ReadIndex)), `TLDR.md` holds no table
/// ([`NoTableRows`](ToolIndexError::NoTableRows)), `README.md` has no tools
/// section ([`NoToolsSection`](ToolIndexError::NoToolsSection)), or the
/// workspace builds no binaries ([`NoBinaries`](ToolIndexError::NoBinaries)).
pub fn audit(repo_root: &Path) -> Result<Report, ToolIndexError> {
    let names = binaries(repo_root)?;

    let readme_path = repo_root.join(README);
    let readme_names =
        readme_entries(&read_index(&readme_path)?).ok_or(ToolIndexError::NoToolsSection {
            path: readme_path.clone(),
        })?;

    let tldr_path = repo_root.join(TLDR);
    let tldr_names = tldr_entries(&read_index(&tldr_path)?);
    if tldr_names.is_empty() {
        return Err(ToolIndexError::NoTableRows { path: tldr_path });
    }

    Ok(Report {
        binaries_examined: names.len(),
        missing_from_readme: absent(&names, &readme_names),
        missing_from_tldr: absent(&names, &tldr_names),
    })
}

/// Every binary name the workspace rooted at `repo_root` builds, sorted and
/// deduplicated.
///
/// Public so a companion test can compare this set against `cargo metadata` —
/// the guard's model of cargo's discovery rules is worth exactly as much as its
/// agreement with cargo, and a binary the guard never enumerates is one it can
/// never report as undocumented.
///
/// # Errors
///
/// Returns [`ToolIndexError`] when the members cannot be enumerated, a member
/// manifest cannot be read or parsed, a manifest moves cargo's binary discovery
/// or omits a name the guard needs, or the workspace builds no binaries at all.
pub fn binaries(repo_root: &Path) -> Result<BTreeSet<String>, ToolIndexError> {
    let mut names = BTreeSet::new();
    for dir in workspace_lints::members(repo_root)? {
        let manifest_path = dir.join(CARGO_TOML);
        let manifest = parse_manifest(&manifest_path)?;
        names.extend(binary_names(&dir, &manifest, &manifest_path)?);
    }

    if names.is_empty() {
        return Err(ToolIndexError::NoBinaries);
    }
    Ok(names)
}

/// The names in `wanted` that `documented` does not hold, in sorted order.
fn absent(wanted: &BTreeSet<String>, documented: &BTreeSet<String>) -> Vec<String> {
    wanted.difference(documented).cloned().collect()
}

/// Read one index file, attributing the failure to its path.
fn read_index(path: &Path) -> Result<String, ToolIndexError> {
    fs::read_to_string(path).map_err(|source| ToolIndexError::ReadIndex {
        path: path.to_path_buf(),
        source,
    })
}

/// The binary names one member builds, following cargo's default discovery:
/// every explicit `[[bin]]`, plus `src/main.rs` named for the package, plus
/// each `src/bin/*.rs` and `src/bin/*/main.rs`.
///
/// A path an explicit `[[bin]]` already claims is not discovered a second time,
/// which is what keeps a package that renames its binary from being counted
/// under both names.
fn binary_names(
    dir: &Path,
    manifest: &Value,
    manifest_path: &Path,
) -> Result<Vec<String>, ToolIndexError> {
    let package = manifest.get(PACKAGE_KEY);
    if package.and_then(|table| table.get(AUTOBINS_KEY)).is_some() {
        return Err(ToolIndexError::AutoDiscoveryOverride {
            path: manifest_path.to_path_buf(),
        });
    }

    let mut names = Vec::new();
    let mut claimed = BTreeSet::new();
    if let Some(bins) = manifest.get(BIN_KEY).and_then(Value::as_array) {
        for (index, bin) in bins.iter().enumerate() {
            let name = bin.get(NAME_KEY).and_then(Value::as_str).ok_or(
                ToolIndexError::NonStringBinName {
                    path: manifest_path.to_path_buf(),
                    index,
                },
            )?;
            names.push(name.to_owned());
            if let Some(path) = bin.get("path").and_then(Value::as_str) {
                claimed.insert(path.replace('\\', "/"));
            }
        }
    }

    if dir.join(SRC_MAIN_RS).is_file() && !claimed.contains(SRC_MAIN_RS) {
        names.push(
            package
                .and_then(|table| table.get(NAME_KEY))
                .and_then(Value::as_str)
                .ok_or(ToolIndexError::MissingPackageName {
                    path: manifest_path.to_path_buf(),
                })?
                .to_owned(),
        );
    }

    names.extend(discovered_bin_dir(dir, &claimed)?);
    Ok(names)
}

/// Every binary `src/bin` contributes: one per `*.rs` file, and one per
/// subdirectory holding a `main.rs`.
fn discovered_bin_dir(
    dir: &Path,
    claimed: &BTreeSet<String>,
) -> Result<Vec<String>, ToolIndexError> {
    let bin_dir = dir.join(SRC_BIN);
    if !bin_dir.is_dir() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(&bin_dir).map_err(|source| ToolIndexError::UnreadableBinDir {
        path: bin_dir.clone(),
        source,
    })?;

    let mut names = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|source| ToolIndexError::UnreadableBinDir {
                path: bin_dir.clone(),
                source,
            })?
            .path();

        let (stem, relative) = if path.is_file() && path.extension().is_some_and(|ext| ext == RS) {
            (path.file_stem(), format!("{SRC_BIN}/{}", file_name(&path)?))
        } else if path.is_dir() && path.join(MAIN_RS).is_file() {
            (
                path.file_name(),
                format!("{SRC_BIN}/{}/{MAIN_RS}", file_name(&path)?),
            )
        } else {
            continue;
        };

        if claimed.contains(&relative) {
            continue;
        }
        names.push(
            stem.and_then(|s| s.to_str())
                .ok_or(ToolIndexError::NonUtf8BinName { path: path.clone() })?
                .to_owned(),
        );
    }
    Ok(names)
}

/// The final component of `path` as UTF-8, or a refusal.
fn file_name(path: &Path) -> Result<&str, ToolIndexError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .ok_or(ToolIndexError::NonUtf8BinName {
            path: path.to_path_buf(),
        })
}

/// The tool names in the first cell of every body row of `markdown`'s tables.
///
/// The head row is skipped, and so is every cell after the first, so a
/// description that names another tool does not document it.
fn tldr_entries(markdown: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut in_head = false;
    let mut cell = 0_usize;
    let mut capturing = false;
    let mut buffer = String::new();

    for event in Parser::new_ext(markdown, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::TableHead) => in_head = true,
            Event::End(TagEnd::TableHead) => in_head = false,
            Event::Start(Tag::TableRow) => cell = 0,
            Event::Start(Tag::TableCell) => {
                capturing = !in_head && cell == 0;
                buffer.clear();
            }
            Event::End(TagEnd::TableCell) => {
                if capturing {
                    insert_trimmed(&mut names, &buffer);
                    capturing = false;
                }
                cell += 1;
            }
            Event::Text(text) | Event::Code(text) if capturing => buffer.push_str(&text),
            _ => {}
        }
    }
    names
}

/// The tool names `markdown` documents: the first word of every top-level list
/// item under `## The tools`, and the first word of every other level-2
/// heading.
///
/// Returns `None` when the tools section is absent, which is a refusal rather
/// than an empty answer — a README that lost its list would otherwise report
/// every tool undocumented.
fn readme_entries(markdown: &str) -> Option<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    let mut found_section = false;
    let mut in_tools = false;
    let mut heading = None;
    let mut list_depth = 0_usize;
    let mut capturing_item = false;
    let mut buffer = String::new();

    for event in Parser::new_ext(markdown, Options::empty()) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some(level);
                buffer.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                if heading == Some(HeadingLevel::H2) {
                    in_tools = buffer.trim().starts_with(TOOLS_SECTION);
                    if in_tools {
                        found_section = true;
                    } else {
                        insert_first_word(&mut names, &buffer);
                    }
                }
                heading = None;
            }
            Event::Start(Tag::List(_)) => list_depth += 1,
            Event::End(TagEnd::List(_)) => list_depth = list_depth.saturating_sub(1),
            Event::Start(Tag::Item) if list_depth == 1 => {
                capturing_item = in_tools;
                buffer.clear();
            }
            Event::End(TagEnd::Item) if list_depth == 1 => {
                if capturing_item {
                    insert_first_word(&mut names, &buffer);
                }
                capturing_item = false;
            }
            Event::Text(text) | Event::Code(text)
                if heading.is_some() || (capturing_item && list_depth == 1) =>
            {
                buffer.push_str(&text);
            }
            _ => {}
        }
    }

    found_section.then_some(names)
}

/// Insert `text`'s first whitespace-separated word, when it has one.
fn insert_first_word(names: &mut BTreeSet<String>, text: &str) {
    if let Some(word) = text.split_whitespace().next() {
        names.insert(word.to_owned());
    }
}

/// Insert `text` without its surrounding whitespace, when anything remains.
fn insert_trimmed(names: &mut BTreeSet<String>, text: &str) {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        names.insert(trimmed.to_owned());
    }
}
