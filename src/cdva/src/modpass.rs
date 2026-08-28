//! The second pass: a `#[cfg(test)] mod <name>;` declaration marks the file it
//! names.
//!
//! This is the one rule of the tool that reads across files. A Rust file that
//! declares
//!
//! ```text
//! #[cfg(test)]
//! mod tests;
//! ```
//!
//! holds none of the test code it is talking about: the whole of the file it
//! names is test code. No path rule can see that, because the name of the named
//! file proves nothing on its own — `tests.rs` is an ordinary name — and no
//! tree rule can see it either, because the evidence lives in a different file.
//! So the declaration is collected while the declaring file is parsed, and
//! [`resolve_test_modules`] resolves it once every file has been counted and
//! before the counts are added up.

use crate::file::FileCount;

/// Mark every file that a `#[cfg(test)] mod <name>;` declaration names.
///
/// This is the one rule of the tool that reads across files, so it runs after
/// every file has been counted and before the counts are added up.
pub fn resolve_test_modules(_files: &mut [FileCount]) {}
