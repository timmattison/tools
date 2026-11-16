//! `glance` — a light macOS menu bar monitor of the CPU share and the memory
//! pressure.
//!
//! One status item in the menu bar shows two small charts. The left chart
//! shows the share of the CPU time that the system and the user processes
//! use. The right chart shows the memory in use, in the color of the pressure
//! level of the kernel. The short name of the Mac is in small text under the
//! charts.
//!
//! The monitors that this tool replaces use much memory and much CPU time. The
//! kernel gives each value in one cheap call, and two charts of 30 columns need
//! very little drawing. So `glance` reads the kernel directly and draws its
//! charts into a pixel buffer itself. In local mode, it starts no child process
//! and no thread.
//!
//! In remote mode (`--host`), the values come from a second Mac. `glance`
//! keeps one `ssh` child open, which runs `glance --stream` on the second Mac,
//! and two threads read the pipes of that child. Each sample then costs one
//! line of text, not one SSH handshake.
//!
//! # The modules
//!
//! The rules are pure functions over plain values, so a test can read each
//! pixel of a chart without AppKit.
//!
//! - [`cpu`] — the share of the CPU time between two readings of the tick
//!   counters. The counters wrap, so the difference wraps too.
//! - [`pressure`] — the memory pressure level, and the memory in use as a
//!   percentage.
//! - [`sample`] — one raw reading of the kernel, and the sample of two
//!   readings that one column of each chart shows.
//! - [`history`] — the ring buffer of the last samples. It allocates once.
//! - [`chart`] — the layout and the colors of the two charts, drawn into an
//!   RGBA pixel buffer.
//! - [`connection`] — the `ssh` connection to a second Mac, which runs
//!   `glance --stream` there and sends one event for each line.
//! - [`host`] — the short name of this host, as `hostname -s` gives it.
//! - [`interval`] — the time between two samples, from the command line.
//! - [`text`] — the lines of the menu, and the JSON line of `--sample` and
//!   `--stream`, with its parse.
//! - `reader` (macOS only) — the one module that calls the kernel.
//! - `shell` (macOS only) — the status item, its menu, and its timer, through
//!   AppKit.
//!
//! The two modules `reader` and `shell` are the only modules that are
//! specific to macOS, and the only modules that can hold unsafe code. The
//! crate denies `unsafe_code`, and only their two declarations expect it, so
//! a review of those two files covers all of the unsafe code.

#![deny(unsafe_code)]

pub mod chart;
pub mod connection;
pub mod cpu;
pub mod history;
pub mod host;
pub mod interval;
pub mod pressure;
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "the reader calls the kernel through libc. It is one of the two modules that can hold unsafe code"
)]
pub mod reader;
pub mod sample;
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "the shell calls AppKit through objc2. It is one of the two modules that can hold unsafe code"
)]
pub mod shell;
pub mod text;
