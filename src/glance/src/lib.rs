//! `glance` — a light macOS menu bar monitor of the CPU share and the memory
//! pressure.
//!
//! One status item in the menu bar shows two small charts. The left chart
//! shows the share of the CPU time that the system and the user processes
//! use. The right chart shows the memory in use, in the color of the pressure
//! level of the kernel.
//!
//! The monitors that this tool replaces use much memory and much CPU time. The
//! kernel gives each value in one cheap call, and two charts of 30 columns need
//! very little drawing. So `glance` reads the kernel directly, starts no child
//! process, and draws its charts into a pixel buffer itself.
//!
//! # The modules
//!
//! The rules are pure functions over plain values, so a test can read each
//! pixel of a chart without AppKit.
//!
//! The two modules `reader` and `shell` are the only modules that are
//! specific to macOS, and the only modules that can hold unsafe code. The
//! crate denies `unsafe_code`, and only their two declarations expect it, so
//! a review of those two files covers all of the unsafe code.

#![deny(unsafe_code)]

pub mod chart;
pub mod cpu;
pub mod history;
pub mod pressure;
pub mod sample;
