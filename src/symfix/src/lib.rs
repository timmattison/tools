//! `symfix` — finds the broken symbolic links under a directory, and repairs
//! the ones the caller knows how to repair.
//!
//! A symbolic link holds its target as text. The text says nothing about
//! whether the target is there. A tree that moved, an archive that unpacked
//! somewhere else, or a checkout at a new prefix thus leaves links that name
//! files which are not there. This crate walks a tree, asks the operating
//! system to resolve every link it finds, and reports each link the operating
//! system cannot resolve.
//!
//! [`run`] is the whole interface. It reads the [`Options`] the command line
//! built, writes its report to the two writers the caller gives it, and gives
//! back a [`Summary`] of what it found. Nothing in this crate writes to the
//! standard streams of the process on its own, so a test gives `run` two
//! `Vec<u8>` values and reads back every line it wrote. The report goes to the
//! output writer and the diagnostics go to the error writer, so a caller can
//! send one through a pipe and still read the other.
//!
//! The walk never follows a link. A link is the thing this tool examines, thus
//! a walk that followed one would examine the tree behind it instead — and a
//! link that points at one of its own parents would make the walk endless.

#![cfg_attr(not(test), warn(clippy::unwrap_used))]
#![cfg_attr(not(test), warn(clippy::expect_used))]

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

/// Everything the caller decides before the walk starts.
#[derive(Debug, Clone)]
pub struct Options {
    /// The root of the walk, already absolute.
    pub root: PathBuf,
    /// The string to put in front of a broken target.
    pub prepend: Option<OsString>,
    /// The prefix to take off the front of a broken target.
    pub remove: Option<OsString>,
    /// Print the plan and touch nothing.
    pub dry_run: bool,
    /// Write the debug lines to the error stream.
    pub verbose: bool,
    /// The names of the directories the walk does not enter.
    pub skip: Vec<OsString>,
}

/// What one run of the tool did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// The number of links whose target is not there.
    pub broken: usize,
    /// The number of links the run repaired.
    pub fixed: usize,
    /// The number of links the run could not resolve, for a reason that is not
    /// absence.
    pub errors: usize,
}

/// Walks the tree, reports every broken link, repairs what the options let it
/// repair, and writes the closing summary.
///
/// The report goes to `out` and the diagnostics go to `err`. The walk carries
/// on past a directory it cannot read and past a link it cannot read, so one
/// unreadable corner of a tree never hides the rest of it.
pub fn run(_options: &Options, _out: &mut dyn Write, _err: &mut dyn Write) -> Summary {
    Summary::default()
}
