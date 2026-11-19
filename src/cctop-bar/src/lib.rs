//! The logic of `cctop-bar`, a macOS menu-bar program that runs the cctop
//! board with no terminal.
//!
//! Each rule lives in a pure module that has no `AppKit` code, thus it builds
//! and runs its tests on every platform:
//!
//! - `chord` reads the global hotkey as text and gives its Carbon codes.
//! - `painter` paints the mini-map of the status item as pixels.
//! - `menu` lists the entries of the menu as data.
//! - `cellgrid` keeps the cells that the cctop screen draws and turns them into paint operations.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod cellgrid;
pub mod chord;
pub mod menu;
pub mod painter;
