//! `symfix` — the process around [`symfix::run`].
//!
//! The library walks a tree and writes its report to two writers. What is left
//! lives here: the command line, the directory the run scans, and the status a
//! shell reads afterwards.
//!
//! # Only a directory it cannot scan gives a failing status
//!
//! A run that warned, a run that could repair nothing, and a run that found
//! links it could not resolve all give status 0. That is the behavior of the
//! tool this port replaces — its `logger.Fatal` calls sit on the directory
//! alone, and its warnings go to the error stream and leave the status where it
//! was — and this port does not change it. So it is a decision and not an
//! oversight: a broken symlink is a finding, and a tool that reported a finding
//! as a failure would turn every scan of an imperfect tree into a failing step
//! of a script that only asked what was there.
//!
//! # `clap` answers `--help` on standard output
//!
//! The Go tool wrote its usage to standard error, because every `fmt.Fprintf`
//! of its `flag.Usage` named `os.Stderr`. This port keeps what `clap` does, and
//! the change is deliberate: a page the user asked for is the output of the
//! run, and every other Rust tool of this workspace answers `--help` there. A
//! refusal of the command line still goes to standard error.

#![cfg_attr(not(test), warn(clippy::unwrap_used))]
#![cfg_attr(not(test), warn(clippy::expect_used))]

use buildinfo::version_string;
use clap::{ArgAction, Parser};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use symfix::Options;

/// What `symfix` does, as `-h` and `--help` both print it.
const ABOUT: &str = "\
Recursively scans directories for broken symlinks and optionally repairs them.

A symlink holds its target as text, and the text says nothing about whether the
target is there. symfix finds the links whose target is not there, and repairs
the ones you tell it how to repair.";

/// How to run `symfix`, printed under the options.
///
/// Every invocation here spells a flag with two dashes. The Go `flag` package
/// takes `-dir` and `--dir` for one flag, and `clap` does not: a short flag
/// that takes a value swallows the characters glued to it, so `-dir` reaches
/// the parser as `-d` carrying the value `ir`, and the directory that follows
/// is then an argument this command line has no place for. A user who kept an
/// old alias thus reads a refusal, and this page gives the spelling that works.
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
    // and the library builds a new target out of those bytes rather than out of
    // characters. A `String` here would make a target that is not UTF-8
    // unspellable on the command line, which is exactly the target whose
    // repair nothing else can do.
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

/// The directory the run scans, or `None` when there is none to scan.
///
/// This follows the tool this port replaces, which calls `filepath.Abs` and
/// then `os.Stat`. [`std::path::absolute`] touches no file system, as
/// `filepath.Abs` does not, so the two checks stay in the order the Go tool has
/// them: make the path absolute, then ask the file system about it. The library
/// takes a root that is already absolute, and every path of the report is built
/// on that root, so a report names the same directory whatever the caller typed.
///
/// Each refusal writes one line to `err` and gives back `None`, and the caller
/// turns that into status 1. The write error is dropped, here and at the two
/// lines below it: this function reports, and it does not depend on the report
/// arriving. A reader that stopped reading must not turn a refusal into
/// something else.
fn resolve_root(dir: &Path, err: &mut dyn Write) -> Option<PathBuf> {
    let root = match std::path::absolute(dir) {
        Ok(root) => root,
        Err(error) => {
            let _ = writeln!(
                err,
                "Failed to resolve absolute path: {}: {error}",
                dir.display()
            );
            return None;
        }
    };

    match fs::metadata(&root) {
        Ok(metadata) if metadata.is_dir() => Some(root),
        Ok(_) => {
            let _ = writeln!(err, "Path is not a directory: {}", root.display());
            None
        }
        Err(error) => {
            let _ = writeln!(err, "Directory does not exist: {}: {error}", root.display());
            None
        }
    }
}

/// Runs the scan and gives the status a shell reads.
///
/// Both standard streams are locked one time and handed to [`symfix::run`],
/// which writes every line of the run through them. A walk of a large tree
/// writes many lines, and a lock taken for each of them would be a lock taken
/// for each line of the report.
///
/// Only [`resolve_root`] can make this run fail. The doc comment of this module
/// says why a run that found broken links it could not repair still gives
/// status 0.
fn main() -> ExitCode {
    let cli = Cli::parse();

    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();

    let Some(root) = resolve_root(&cli.dir, &mut err) else {
        return ExitCode::FAILURE;
    };

    let options = Options {
        root,
        prepend: cli.prepend_to_fix,
        remove: cli.remove_to_fix,
        dry_run: cli.dry_run,
        verbose: cli.verbose,
        skip: cli.skip,
    };

    symfix::run(&options, &mut out, &mut err);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_default_directory_is_the_one_the_shell_is_in() {
        let cli = Cli::try_parse_from(["symfix"]).expect("no argument is taken");

        assert_eq!(cli.dir, PathBuf::from("."));
        assert_eq!(
            cli.prepend_to_fix, None,
            "no repair is asked for by default"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_prepend_value_that_is_not_text_reaches_the_run_whole() {
        // These three live here, and not beside the other imports of the
        // module, because `std::os::unix` is absent where the tool builds for
        // another platform. The test itself is the one thing that needs them,
        // so the `cfg` it already carries is the only one this needs.
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        // A byte that begins no UTF-8 sequence. A symlink target that holds it
        // is a target a Unix kernel accepts and a Rust string cannot hold.
        const NOT_TEXT: u8 = 0x80;

        // This is why the value is an `OsString`. A target that is not UTF-8 is
        // exactly the target whose repair nothing else can do, and a `String`
        // here would refuse to carry the prefix that repairs it.
        let prefix = OsStr::from_bytes(&[NOT_TEXT, b'/']);

        let cli =
            Cli::try_parse_from([OsStr::new("symfix"), OsStr::new("--prepend-to-fix"), prefix])
                .expect("a prefix that is not text is taken");

        assert_eq!(cli.prepend_to_fix.as_deref(), Some(prefix));
    }

    #[test]
    fn a_second_skip_adds_a_name_rather_than_replacing_the_first() {
        let cli = Cli::try_parse_from(["symfix", "--skip", "node_modules", "--skip", ".git"])
            .expect("both names are taken");

        assert_eq!(
            cli.skip,
            vec![OsString::from("node_modules"), OsString::from(".git")]
        );
    }

    #[test]
    fn the_single_dash_spelling_of_dir_does_not_name_the_directory() {
        // `clap` reads `-dir` as `-d` carrying the value `ir`, so the directory
        // that follows has no place on this command line and the whole line is
        // refused. The help page and the README both say so, because a user who
        // kept the old alias meets this refusal and nothing else explains it.
        Cli::try_parse_from(["symfix", "-dir", "/some/path"])
            .expect_err("-dir is -d with the value ir, and the path is then extra");
    }
}
