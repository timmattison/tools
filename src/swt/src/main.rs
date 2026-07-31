//! swt — subagent worktree helper for parallel TDD.
//!
//! ```text
//! swt create <name>          → verify HEAD green, create worktree on a new branch, print path
//! swt merge <worktree-path>  → verify subagent green, ff-merge (rebase if parent advanced), cleanup
//! ```
//!
//! The guarantees the two commands enforce — worktrees are only ever created
//! from a green commit, a name can only ever name the thing it spells, a merge
//! neither loses work nor advances the parent past an in-progress red, what
//! lands is green *as merged*, and merges into one repository never interleave —
//! live with their implementations. This module is only the command line
//! surface: it decides which command was asked for, and rejects everything that
//! is not one of them with a usage error rather than a guess.

use std::path::{Path, PathBuf};

use buildinfo::version_string;
use clap::{Parser, Subcommand};

/// Command line surface of `swt`.
///
/// A bare `swt` prints the usage text on stderr and exits with the usual
/// usage status, so a caller that forgot the command sees both commands
/// instead of silence.
#[derive(Debug, Parser)]
#[command(
    name = "swt",
    version = version_string!(),
    about = "Subagent worktree helper for parallel TDD",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The commands `swt` accepts. Each takes exactly one argument, and neither is
/// optional: there is nothing sensible to create without a name, and nothing to
/// merge without a worktree.
#[derive(Debug, Subcommand)]
enum Command {
    /// Verify HEAD is green, create a worktree on a new branch, and print its path.
    Create {
        /// Name for the new worktree and its branch.
        name: String,
    },
    /// Verify the subagent is green, merge it into the parent, and clean up.
    Merge {
        /// Path to the subagent worktree to merge back.
        worktree_path: PathBuf,
    },
}

/// Creates a subagent worktree named `name`, branched from a green HEAD.
///
/// # Panics
///
/// Always: the implementation lands in a later slice.
fn create(name: &str) {
    todo!("swt create {name}: implemented in the `create` slice");
}

/// Merges the subagent worktree at `worktree_path` back into the parent.
///
/// # Panics
///
/// Always: the implementation lands in a later slice.
fn merge(worktree_path: &Path) {
    todo!(
        "swt merge {}: implemented in the `merge` slice",
        worktree_path.display()
    );
}

fn main() {
    match Cli::parse().command {
        Command::Create { name } => create(&name),
        Command::Merge { worktree_path } => merge(&worktree_path),
    }
}
