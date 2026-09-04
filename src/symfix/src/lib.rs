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
//!
//! A link that the operating system refuses to resolve is not a broken link.
//! [`scan::classify`] keeps the two apart, because the repairs act on
//! [`scan::LinkState::Broken`] alone: a tool that rewrote a link whose failure
//! it did not understand would destroy a working link to punish a directory
//! that denied it a read.

#![cfg_attr(not(test), warn(clippy::unwrap_used))]
#![cfg_attr(not(test), warn(clippy::expect_used))]

pub mod pathbytes;
pub mod repair;
pub mod scan;

use std::ffi::OsString;
use std::fmt;
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
    /// The number of links the run repaired, or, under [`Options::dry_run`],
    /// the number of links the run would have repaired.
    ///
    /// The flag changes what this number counts, and the name of the field
    /// cannot say so on its own, thus a reader of the count reads it beside the
    /// options the run was given. A dry run plans exactly the repairs a real
    /// run over the same tree would make — the deciding is the same work, and
    /// only the one call that writes to the tree is left out — so the two
    /// numbers agree.
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
pub fn run(options: &Options, out: &mut dyn Write, err: &mut dyn Write) -> Summary {
    line(
        err,
        format_args!("Scanning for broken symlinks: {}", options.root.display()),
    );

    let mut summary = Summary::default();
    scan::scan(options, out, err, &mut summary);

    // The closing summary counts broken links and nothing else. A link the
    // walk could not resolve was already reported on the error stream by
    // `scan::report_unresolvable`, which says there why it adds no line here.
    if summary.broken == 0 {
        line(out, format_args!("No broken symlinks found."));
    } else {
        line(
            out,
            format_args!("Found {} broken symlink(s).", summary.broken),
        );

        // A run that was never asked to repair anything says nothing about
        // repairs. The last line answers a question the caller asked, so a
        // caller who asked no question gets no answer: `No symlinks could be
        // fixed` under a run with no fix flag would read as a failure of a
        // tool that was only ever asked to look.
        if summary.fixed > 0 {
            // A dry run counted the repairs it planned, so the closing line
            // says what it would have done rather than claiming a change it did
            // not make. Everything else about the line is the same, thus the
            // two wordings sit in one place and cannot drift apart.
            let verb = if options.dry_run {
                "Would fix"
            } else {
                "Fixed"
            };
            line(out, format_args!("{verb} {} symlink(s).", summary.fixed));
        } else if options.prepend.is_some() || options.remove.is_some() {
            line(
                out,
                format_args!("No symlinks could be fixed with the provided options."),
            );
        }
    }

    summary
}

/// Writes one line to `w`, and drops the error the write may give back.
///
/// Every line this crate writes goes through here, so the decision to drop that
/// error sits in one place instead of at each call. The tool reports; it does
/// not depend on the report arriving. A reader that stopped reading — a pipe
/// into `head`, a terminal that went away — must not turn a scan of a tree into
/// a failure, and the writer a test gives this crate is a `Vec<u8>`, which can
/// fail in no way at all.
fn line(w: &mut dyn Write, message: fmt::Arguments<'_>) {
    let _ = w.write_fmt(format_args!("{message}\n"));
}
