//! Simulates squash-merge orderings and ranks them by how much conflict
//! resolution each would cost.
//!
//! Squash-merging destroys the commit identity of the branch being merged, so
//! git cannot recognise those changes when a sibling branch is later rebased on
//! top. That makes the *order* in which branches land materially expensive.
//! `grist` measures that cost by replaying each candidate ordering against
//! throwaway git state and counting what a human would have to resolve.

pub mod git;
pub mod metrics;
pub mod plan;
pub mod rank;
pub mod simulate;

pub use metrics::{BranchName, Files, Hunks, OrderingScore, Stops};
pub use plan::ordering_count;
pub use rank::rank;
pub use simulate::{orderings_to_simulate, Simulator, MAX_BRANCHES};
