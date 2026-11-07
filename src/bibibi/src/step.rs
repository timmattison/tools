//! One step of the search: the types of a commit and of its verdict.

use std::fmt;

/// A full object id that git resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit(pub String);

impl Commit {
    /// The first seven characters of the id.
    pub fn short(&self) -> String {
        self.0.chars().take(7).collect()
    }
}

impl fmt::Display for Commit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What one step decided about one commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The test passed.
    Good,
    /// The test failed.
    Bad,
}
