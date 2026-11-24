//! Claude Code top. This crate reads the state files that each Claude Code
//! session writes, keeps the state of each session on a board, and focuses
//! the terminal of a session.
//!
//! The library holds all the logic of cctop:
//!
//! - `state`, `paths`, `registry`, `store`, `liveness`, and `sync` read the
//!   state files and the session names, find the stale sessions, and keep
//!   the `board` current.
//! - `app` turns a key into an action, and `ui` draws the screen into a
//!   ratatui `Frame`.
//! - `focus` runs the two focus steps: `zellij` focuses the pane, and
//!   `wezterm` writes the focus request to the terminal device of the pane.
//!   `ancestry` and `lsof` find that device.
//! - `notification` sends a macOS notification.
//! - `runner` runs each external command, thus a test can answer a command
//!   without it running.
//! - `controller` is the logic of the main loop.
//!
//! The binary only wires these modules to the terminal: one channel of
//! events, one loop that draws, and one thread for each job that can block.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod ancestry;
pub mod app;
pub mod board;
pub mod controller;
pub mod focus;
pub mod liveness;
pub mod lsof;
pub mod notification;
pub mod paths;
pub mod registry;
pub mod runner;
pub mod state;
pub mod store;
pub mod sync;
pub mod ui;
pub mod wezterm;
pub mod zellij;
