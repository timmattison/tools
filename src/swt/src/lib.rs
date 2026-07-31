//! Library half of `swt`, the subagent worktree helper for parallel TDD.
//!
//! The binary at `src/main.rs` is only a command line surface; everything it
//! decides lives here, where it can be unit tested directly and exercised from
//! `tests/` without going through a subprocess. Splitting the crate this way is
//! not cosmetic: a binary-only crate exports nothing, so its logic can only ever
//! be tested through its own stdout, which is a poor place to pin the meaning of
//! a predicate.

pub mod git;
