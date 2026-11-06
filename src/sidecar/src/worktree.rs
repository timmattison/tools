//! `sidecar worktree`: point the link of a sidecar at a worktree of its main clone.

use std::path::PathBuf;

use crate::host::Host;
use crate::name::SidecarName;

/// What `point` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pointed {
    /// The sidecar.
    pub name: SidecarName,
    /// The branch that the link now reads.
    pub branch: String,
    /// The path that the link now points at.
    pub target: PathBuf,
    /// True when this run made the worktree.
    pub made_worktree: bool,
    /// One line for each problem that did not stop the run.
    pub warnings: Vec<String>,
}

/// Points the link of the sidecar `raw_name` at the worktree of `branch`. Without a
/// branch, the current branch of the host worktree is the branch.
///
/// # Errors
///
/// Returns an error that names the fix when a check fails.
pub fn point(host: &Host, raw_name: &str, branch: Option<&str>) -> anyhow::Result<Pointed> {
    let _ = (host, raw_name, branch);
    anyhow::bail!("stub")
}
