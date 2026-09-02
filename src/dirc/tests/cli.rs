//! Black-box tests of the `dirc` binary, driving the real command line of the
//! built tool.
//!
//! These tests prove the paths no unit test reaches. Copy mode reads the
//! directory of the process, so only a process that starts in a directory shows
//! which directory it copies. Paste mode writes a line for a shell, so only a
//! shell that runs that line shows the line is safe. A refusal is a status and
//! two streams together, and only a process gives all three.
//!
//! # No child of this file touches the clipboard of the machine
//!
//! The clipboard is one shared resource of the whole machine. A test that
//! writes it destroys what the person at the keyboard copied, and two such
//! tests at the same time destroy each other. So every child gets
//! [`CLIPBOARD_FILE_ENV`], which names a file in the temporary directory of the
//! one test that made it.
//!
//! One helper, [`run_in`], starts every child of this file, and it always names
//! that file. The rule is thus a mechanism and not something each test must
//! remember: a test that forgot it would have to start a child of its own.
//!
//! # The environment of a child is built from nothing
//!
//! `env_clear` comes first, and two variables go back in: the clipboard file,
//! and [`LOGICAL_DIRECTORY_ENV`], which every shell exports. A child that
//! inherited the environment of `cargo test` would get whatever the terminal
//! exported, and a variable that names a clipboard file is exactly the kind of
//! variable a person exports by hand. `std::env::set_var` is never called: the
//! environment belongs to the whole process, and `cargo test` runs these tests
//! on many threads.
//!
//! Every path of every test lives in a temporary directory of that one test, so
//! two runs of this file at the same time stay out of each other's way.

#![cfg(unix)]

use dirc::clipboard::CLIPBOARD_FILE_ENV;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The name of the file that stands in for the clipboard of the machine.
///
/// The file sits in the temporary directory of the one test that made it, so
/// the name itself does not have to be unique.
const CLIPBOARD_FILE: &str = "clipboard.txt";

/// The flag that asks for paste mode.
const PASTE: &str = "--paste";

/// The spelling of that flag the Go tool took. `clap` reads it as five short
/// flags and refuses it.
const OLD_PASTE: &str = "-paste";

/// The flag that asks for the help page.
const HELP: &str = "--help";

/// The flag that asks for the version.
const VERSION: &str = "--version";

/// The mode the usage text of the Go tool named. No flag of either tool ever
/// gave it: copying is what happens when [`PASTE`] is absent.
const COPY_FLAG: &str = "-copy";

/// What copy mode writes in front of the path it put on the clipboard.
const COPIED: &str = "Copied to clipboard: ";

/// The variable a shell exports to name the directory the person walked to.
///
/// It holds the path with every symlink still in it, where `getcwd` holds the
/// path the kernel resolved. `dirc` reads the second one, and this file gives
/// every child the first one as well, so a `dirc` that read the wrong one is a
/// `dirc` these tests catch.
const LOGICAL_DIRECTORY_ENV: &str = "PWD";

/// The status of a run that refused the clipboard it was given.
const REFUSED: i32 = 1;

/// A directory name of characters that no single byte holds.
const MULTI_BYTE_NAME: &str = "日本語 café 🎉";

/// The refusal of a clipboard that holds nothing.
const EMPTY: &str = "Clipboard is empty";

/// The refusal of a clipboard that holds whitespace and nothing else.
const ONLY_WHITESPACE: &str = "Clipboard contains only whitespace";

/// The refusal of a path that cannot be read.
const INVALID_PATH: &str = "Invalid directory path in clipboard";

/// The refusal of a path that names something that is not a directory.
const NOT_A_DIRECTORY: &str = "Path in clipboard is not a directory";

/// What `--version` writes in front of the version of the package.
const VERSION_PREFIX: &str = "dirc ";

/// What stands between the version and the build, in the version line.
const BUILD_OPENS: &str = " (";

/// What stands between the hash and the state, in the version line.
const FIELD_SEPARATOR: &str = ", ";

/// How many characters the short hash of a commit holds.
const HASH_LENGTH: usize = 7;

/// What a build that could not ask git says in place of the hash, and in place
/// of the state.
const UNKNOWN: &str = "unknown";

/// What the version line says about the tree the build came from.
const BUILD_STATES: [&str; 3] = ["clean", "dirty", UNKNOWN];

/// The shell that runs the `cd` line. Every Unix holds one at this path.
const SHELL: &str = "/bin/sh";

/// The script that shell runs. It evaluates the line the tool printed, which is
/// what `eval $(dirc --paste)` does, and then says where it landed.
const EVAL_AND_REPORT: &str = r#"eval "$1"; pwd"#;

/// The flag that gives `sh` a script on its command line.
const SHELL_SCRIPT_FLAG: &str = "-c";

/// The name the script reads as `$0`. `sh -c` takes that name before the
/// arguments, so a script that reads `$1` has to name it.
const SHELL_NAME: &str = "sh";

/// The temporary directory of one test, and the clipboard file inside it.
struct Scratch {
    /// The directory. It removes itself and everything in it when the test
    /// drops it.
    dir: tempfile::TempDir,
}

impl Scratch {
    /// A new temporary directory, with no clipboard file in it yet.
    ///
    /// `tempfile` names the directory after the process and a random word,
    /// which is what keeps two runs of these tests at the same time out of each
    /// other's way.
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("a temporary directory"),
        }
    }

    /// The directory itself.
    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The file the children of this test read and write in place of the
    /// clipboard of the machine.
    fn clipboard(&self) -> PathBuf {
        self.dir.path().join(CLIPBOARD_FILE)
    }

    /// Puts `copied` on the clipboard of this test.
    fn write_clipboard(&self, copied: &str) {
        std::fs::write(self.clipboard(), copied).expect("the clipboard file is written");
    }

    /// What the clipboard of this test holds.
    fn clipboard_holds(&self) -> String {
        std::fs::read_to_string(self.clipboard()).expect("the clipboard file is read")
    }

    /// A directory named `name`, made inside this one.
    fn directory(&self, name: &str) -> PathBuf {
        let child = self.dir.path().join(name);
        std::fs::create_dir(&child).expect("the directory is made");
        child
    }
}

/// Runs the built `dirc` in `working_dir`, over the clipboard of `scratch`.
///
/// This is the one place in this file that starts a `dirc`, and it always names
/// the clipboard file of the test. No child of this file reaches the clipboard
/// of the machine, because no other path starts a child.
///
/// The environment holds two variables and nothing else. `dirc` starts no
/// program, so it needs no `PATH`, and every other variable it could read is a
/// variable the terminal that started `cargo test` chose.
///
/// The second variable is `PWD`, and it names `working_dir` as the parent wrote
/// it. Every shell exports that variable, so a `dirc` a person runs always has
/// one, and the value a shell puts there is the path the person walked and not
/// the path the kernel holds. The variable is set for that reason: a copy mode
/// that read it in place of `getcwd` would pass under an environment that
/// carried none, and would put the wrong path on the clipboard for every user.
fn run_in(scratch: &Scratch, working_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dirc"))
        .env_clear()
        .env(CLIPBOARD_FILE_ENV, scratch.clipboard())
        .env(LOGICAL_DIRECTORY_ENV, working_dir)
        .current_dir(working_dir)
        .args(args)
        .output()
        .expect("the built binary starts")
}

/// The same, in the temporary directory of the test.
///
/// Paste mode never reads the directory of the process, and neither does a run
/// that only prints. Such a run names no directory of its own.
fn run(scratch: &Scratch, args: &[&str]) -> Output {
    run_in(scratch, scratch.path(), args)
}

/// What the run wrote to standard output.
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// What the run wrote to standard error.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// `path` as text.
fn text(path: &Path) -> &str {
    path.to_str().expect("a temporary path is UTF-8")
}

/// `path` with every symlink in it resolved, as text.
///
/// `getcwd` gives a child the resolved path of its directory, whatever path the
/// parent named. On macOS a temporary directory sits under `/var`, and `/var`
/// is itself a symlink to `/private/var`, so a test that compared the output of
/// copy mode against the path it built would fail for a reason that has nothing
/// to do with the tool.
fn resolved(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).expect("the directory is there");
    text(&canonical).to_string()
}

/// `path` as text, checked to hold no quote.
///
/// A test that writes an expected `cd` line by hand builds it on this text. A
/// quote in the path would be escaped by the code under test, and the test
/// would then fail for a reason that has nothing to do with the code.
fn quoteless(path: &Path) -> &str {
    let written = text(path);
    assert!(!written.contains('\''), "the path holds a quote: {written}");
    written
}

/// Asserts that paste mode refused the clipboard of `scratch` and said
/// `message`.
///
/// The three parts of a refusal are asserted together, because each one has a
/// different reader. The status is what a script reads. The empty standard
/// output is what keeps `eval` from running a half-written line. The message is
/// what the person at the keyboard reads.
///
/// The assertion on the message is `starts_with`, because two of the four
/// refusals name the path and what the operating system said after it.
fn assert_paste_refuses(scratch: &Scratch, message: &str) {
    let output = run(scratch, &[PASTE]);
    assert_eq!(
        output.status.code(),
        Some(REFUSED),
        "stderr: {}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "", "eval would run this");
    let said = stderr(&output);
    assert!(said.starts_with(message), "{said}");
}

#[test]
fn copy_mode_copies_the_directory_the_symlink_led_to() {
    // `getcwd` resolves the symlink before `dirc` ever sees it, so the tool
    // puts the directory the link led to on the clipboard. That is the path the
    // other shell reaches whatever later happens to the link, and this is the
    // one place the choice can be proved: a unit test hands the path in, and a
    // process reads it from the kernel.
    let scratch = Scratch::new();
    let real = scratch.directory("real");
    let link = scratch.path().join("link");
    symlink(&real, &link).expect("the symlink is made");

    let output = run_in(&scratch, &link, &[]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let led_to = resolved(&real);
    assert_eq!(stdout(&output), format!("{COPIED}{led_to}\n"));
    // The clipboard carries no newline. The newline belongs to the line the
    // reader sees, and a path with one on it names no directory.
    assert_eq!(scratch.clipboard_holds(), led_to);
}

#[test]
fn copy_mode_copies_a_name_of_multi_byte_characters_whole() {
    // A path that lost a byte names a different directory, and the shell that
    // reads the clipboard then goes somewhere else or goes nowhere.
    let scratch = Scratch::new();
    let child = scratch.directory(MULTI_BYTE_NAME);

    let output = run_in(&scratch, &child, &[]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let copied = resolved(&child);
    assert!(copied.ends_with(MULTI_BYTE_NAME), "{copied}");
    assert_eq!(stdout(&output), format!("{COPIED}{copied}\n"));
    assert_eq!(scratch.clipboard_holds(), copied);
}

#[test]
fn paste_mode_writes_the_cd_line_for_the_directory_the_clipboard_names() {
    let scratch = Scratch::new();
    let child = scratch.directory("plain");
    scratch.write_clipboard(text(&child));

    let output = run(&scratch, &[PASTE]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), format!("cd '{}'\n", quoteless(&child)));
    // The shell runs standard output. A word on standard error would be a word
    // the reader sees and the shell does not, so a quiet run is part of the
    // contract of the good path.
    assert_eq!(stderr(&output), "");
}

#[test]
fn the_shell_runs_the_cd_line_of_a_name_that_holds_a_quote() {
    // This is `eval $(dirc --paste)`, which is how a person runs the tool. A
    // quote in the name is shell syntax until the line escapes it, so a line
    // that dropped the escape leaves the shell where it was. Only a shell shows
    // that, and it is what the escape is worth having for.
    let scratch = Scratch::new();
    let child = scratch.directory("it's here");
    scratch.write_clipboard(text(&child));

    let printed = run(&scratch, &[PASTE]);
    assert!(printed.status.success(), "stderr: {}", stderr(&printed));
    let written = stdout(&printed);
    let line = written.trim_end();

    let landed = Command::new(SHELL)
        .env_clear()
        // The shell starts in the temporary directory, so a line the shell
        // refused leaves it here and this test then reads a directory that is
        // not the one the clipboard named.
        .current_dir(scratch.path())
        .args([SHELL_SCRIPT_FLAG, EVAL_AND_REPORT, SHELL_NAME, line])
        .output()
        .expect("the shell starts");

    assert!(landed.status.success(), "stderr: {}", stderr(&landed));
    let said = stdout(&landed);
    let there = Path::new(said.trim_end());
    // Both paths are resolved before they are compared. `cd` keeps the path it
    // was given, symlinks and all, and `pwd` gives that path back, so the shell
    // reports `/var` where the kernel reports `/private/var`.
    assert_eq!(
        resolved(there),
        resolved(&child),
        "the shell landed in {there:?}"
    );
}

#[test]
fn an_empty_clipboard_stops_paste_mode() {
    let scratch = Scratch::new();
    scratch.write_clipboard("");

    assert_paste_refuses(&scratch, EMPTY);
}

#[test]
fn a_clipboard_of_whitespace_stops_paste_mode() {
    let scratch = Scratch::new();
    scratch.write_clipboard(" \t\r\n ");

    assert_paste_refuses(&scratch, ONLY_WHITESPACE);
}

#[test]
fn a_path_that_is_not_there_stops_paste_mode() {
    let scratch = Scratch::new();
    let missing = scratch.path().join("no-such-directory");
    scratch.write_clipboard(text(&missing));

    assert_paste_refuses(&scratch, INVALID_PATH);
}

#[test]
fn a_path_that_names_a_file_stops_paste_mode() {
    let scratch = Scratch::new();
    let file = scratch.path().join("not-a-directory");
    std::fs::write(&file, b"").expect("the file is made");
    scratch.write_clipboard(text(&file));

    assert_paste_refuses(&scratch, NOT_A_DIRECTORY);
}

#[test]
fn the_version_flag_writes_the_build_string() {
    // The hash and the state belong to the build, so the test reads the shape
    // of the line and never the values in it. A test that pinned one hash would
    // fail on the next commit.
    let scratch = Scratch::new();
    let output = run(&scratch, &[VERSION]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let written = stdout(&output);
    assert_eq!(written.lines().count(), 1, "{written:?}");
    let line = written.trim_end();

    let shape = line
        .strip_prefix(VERSION_PREFIX)
        .and_then(|rest| rest.strip_suffix(')'))
        .and_then(|fields| fields.split_once(BUILD_OPENS));
    let Some((version, build)) = shape else {
        panic!(
            "`dirc {VERSION}` must write `dirc <version> (<hash>, <state>)`, and it wrote {line:?}"
        );
    };
    let Some((hash, state)) = build.split_once(FIELD_SEPARATOR) else {
        panic!("`dirc {VERSION}` must write one hash and one state, and the parentheses hold {build:?}");
    };

    assert!(
        !version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.'),
        "the version is a number of parts: {version:?}"
    );
    // The characters are counted and never the bytes, because a hash that is
    // not a hash can hold anything at all.
    let hash_is_short = hash == UNKNOWN
        || (hash.chars().count() == HASH_LENGTH && hash.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(
        hash_is_short,
        "the hash is {HASH_LENGTH} hexadecimal characters or `{UNKNOWN}`: {hash:?}"
    );
    assert!(
        BUILD_STATES.contains(&state),
        "the state is one of {BUILD_STATES:?}: {state:?}"
    );
}

#[test]
fn the_help_names_the_flag_that_runs_and_names_no_flag_that_does_not() {
    let scratch = Scratch::new();
    let output = run(&scratch, &[HELP]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let help = stdout(&output);
    assert!(help.contains(PASTE), "{help}");

    // The usage of the Go tool named a `-copy` mode. No such flag was ever
    // there, in that tool or in this one, so a reader who typed what the page
    // said got a refusal.
    assert!(!help.contains(COPY_FLAG), "{help}");

    // `--paste` holds `-paste`, so the long flag comes out of the page before
    // the page is read for the short one. `clap` reads `-paste` as five short
    // flags and refuses it, so a page that showed that spelling would teach a
    // reader a command that does not run.
    let without_the_long_flag = help.replace(PASTE, "");
    assert!(!without_the_long_flag.contains(OLD_PASTE), "{help}");
}

#[test]
fn the_single_dash_spelling_writes_nothing_to_standard_output() {
    // A person whose shell alias still says `eval $(dirc -paste)` gets the
    // refusal of `clap`. That refusal goes to standard error, and standard
    // output stays empty, so `eval` runs nothing and the shell stays where it
    // is. A refusal on standard output would be a line the shell runs.
    let scratch = Scratch::new();

    let output = run(&scratch, &[OLD_PASTE]);

    assert_eq!(stdout(&output), "", "eval would run this");
    assert!(!stderr(&output).is_empty(), "the refusal says why");
    // The status of a command line `clap` refused belongs to `clap`, so the
    // test reads that the run failed and not which number it failed with.
    assert!(!output.status.success());
}
