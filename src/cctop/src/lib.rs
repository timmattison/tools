//! Claude Code top. This crate reads the state files that each Claude Code
//! session writes, and keeps the state of each session on a board.
//!
//! The modules here do the work that has no terminal in it: they read the
//! state files, find the config directories, and read the session names.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod ancestry;
pub mod board;
pub mod liveness;
pub mod lsof;
pub mod notification;
pub mod paths;
pub mod registry;
pub mod runner;
pub mod state;
pub mod store;
pub mod sync;
pub mod wezterm;
pub mod zellij;
