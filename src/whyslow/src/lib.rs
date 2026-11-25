//! `whyslow` — "why slow": one sample of this Mac, and a short summary of the
//! likely cause when it is slow. It uses no language model.
//!
//! On 2026-11-15 at 13:17, the load average was 152 on 14 processors, and the
//! CPU was 0% idle. The memory was full, and no row of `ps` or `top` showed the
//! cause. The cause was the sum of many small processes: 176 Claude Code
//! processes and 12 `clang` processes that build one C++ crate in two
//! worktrees. Thus `whyslow` puts each process into a group, adds the CPU and
//! the memory of each group, and names the groups that use the most of each
//! resource that is short.
//!
//! # The modules
//!
//! - [`groups`] — the table of groups, and the group of a process name.
//! - [`worktree`] — the worktree that a compiler builds for, from its
//!   arguments.
//! - [`sample`] — the process table of `ps`, and the CPU time that each
//!   process used between two samples.
//! - [`tally`] — the CPU and the memory of each group, and of each worktree of
//!   the compilers.
//! - [`verdict`] — the thresholds, and the summary of the likely cause.
//! - [`render`] — the lines of text that show the summary.
//! - [`machine`] — the facts of this Mac behind one trait, and the read of
//!   them in one fixed order.
//! - [`report`] — what the binary prints, and the exit code that it gives.

#[cfg(test)]
mod fixtures;
pub mod groups;
pub mod machine;
pub mod render;
pub mod report;
pub mod sample;
pub mod tally;
pub mod verdict;
pub mod worktree;
