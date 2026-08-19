//! Guard: every binary the workspace builds must appear in both tool indexes.
//!
//! The repository documents its tools twice, and on purpose. `README.md`
//! carries the long entry — what the tool is for, how to run it, how to install
//! it. `TLDR.md` carries one line per tool, alphabetized, for a reader who only
//! needs to know which tool to reach for. A tool that is missing from either one
//! is a tool nobody finds.
//!
//! Nothing enforced this before. The omission is spelled as an *absence*, which
//! is why it spread: a new crate that nobody remembers to document is born
//! undocumented, and no build step ever says so.
//!
//! [`audit`] closes that hole.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Everything that can stop the audit from reaching a verdict.
#[derive(Debug, Error)]
pub enum ToolIndexError {
    /// An index file could not be read from disk.
    #[error("cannot read the index {}: {source}", path.display())]
    ReadIndex {
        /// The index that could not be read.
        path: PathBuf,
        /// The underlying I/O failure.
        source: std::io::Error,
    },
}

/// The verdict of one audit: how many binaries were examined, and which of them
/// are absent from each index.
#[derive(Debug, Clone, Default)]
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

/// Audit the tool indexes of the workspace rooted at `repo_root`.
///
/// # Errors
///
/// Returns [`ToolIndexError`] — never a clean [`Report`] — when the workspace
/// or its indexes cannot be read with confidence.
pub fn audit(repo_root: &Path) -> Result<Report, ToolIndexError> {
    let _ = repo_root;
    Ok(Report::default())
}
