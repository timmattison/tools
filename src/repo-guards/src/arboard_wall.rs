//! Guard: only `sysclip` may depend on `arboard`.
//!
//! SKELETON. The real rule follows in the next commit.

use std::fmt;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// The package that no member but the owner may depend on.
pub const GUARDED_PACKAGE: &str = "arboard";

/// The one member that may depend on [`GUARDED_PACKAGE`].
pub const OWNER: &str = "sysclip";

/// Everything that can stop the audit from reaching a verdict.
#[derive(Debug, Error)]
pub enum ArboardWallError {
    /// No workspace member is named [`OWNER`].
    #[error("no workspace member is named `{OWNER}`")]
    OwnerMissing,
}

/// One dependency on [`GUARDED_PACKAGE`] that a member must not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offender {
    /// Name of the member package.
    pub member: String,
    /// Path of the manifest of the member.
    pub manifest_path: PathBuf,
    /// `normal`, `dev`, `build`, or the raw kind that cargo reported.
    pub kind: String,
    /// The local name, when the dependency is renamed.
    pub rename: Option<String>,
    /// The `cfg` or triple of a `[target.*.dependencies]` table.
    pub target: Option<String>,
}

/// The verdict of [`audit`].
#[derive(Debug, Default)]
pub struct Report {
    offenders: Vec<Offender>,
    members_examined: usize,
}

impl Report {
    /// The offenders, in the order of the members.
    #[must_use]
    pub fn offenders(&self) -> &[Offender] {
        &self.offenders
    }

    /// How many workspace members the audit read.
    #[must_use]
    pub fn members_examined(&self) -> usize {
        self.members_examined
    }

    /// True when no member but the owner depends on the guarded package.
    #[must_use]
    pub fn is_compliant(&self) -> bool {
        self.offenders.is_empty()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} offenders", self.offenders.len())
    }
}

/// Audit the workspace at `repo_root`.
///
/// # Errors
///
/// Skeleton: never fails.
pub fn audit(repo_root: &Path) -> Result<Report, ArboardWallError> {
    let _ = repo_root;
    Ok(Report::default())
}
