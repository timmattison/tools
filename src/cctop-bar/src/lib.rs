//! The logic of `cctop-bar`, a macOS menu-bar program that runs the cctop
//! board with no terminal.
//!
//! Each rule lives in a pure module that has no `AppKit` code, thus it builds
//! and runs its tests on every platform:
//!
//! - `chord` reads the global hotkey as text and gives its Carbon codes.
//! - `painter` paints the mini-map of the status item as pixels.
//! - `menu` lists the entries of the menu as data.
//! - `keymap` turns an `AppKit` key event into a crossterm key.
//! - `placement` picks the screen, the cells and the frame of the panel.
//! - `panel` decides when the panel shows and hides.
//! - `settings` holds the settings, the arguments of the background copy and the lines of the launch.
//! - `shell` (macOS only) is the one module with `AppKit` code: the status item and the menu.
//! - `cellgrid` keeps the cells that the cctop screen draws and turns them into paint operations.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod cellgrid;
pub mod chord;
pub mod keymap;
pub mod menu;
pub mod painter;
pub mod panel;
pub mod placement;
pub mod settings;
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "the shell calls AppKit through objc2. It is the one module that can hold unsafe code"
)]
pub mod shell;
