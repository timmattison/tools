//! Host crate for repository-level guard tests.
//!
//! This crate exists solely to carry automated tests that prove the repo's
//! own guardrails actually fire — for example, that the `.husky/pre-commit`
//! hook rejects misformatted Rust. It is not a tool: it ships no binary and
//! deliberately omits the `--version`/git-hash handling the repo otherwise
//! mandates for tools, because there is nothing for a user to run.
//!
//! All meaningful behavior lives in `tests/`; this library is empty by design.
