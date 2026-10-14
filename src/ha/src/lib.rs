//! `ha` (hot action) shows the GitHub Actions runs of many repositories
//! and owners in one self-refreshing terminal view.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    reason = "types such as run::RunState repeat the module name on purpose"
)]
pub mod target;
