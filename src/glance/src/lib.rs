//! `glance` — a light macOS menu bar monitor of the CPU share and the memory
//! pressure.
//!
//! One status item in the menu bar shows two small charts, each as high as the
//! menu bar. The left chart shows the share of the CPU time that the
//! system and the user processes use. The right chart shows the memory in use,
//! in the color of the pressure level of the kernel. The name of the Mac is in
//! small text at the top of the charts. In local mode, it is the short name of
//! the Mac, as `hostname -s` gives it.
//!
//! The monitors that this tool replaces use much memory and much CPU time. The
//! kernel gives each value in one cheap call, and two charts of 32 columns
//! need very little drawing. So `glance` reads the kernel directly and draws its
//! charts into a pixel buffer itself. In local mode, the status item starts no
//! child process and no thread.
//!
//! In remote mode (`--host`), the values come from a second Mac. `glance`
//! keeps one `ssh` child open, which runs `glance --stream` on the second Mac,
//! and two threads read the pipes of that child. Each sample then costs one
//! line of text, not one SSH handshake. The name at the top of the charts is
//! the destination of `--host` as the user typed it. With `--resolve`, it is
//! the name that the second Mac reports.
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
//! - [`flags`] — the names of the flags that a background start gives to its
//!   copy.
//! - [`text`] — the lines of the menu, and the JSON line of `--sample` and
//!   `--stream`, with its parse.
//! - [`view`] — the samples and the last error that the status item shows, and
//!   the rules of each mode that change them.
//! - `reader` (macOS only) — the one module that calls the kernel.
//! - `shell` (macOS only) — the status item, its menu, and its timer, through
//!   AppKit.
//! - `bundle` (macOS only) — the app bundle that each status item runs from, so
//!   that macOS gives the status item a bundle identifier.
//! - `instance` (macOS only) — the one status item of each target: the lock
//!   that marks it, and the calls to the kernel that detach it from the
//!   terminal and that stop it.
//! - `launch` (macOS only) — what one run does with the status item of its
//!   target: start it here, start it in the background, or stop it.
//!
//! # One status item for each target
//!
//! A target is this Mac, or one destination of `--host`. A run of `glance`
//! first looks for the status item of its target. When one runs, the run
//! stops it and starts nothing. Thus a second run of the same command closes
//! the status item, and a run for a different target leaves it alone.
//! A run starts a new status item in a detached copy of `glance` and returns
//! to the shell. `--foreground` keeps the status item in the process of the
//! run, which holds the terminal.
//!
//! # Unsafe code
//!
//! The modules `reader`, `shell`, `instance`, and `launch` are specific to
//! macOS. Only `reader`, `shell`, and `instance` can hold unsafe code. The
//! crate denies `unsafe_code`, and only their three declarations expect it,
//! so a review of those three files covers all of the unsafe code.

#![deny(unsafe_code)]

#[cfg(target_os = "macos")]
pub mod bundle;
pub mod chart;
pub mod connection;
pub mod cpu;
pub mod flags;
pub mod history;
pub mod host;
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "the instance module calls the kernel through libc for its lock, its signals, and its session. It is one of the three modules that can hold unsafe code"
)]
pub mod instance;
pub mod interval;
#[cfg(target_os = "macos")]
pub mod launch;
pub mod pressure;
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "the reader calls the kernel through libc. It is one of the three modules that can hold unsafe code"
)]
pub mod reader;
pub mod sample;
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "the shell calls AppKit through objc2. It is one of the three modules that can hold unsafe code"
)]
pub mod shell;
pub mod text;
pub mod view;
