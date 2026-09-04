//! Black-box tests of the `symfix` binary, driving the real command line of
//! the built tool.
//!
//! These tests prove what no test of the library reaches. [`symfix::run`] takes
//! a root that is already absolute and two writers, so the questions left over
//! belong to the process around it: which flag names which option, which
//! directory a run with no flags scans, what a run says about a directory it
//! will not scan, which status a shell reads afterwards, and which of the two
//! streams each line lands on. Only a process answers those.
//!
//! **This tool deletes and recreates symbolic links.** A test that named a path
//! in the repository, in the home directory, or anywhere else outside its own
//! temporary directory would let the tool rewrite that path. Every fixture
//! below lives in its own [`TempDir`], which `tempfile` names after the process
//! and a random word, so two runs of this file at the same time stay out of
//! each other's way. No test in this file names a path it did not just make,
//! and every new test must keep it that way.
//!
//! # The environment of a child is built from nothing
//!
//! [`run_in`] starts every child with `env_clear`, and puts nothing back. The
//! tool reads no variable of its own, and a child that inherited the
//! environment of `cargo test` would get whatever the terminal exported.
//! `std::env::set_var` and `std::env::set_current_dir` are never called: both
//! belong to the whole process, and `cargo test` runs these tests on many
//! threads. A child gets its directory through `Command::current_dir`.
//!
//! # These tests carry no timer
//!
//! macOS scans a freshly built unsigned binary the first time it runs, which
//! can take ten seconds and more. A deadline around the first child would thus
//! measure the machine rather than the code, so there is none.

#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "every unwrap and expect in this file is an assertion, not an unhandled error: on the temporary directory the test just made, on the links and files it just wrote there, and on the streams it just read back. The behavior of the binary is read through its status and its two streams, never through a panic"
)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// The flag that names the directory to scan.
const DIR: &str = "--dir";

/// The spelling of that flag the Go tool took.
///
/// The Go `flag` package reads `-dir` and `--dir` as one flag. `clap` reads a
/// short flag that takes a value together with the characters glued to it, so
/// `-dir` reaches the parser as `-d` carrying the value `ir`, and the path that
/// follows is then an argument this command line has no place for.
const OLD_DIR: &str = "-dir";

/// The flag that puts a string in front of a broken target.
const PREPEND: &str = "--prepend-to-fix";

/// The flag that takes a prefix off the front of a broken target.
const REMOVE: &str = "--remove-to-fix";

/// The flag that plans every repair and touches nothing.
const DRY_RUN: &str = "--dry-run";

/// The flag that names a directory the walk does not enter.
const SKIP: &str = "--skip";

/// The flag that asks for the help page.
const HELP: &str = "--help";

/// The flag that asks for the version.
const VERSION: &str = "--version";

/// What `--version` writes in front of the version of the package.
const VERSION_PREFIX: &str = "symfix ";

/// What opens the build in the version line.
const BUILD_OPENS: char = '(';

/// What the report says about each link whose target is not there.
const BROKEN: &str = "Broken symlink: ";

/// What the diagnostics say once, at the start of every run that scans.
const SCANNING: &str = "Scanning for broken symlinks: ";

/// What the report says about a repair the prepend made.
const FIXED_BY_PREPENDING: &str = "Fixed symlink by prepending: ";

/// What the report says about a repair a dry run only planned.
const WOULD_FIX: &str = "Would fix ";

/// The closing line of a run that found one broken link.
const FOUND_ONE: &str = "Found 1 broken symlink(s).";

/// The refusal of a directory the tool could not read at all.
const NO_DIRECTORY: &str = "Directory does not exist: ";

/// The refusal of a path that names something other than a directory.
const NOT_A_DIRECTORY: &str = "Path is not a directory: ";

/// What `clap` says about an argument this command line has no place for.
const UNEXPECTED_ARGUMENT: &str = "unexpected argument";

/// The status a run that refused its directory gives back.
const REFUSED: i32 = 1;

/// The name of the file a repaired link points at.
const TARGET: &str = "target.txt";

/// The name of the directory that holds the link in a repair fixture.
const SUBDIRECTORY: &str = "sub";

/// The name every link in these fixtures takes.
const LINK: &str = "link";

/// A target no fixture ever makes, so a link that holds it is broken.
const MISSING: &str = "missing.txt";

/// The prefix a repair puts in front of a target.
const PARENT: &str = "../";

/// The name of the directory one test asks the walk to leave out.
const SKIPPED_DIRECTORY: &str = "skipped";

/// The name of the directory beside it, which the same walk enters.
const KEPT_DIRECTORY: &str = "kept";

/// The name of a directory no test makes.
const NO_SUCH_DIRECTORY: &str = "no-such-directory";

/// The name of a plain file one test points `--dir` at.
const PLAIN_FILE: &str = "plain-file";

/// Starts the built binary in `working_dir` with `args`, and collects the
/// status and both streams.
///
/// The environment is built from nothing, and nothing goes back into it. The
/// tool reads no variable, so a child with an empty environment is a child that
/// behaves as the tool does on the machine of a user, and a child that
/// inherited this one would carry whatever the terminal exported into a test
/// that never mentioned it.
fn run_in(working_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_symfix"))
        .env_clear()
        .current_dir(working_dir)
        .args(args)
        .output()
        .expect("the built binary starts")
}

/// What the run wrote to standard output.
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// What the run wrote to standard error.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// `path` as text, for a command line that takes text.
///
/// The tool takes a directory as an `OsString` and never as a `String`, so this
/// is a convenience of the tests and not a limit of the tool. Every path here
/// is a temporary directory of the test that made it, and `tempfile` names one
/// out of ASCII.
fn text(path: &Path) -> &str {
    path.to_str().expect("a temporary path is UTF-8")
}

/// A new temporary directory, which removes itself and everything in it when
/// the test drops it.
fn scratch() -> TempDir {
    tempfile::tempdir().expect("a temporary directory")
}

/// A temporary directory holding one broken link named [`LINK`].
fn one_broken_link() -> TempDir {
    let base = scratch();
    symlink(MISSING, base.path().join(LINK)).expect("the link is made");
    base
}

/// A temporary directory holding `target.txt` and `sub/link -> target.txt`.
///
/// That link is broken: a relative target resolves against the directory that
/// holds the link, and no `target.txt` sits in `sub`. `--prepend-to-fix ../`
/// builds `../target.txt`, which resolves to the file in the root, so this is
/// the tree a repair through the command line acts on.
fn repairable_by_prepending() -> TempDir {
    let base = scratch();
    fs::write(base.path().join(TARGET), b"contents").expect("the target is written");
    let sub = base.path().join(SUBDIRECTORY);
    fs::create_dir(&sub).expect("the directory is made");
    symlink(TARGET, sub.join(LINK)).expect("the link is made");
    base
}

/// The link of a [`repairable_by_prepending`] tree.
fn repairable_link(base: &TempDir) -> PathBuf {
    base.path().join(SUBDIRECTORY).join(LINK)
}

#[test]
fn the_version_flag_writes_the_name_and_the_build() {
    let base = scratch();

    let output = run_in(base.path(), &[VERSION]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let written = stdout(&output);
    let line = written.trim_end();
    assert!(line.starts_with(VERSION_PREFIX), "{line:?}");
    // The hash and the state belong to the build, so this reads the shape of
    // the line and never the values in it. A test that pinned one hash would
    // fail on the next commit.
    assert!(line.contains(BUILD_OPENS), "{line:?}");
}

#[test]
fn the_help_flag_names_every_flag_on_standard_output() {
    let base = scratch();

    let output = run_in(base.path(), &[HELP]);

    // The Go tool wrote its usage to standard error, because every
    // `fmt.Fprintf` of its `flag.Usage` named `os.Stderr`. This port keeps what
    // `clap` does, and the change is deliberate: a page the user asked for is
    // the output of the run, and every other Rust tool of this workspace
    // answers `--help` on standard output.
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let written = stdout(&output);
    for flag in [DIR, PREPEND, REMOVE, DRY_RUN, SKIP] {
        assert!(written.contains(flag), "{flag} is missing from {written}");
    }
    assert_eq!(stderr(&output), "");
}

#[test]
fn a_directory_that_is_not_there_is_refused() {
    let base = scratch();
    let missing = base.path().join(NO_SUCH_DIRECTORY);

    let output = run_in(base.path(), &[DIR, text(&missing)]);

    assert_eq!(
        output.status.code(),
        Some(REFUSED),
        "stderr: {}",
        stderr(&output)
    );
    let diagnostics = stderr(&output);
    assert!(diagnostics.contains(NO_DIRECTORY), "{diagnostics}");
    assert!(diagnostics.contains(text(&missing)), "{diagnostics}");
    assert_eq!(stdout(&output), "");
}

#[test]
fn a_path_that_names_a_file_is_refused() {
    let base = scratch();
    let file = base.path().join(PLAIN_FILE);
    fs::write(&file, b"contents").expect("the file is written");

    let output = run_in(base.path(), &[DIR, text(&file)]);

    assert_eq!(
        output.status.code(),
        Some(REFUSED),
        "stderr: {}",
        stderr(&output)
    );
    let diagnostics = stderr(&output);
    assert!(diagnostics.contains(NOT_A_DIRECTORY), "{diagnostics}");
    assert!(diagnostics.contains(text(&file)), "{diagnostics}");
    assert_eq!(stdout(&output), "");
}

#[test]
fn a_run_with_no_flags_scans_the_working_directory() {
    // The default of `--dir` is `.`, so this pins that the default is read
    // against the directory of the process and not against anything else.
    let base = one_broken_link();

    let output = run_in(base.path(), &[]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let report = stdout(&output);
    assert!(report.contains(BROKEN), "{report}");
    assert!(report.contains(LINK), "{report}");
}

#[test]
fn a_repair_through_the_command_line_leaves_a_link_that_resolves() {
    let base = repairable_by_prepending();
    let link = repairable_link(&base);

    let output = run_in(base.path(), &[DIR, text(base.path()), PREPEND, PARENT]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let report = stdout(&output);
    assert!(report.contains(FIXED_BY_PREPENDING), "{report}");
    // The new link holds the target as it was built, so a relative target stays
    // relative and a tree that moves again keeps working.
    assert_eq!(
        fs::read_link(&link).expect("the repaired link is read"),
        PathBuf::from(format!("{PARENT}{TARGET}"))
    );
    // `metadata` follows the link, so this asks the operating system the
    // question the tool asked, and the answer is the whole point of a repair.
    fs::metadata(&link).expect("the repaired link resolves");
}

#[test]
fn a_dry_run_through_the_command_line_prints_the_plan_and_changes_nothing() {
    let base = repairable_by_prepending();
    let link = repairable_link(&base);

    let output = run_in(
        base.path(),
        &[DIR, text(base.path()), PREPEND, PARENT, DRY_RUN],
    );

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let report = stdout(&output);
    assert!(report.contains(WOULD_FIX), "{report}");
    assert!(!report.contains(FIXED_BY_PREPENDING), "{report}");
    assert_eq!(
        fs::read_link(&link).expect("the link is read"),
        PathBuf::from(TARGET),
        "a dry run must not rewrite the link"
    );
    fs::metadata(&link).expect_err("a dry run leaves the link broken");
}

#[test]
fn a_skipped_directory_is_left_out_of_the_walk() {
    let base = scratch();
    let skipped = base.path().join(SKIPPED_DIRECTORY);
    let kept = base.path().join(KEPT_DIRECTORY);
    for directory in [&skipped, &kept] {
        fs::create_dir(directory).expect("the directory is made");
        symlink(MISSING, directory.join(LINK)).expect("the link is made");
    }

    let output = run_in(
        base.path(),
        &[DIR, text(base.path()), SKIP, SKIPPED_DIRECTORY],
    );

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let report = stdout(&output);
    assert!(report.contains(text(&kept.join(LINK))), "{report}");
    assert!(!report.contains(text(&skipped.join(LINK))), "{report}");
    assert!(report.contains(FOUND_ONE), "{report}");
}

#[test]
fn the_single_dash_spelling_of_dir_is_refused() {
    // This pins the flag syntax the README documents. The Go tool took `-dir`
    // and `--dir` for one flag; this one does not, so a user who kept an old
    // alias reads a refusal rather than watching a tool of symbolic links scan
    // a directory nobody named.
    let base = one_broken_link();

    let output = run_in(base.path(), &[OLD_DIR, text(base.path())]);

    // `clap` gives status 2 for a command line it refused, and the number is
    // not what this test is about: what matters is that the run refused and
    // scanned nothing.
    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    let diagnostics = stderr(&output);
    assert!(diagnostics.contains(UNEXPECTED_ARGUMENT), "{diagnostics}");
    assert!(!stdout(&output).contains(BROKEN), "{}", stdout(&output));
}

#[test]
fn the_report_goes_to_standard_output_and_the_diagnostics_go_to_standard_error() {
    // A caller can thus send the report through a pipe and still read the
    // diagnostics, which is what keeps `symfix | grep` useful.
    let base = one_broken_link();

    let output = run_in(base.path(), &[DIR, text(base.path())]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let report = stdout(&output);
    let diagnostics = stderr(&output);
    assert!(report.contains(BROKEN), "{report}");
    assert!(!report.contains(SCANNING), "{report}");
    assert!(diagnostics.contains(SCANNING), "{diagnostics}");
    assert!(!diagnostics.contains(BROKEN), "{diagnostics}");
}
