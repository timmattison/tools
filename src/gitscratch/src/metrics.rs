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
    ($name:ident, $noun:literal) => {
        impl $name {
            /// Wrap a raw count.
            #[must_use]
            pub fn new(count: usize) -> Self {
                Self(count)
            }

            /// The count with its noun, pluralised - `"1 hunk"`, `"4 hunks"`.
            ///
            /// The noun belongs to the type rather than to whoever is printing
            /// it. A renderer that has to remember both the word and when to
            /// add the `s` is a renderer that can get one of them wrong, and
            /// the tools built on this crate must not be able to disagree about
            /// what to call the same number.
            #[must_use]
            pub fn phrase(&self) -> String {
                if self.0 == 1 {
                    format!("{} {}", self.0, $noun)
                } else {
                    format!("{} {}s", self.0, $noun)
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

counter!(Stops, "stop");
counter!(Files, "file");
counter!(Hunks, "hunk");

impl Stops {
    /// The raw count, for the one place in this crate that has to put a `Stops`
    /// back into the `usize` a `Conflicts` stores it as.
    ///
    /// Compiled only alongside its single caller,
    /// [`crate::scratch::Conflicts::from_files`], which is the whole
    /// justification for it existing. Unwrapping a counter is precisely what
    /// these types are here to stop, so the unwrap exists only in the builds
    /// that have the fixture constructor to feed - never in a released binary.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) const fn count(self) -> usize {
        self.0
    }
}
