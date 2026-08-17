//! Host crate for repository-level guard tests.
//!
//! This crate exists solely to carry automated tests that prove the repo's
//! own guardrails actually fire — for example, that the `.husky/pre-commit`
//! hook rejects misformatted Rust. It is not a tool: it ships no binary and
//! deliberately omits the `--version`/git-hash handling the repo otherwise
//! mandates for tools, because there is nothing for a user to run.
//!
//! Most guards need no library code at all — running the real artifact and
//! asserting on its exit status is enough, and that logic lives in `tests/`.
//! The modules here exist for the guards that must *inspect* the repository
//! rather than run it, where the inspection deserves a narrow, reusable
//! interface and a remediation message written once instead of per call site.

// Each module carries its own `//!` header, whose first paragraph is its
// summary in the module list. Adding an outer `///` summary here as well would
// move module-doc link resolution to the crate root, silently breaking every
// intra-doc link inside those headers.
pub mod target_lints;
pub mod workspace_lints;
