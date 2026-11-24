//! Simulates squash-merge orderings and ranks them by how much conflict
//! resolution each would cost.
//!
//! Squash-merging destroys the commit identity of the branch being merged, so
//! git cannot recognise those changes when a sibling branch is later rebased on
//! top. That makes the *order* in which branches land materially expensive.
//! `grist` measures that cost by replaying each candidate ordering against
//! throwaway git state and counting what a human would have to resolve.
//!
//! The replay machinery — the scratch worktree and the safety configuration
//! that makes running git against a real repository non-destructive — lives in
//! [`gitscratch`], which `grist` shares with the other dry-run tools. What is
//! `grist`'s own is enumerating orderings, squashing between them, and ranking
//! the results.

pub mod metrics;
pub mod plan;
pub mod rank;
pub mod simulate;

pub use gitscratch::{BranchName, Files, Hunks, Stops};
pub use metrics::OrderingScore;
pub use plan::ordering_count;
pub use rank::rank;
pub use simulate::{orderings_to_simulate, Simulator, MAX_BRANCHES};
