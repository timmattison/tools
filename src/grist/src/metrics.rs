//! Newtyped conflict metrics.
//!
//! All three cost measures are counts, so a bare `usize` triple invites
//! transposition bugs that no compiler would catch. Each gets its own opaque
//! type with a private field.

/// A git branch name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BranchName(String);

impl BranchName {
    /// Wrap a branch name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The underlying branch name, for handing to git.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BranchName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Count of commits whose replay stopped the rebase for manual resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Stops(usize);

/// Count of distinct files that conflicted at least once across an ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Files(usize);

/// Count of individual conflict hunks a human would have to hand-merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Hunks(usize);

macro_rules! counter {
    ($name:ident) => {
        impl $name {
            /// Wrap a raw count.
            #[must_use]
            pub fn new(count: usize) -> Self {
                Self(count)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

counter!(Stops);
counter!(Files);
counter!(Hunks);

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
