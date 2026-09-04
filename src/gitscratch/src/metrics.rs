//! Newtyped conflict metrics.
//!
//! Every measure here is a count, so passing them as bare `usize`s invites
//! transposition bugs that no compiler would catch. Each gets its own opaque
//! type with a private field.
//!
//! Each also owns the *noun* it is counted in, and the `s` that noun takes in
//! the plural, so the two never have to be remembered by whoever is printing
//! the number. That is what makes a renderer unable to word the same count two
//! ways, and it is why a count crosses this crate's boundary as a counter
//! rather than as the integer inside it.
//!
//! # A counter has no bare rendering
//!
//! That last claim holds only while the integer inside a counter is out of
//! reach. `format!("{} across {}", hunks, files)` prints `"4 across 2"`, which
//! is the wording failure these types exist to stop, and `{}` is the spelling a
//! caller reaches for first. So a counter has no `Display`. A count leaves one
//! through a method that names which rendering the caller wants: `phrase` for a
//! sentence, which supplies the noun, and `digits` for a table cell, whose
//! column heading supplies it instead. Neither one is the free one, so a caller
//! chooses between them rather than falling into either.
//!
//! [`BranchName`] keeps its `Display`, and it is not a counter. That type *is*
//! its string, so `{}` on it prints the branch name and nothing else.
//!
//! A counter renders through a method that names the rendering:
//!
//! ```
//! let hunks = gitscratch::Hunks::new(4);
//! assert_eq!(hunks.phrase(), "4 hunks");
//! ```
//!
//! The block below is the same two lines with the rendering left unnamed, so
//! what it proves is that the unnamed rendering is what stops it:
//!
//! ```compile_fail
//! let hunks = gitscratch::Hunks::new(4);
//! assert_eq!(format!("{hunks}"), "4");
//! ```

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

/// Count of files a replay would not carry with it - staged, unstaged, or
/// untracked.
///
/// Its noun is the whole `"uncommitted file"`, not `"file"`, because the word
/// the count is *about* is what the reader needs and what the renderer must
/// not be left to supply. `Files` counts files that conflicted; this counts
/// files that were never committed. Two counts of files that mean opposite
/// things, so each says which it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Uncommitted(usize);

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

            /// The count on its own, as digits - `"1"`, `"4"`.
            ///
            /// For a table cell under a heading that already carries the noun,
            /// which is the one place a bare number reads correctly. It is a
            /// method rather than a `Display`, so a caller who wants it has to
            /// name it. A `Display` would make the bare number the free
            /// rendering and `phrase` the remembered one, and
            /// `format!("{} across {}", hunks, files)` would then compile and
            /// print `"4 across 2"` - which is the sentence this module exists
            /// to make unwritable.
            #[must_use]
            pub fn digits(&self) -> String {
                self.0.to_string()
            }
        }
    };
}

counter!(Stops, "stop");
counter!(Files, "file");
counter!(Hunks, "hunk");
counter!(Uncommitted, "uncommitted file");

impl Stops {
    /// Count one more halt.
    ///
    /// The two ways a stop count grows are here rather than at the call sites,
    /// because a caller that could reach the integer to add to it could reach
    /// it to print it. [`crate::scratch::Conflicts`] stores a `Stops` and never
    /// a `usize`, so no route in this crate has to unwrap one.
    pub(crate) fn increment(&mut self) {
        self.0 += 1;
    }

    /// Fold another replay's halts into this running total.
    pub(crate) fn add(&mut self, other: Self) {
        self.0 += other.0;
    }
}

#[cfg(test)]
mod tests {
    use super::Uncommitted;

    /// Both halves of the noun - the word and the `s` - belong to the counter,
    /// because a renderer that has to remember the word *and* when to add the
    /// `s` is a renderer that can get one of them wrong. The default is a
    /// clean tree, so a caller that has nothing to report says "0 uncommitted
    /// files" in the plural, the way English counts nothing.
    #[test]
    fn the_uncommitted_counter_owns_both_halves_of_its_noun() {
        assert_eq!(Uncommitted::new(1).phrase(), "1 uncommitted file");
        assert_eq!(Uncommitted::new(3).phrase(), "3 uncommitted files");
        assert_eq!(Uncommitted::default().phrase(), "0 uncommitted files");
    }
}
