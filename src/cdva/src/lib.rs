//! `cdva` — "count da various attributes".
//!
//! The library behind the `cdva` binary: a code counter that reports the test
//! code of a tree apart from its production code.
//!
//! This slice carries the language table. [`Language`] names every language the
//! tool counts and reads the language of a file out of its path. Later slices
//! hang the line classifier, the path rule, and the tree rule off the same
//! table.

pub mod lang;

pub use lang::Language;
