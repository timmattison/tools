//! `sidecar worktree`: point the link of a sidecar at a worktree of its main clone.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

use crate::host::Host;
use crate::link::point_link;
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
    let name = SidecarName::parse(raw_name)?;
    let main_clone = host.main_clone(&name);
    let branch = match branch {
        Some(branch) => branch.to_string(),
        None => current_branch(host)?,
    };
    let warnings = Vec::new();
    let target = host.worktrees_dir(&name).join(&branch);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    add_worktree(
        host,
        &main_clone,
        &["-b", &branch, &target.to_string_lossy(), "refs/heads/main"],
    )?;
    point_link(&host.link(&name), &target)
        .with_context(|| format!("cannot change {}", host.link_shown(&name)))?;
    Ok(Pointed {
        name,
        branch,
        target,
        made_worktree: true,
        warnings,
    })
}

/// The current branch of the host worktree.
fn current_branch(host: &Host) -> anyhow::Result<String> {
    let head = host.git().run(
        host.worktree_root(),
        ["symbolic-ref", "--quiet", "--short", "HEAD"],
    );
    if head.ok {
        Ok(head.stdout)
    } else {
        bail!(
            "cannot read the current branch of {}",
            host.worktree_root().display()
        )
    }
}

/// Runs `git worktree add --quiet` in the main clone with `args`.
fn add_worktree(host: &Host, main_clone: &Path, args: &[&str]) -> anyhow::Result<()> {
    let mut full = vec!["worktree", "add", "--quiet"];
    full.extend_from_slice(args);
    let added = host.git().run(main_clone, full);
    if added.ok {
        Ok(())
    } else {
        bail!("git cannot make the worktree: {}", added.stderr)
    }
}
