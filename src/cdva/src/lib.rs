//! `cdva` — "count da various attributes".
//!
//! The library behind the `cdva` binary: a code counter that reports the test
//! code of a tree apart from its production code.
//!
//! [`Language`] names every language the tool counts, reads the language of a
//! file out of its path, and carries the comment and string syntax of that
//! language. [`classify`] reads a source under that syntax and labels every row
//! blank, comment, or code. [`PathRules`] marks a whole file as test material
//! from its path alone, which is the cheap half of the split. A later slice
//! hangs the tree rule off the same table.

pub mod lang;
pub mod lines;
pub mod pathrule;

pub use lang::{BlockSpec, CommentSyntax, Language, StringSpec};
pub use lines::{classify, count, Counts, LineIndex, LineKind};
pub use pathrule::{PathRules, PathVerdict};
