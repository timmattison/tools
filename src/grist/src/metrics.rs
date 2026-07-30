//! What one candidate ordering costs, in newtyped conflict metrics.
//!
//! The counts themselves - [`Stops`], [`Files`], [`Hunks`] - are opaque
//! newtypes owned by [`gitscratch`], because every tool measuring a replay
//! reports the same three numbers. What is specific to `grist` is attributing
//! them to a *branch ordering*, which is [`OrderingScore`].

use gitscratch::{BranchName, Files, Hunks, Stops};

/// What one candidate ordering would cost to actually carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderingScore {
    order: Vec<BranchName>,
    stops: Stops,
    files: Files,
    hunks: Hunks,
}

impl OrderingScore {
    /// Record the cost of landing `order` in that sequence.
    #[must_use]
    pub fn new(order: Vec<BranchName>, stops: Stops, files: Files, hunks: Hunks) -> Self {
        Self {
            order,
            stops,
            files,
            hunks,
        }
    }

    /// The branch sequence this score describes.
    #[must_use]
    pub fn order(&self) -> &[BranchName] {
        &self.order
    }

    /// How many times a rebase would halt for manual resolution.
    #[must_use]
    pub fn stops(&self) -> Stops {
        self.stops
    }

    /// How many distinct files would conflict.
    #[must_use]
    pub fn files(&self) -> Files {
        self.files
    }

    /// How many conflict hunks would need hand-merging.
    #[must_use]
    pub fn hunks(&self) -> Hunks {
        self.hunks
    }

    /// *The* ranking key: what this ordering costs, ordered for comparison.
    ///
    /// Hunks lead because they count the lines a human actually hand-merges;
    /// stops and files break ties in favour of fewer interruptions and a
    /// smaller blast radius. Comparing the tuple compares all three
    /// lexicographically.
    ///
    /// Anything deciding whether two orderings cost the same - ranking them,
    /// declaring a tie, deduplicating them - must compare this and nothing
    /// narrower. Two scores can share a hunk count and still be ranked apart,
    /// so a check written against `hunks()` alone will contradict the ranking.
    #[must_use]
    pub fn cost_key(&self) -> (Hunks, Stops, Files) {
        (self.hunks, self.stops, self.files)
    }
}
