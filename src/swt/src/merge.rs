//! merge — `swt merge <worktree-path>`: the subagent's work comes back only
//! when both sides are clean and green.
//!
//! The implementation lands in the green half of this slice. This stub exists so
//! the command is wired end to end and the suite's failures are about behaviour
//! rather than about a missing symbol.

use std::path::Path;
use std::process::ExitCode;

/// Merges the subagent worktree at `worktree_path` back into the parent.
///
/// Refuses — and reports why — when either worktree is dirty or not green.
/// Returns the status `swt` should exit with.
pub fn merge(_worktree_path: &Path) -> ExitCode {
    ExitCode::FAILURE
}
