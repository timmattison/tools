//! `symfix` — the process around [`symfix::run`].
//!
//! The library walks a tree and writes its report. What is left lives here: the
//! command line, the directory the run scans, and the status a shell reads
//! afterwards.

#![cfg_attr(not(test), warn(clippy::unwrap_used))]
#![cfg_attr(not(test), warn(clippy::expect_used))]

use buildinfo::version_string;
use clap::{ArgAction, Parser};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

/// What `symfix` does, as `-h` and `--help` both print it.
const ABOUT: &str = "\
Recursively scans directories for broken symlinks and optionally repairs them.

A symlink holds its target as text, and the text says nothing about whether the
target is there. symfix finds the links whose target is not there, and repairs
the ones you tell it how to repair.";

/// How to run `symfix`, printed under the options.
const AFTER_HELP: &str = "\
EXAMPLES:
  symfix                                 Scan the current directory
  symfix --dir /path/to/scan             Scan a specific directory
  symfix --prepend-to-fix ../            Repair by prepending \"../\" to the target
  symfix --remove-to-fix /old/path/      Repair by removing the \"/old/path/\" prefix
  symfix --prepend-to-fix ../ --dry-run  Print the plan and change nothing
  symfix --skip node_modules --skip .git Leave out the directories that hold no source

--prepend-to-fix is tried first, and --remove-to-fix is tried only when the
prepend did not repair the link. A repair writes the new target as it was built,
so a relative target stays relative.

THE SINGLE-DASH SPELLINGS ARE GONE. The Go version of this tool accepted -dir
and --dir for the same flag, and this version does not. Every flag needs two
dashes, except the short forms above.";

/// The command line of `symfix`.
//
// The documentation comments below belong to the flags, where `clap` turns each
// one into the description of its option. This note is a plain comment so that
// it stays out of the help: `clap` derives `about` from the first paragraph of
// a documentation comment on this struct and `long_about` from the whole of it,
// and a derived `long_about` wins over ABOUT in `--help`, so a documentation
// comment here would make the long help say something the short help does not.
#[derive(Debug, Parser)]
#[command(name = "symfix", version = version_string!(), about = ABOUT, after_help = AFTER_HELP)]
#[allow(
    dead_code,
    reason = "the skeleton of this slice parses the command line and does nothing with it; the run reads every field"
)]
struct Cli {
    /// The directory to scan
    //
    // A `PathBuf` and not a `String`: a directory name is a sequence of bytes
    // on Unix, and a user whose directory is not UTF-8 must still be able to
    // name it.
    #[arg(short, long, value_name = "DIR", default_value = ".")]
    dir: PathBuf,

    /// Put this string in front of a broken symlink target
    //
    // An `OsString` and not a `String`, for the same reason and one more: a
    // symlink target is bytes that the operating system never reads as text,
    // and the library strips and joins those bytes rather than characters. A
    // `String` here would make a target that is not UTF-8 unspellable on the
    // command line, which is exactly the target a repair is most needed for.
    #[arg(long, value_name = "STRING")]
    prepend_to_fix: Option<OsString>,

    /// Take this string off the front of a broken symlink target
    #[arg(long, value_name = "STRING")]
    remove_to_fix: Option<OsString>,

    /// Print every planned change and touch nothing
    #[arg(short = 'n', long)]
    dry_run: bool,

    /// Do not enter a directory with this name. Repeatable
    //
    // A directory name is bytes as well, so this is a `Vec<OsString>`, and
    // `ArgAction::Append` is what makes a second `--skip` add a name rather
    // than replace the one before it.
    #[arg(long, value_name = "NAME", action = ArgAction::Append)]
    skip: Vec<OsString>,

    /// Write the debug lines to standard error
    #[arg(short, long)]
    verbose: bool,
}

/// Parses the command line, and gives the status a shell reads.
fn main() -> ExitCode {
    let _cli = Cli::parse();
    ExitCode::SUCCESS
}
