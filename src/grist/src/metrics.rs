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

            /// The raw count, for display and arithmetic at the edges.
            #[must_use]
            pub fn get(self) -> usize {
                self.0
            }
        }

        impl std::ops::Add for $name {
            type Output = Self;

            fn add(self, other: Self) -> Self {
                Self(self.0 + other.0)
            }
        }

        impl std::iter::Sum for $name {
            fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
                iter.fold(Self(0), std::ops::Add::add)
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
}
