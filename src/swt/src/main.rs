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

use std::path::PathBuf;
use std::process::ExitCode;

use buildinfo::version_string;
use clap::{Parser, Subcommand};
use swt::create::create;
use swt::merge::merge;

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
///
/// Both arguments take hyphen-leading values. Neither command has options of its
/// own beyond `--help`, so an argument that looks like a flag is a name or a
/// path that starts with `-`, and the command that owns it has a better answer
/// than "unexpected argument": `create` quotes the naming rule the input broke,
/// and `merge` reports that no such worktree exists.
#[derive(Debug, Subcommand)]
enum Command {
    /// Verify HEAD is green, create a worktree on a new branch, and print its path.
    Create {
        /// Name for the new worktree and its branch.
        #[arg(allow_hyphen_values = true)]
        name: String,
    },
    /// Verify the subagent is green, merge it into the parent, and clean up.
    Merge {
        /// Path to the subagent worktree to merge back.
        #[arg(allow_hyphen_values = true)]
        worktree_path: PathBuf,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Create { name } => create(&name),
        Command::Merge { worktree_path } => merge(&worktree_path),
    }
}
