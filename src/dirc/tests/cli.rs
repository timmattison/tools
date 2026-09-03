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
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The name of the file that stands in for the clipboard of the machine.
///
/// The file sits in the temporary directory of the one test that made it, so
/// the name itself does not have to be unique.
const CLIPBOARD_FILE: &str = "clipboard.txt";

/// A byte that begins no UTF-8 sequence.
///
/// A path that holds it is a path a Unix kernel accepts and a Rust string
/// cannot hold. One test names its clipboard file that way.
const NOT_TEXT: u8 = 0xff;

/// The name of a plain file one test makes.
///
/// No directory sits under a plain file, so the kernel answers a read of any
/// path under this one with "Not a directory".
const PLAIN_FILE: &str = "plain-file";

/// The refusal of a clipboard the tool could not open or read.
const UNREACHABLE: &str = "Failed to reach the clipboard: ";

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

/// The script that shell runs. It evaluates the line the tool printed as one
/// argument, and then says where it landed.
///
/// This is the quoted substitution of `eval "$(dirc --paste)"` and not the
/// idiom itself. The idiom comes out of the help page, in
/// `the_shell_runs_the_idiom_the_help_teaches_into_a_name_that_holds_a_tab`.
const EVAL_AND_REPORT: &str = r#"eval "$1"; pwd"#;

/// The flag that gives `sh` a script on its command line.
const SHELL_SCRIPT_FLAG: &str = "-c";

/// The name the script reads as `$0`. `sh -c` takes that name before the
/// arguments, so a script that reads `$1` has to name it.
const SHELL_NAME: &str = "sh";

/// What the script says after the idiom of the help, so the test reads where
/// the shell landed.
const REPORT_WHERE: &str = "; pwd";

/// The word the help spells the tool with.
const TOOL_WORD: &str = "dirc";

/// What the test writes in place of that word.
///
/// The path of the built binary goes to the shell as an argument, and the
/// script names it here. This reference carries quotes of its own, so a build
/// directory whose path holds a space stays one word. What is left for the test
/// to read is the quoting the help teaches around the substitution.
const BINARY_REFERENCE: &str = "\"$1\"";

/// The label the help puts in front of the line for Bash and Zsh.
const BASH_LABEL: &str = "Bash/Zsh:";

/// The label the help puts in front of the line for fish.
const FISH_LABEL: &str = "Fish:";

/// The word that opens an alias. It tells the line of the TIP paragraph from
/// the line of the NOTE paragraph, because both carry the same label.
const ALIAS: &str = "alias ";

/// The name the alias of the help takes.
const ALIAS_NAME: &str = "dirp=";

/// What a fish substitution needs, to stay one word.
const COLLECT: &str = "string collect";

/// A directory name that holds a tab.
///
/// A tab is one of the three characters the field separator of a shell holds.
/// An unquoted substitution is thus split at this tab, and the pieces come back
/// together with a space between them.
const TAB_NAME: &str = "holds\ta tab";

/// The file that documents every tool of this workspace.
const README: &str = "README.md";

/// The temporary directory of one test, and the clipboard file inside it.
struct Scratch {
    /// The directory. It removes itself and everything in it when the test
    /// drops it.
    dir: tempfile::TempDir,

    /// The name of the clipboard file in that directory.
    ///
    /// `OsString` and not `String`, because a Unix kernel accepts a file name
    /// that no Rust string holds. The value goes to the child through the
    /// environment, so a test names such a file and reads what the tool does
    /// with the value.
    clipboard_name: OsString,
}

impl Scratch {
    /// A new temporary directory, with no clipboard file in it yet.
    ///
    /// `tempfile` names the directory after the process and a random word,
    /// which is what keeps two runs of these tests at the same time out of each
    /// other's way.
    fn new() -> Self {
        Self::with_clipboard_name(CLIPBOARD_FILE)
    }

    /// The same, with `name` for the clipboard file.
    fn with_clipboard_name(name: impl Into<OsString>) -> Self {
        Self {
            dir: tempfile::tempdir().expect("a temporary directory"),
            clipboard_name: name.into(),
        }
    }

    /// The directory itself.
    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The file the children of this test read and write in place of the
    /// clipboard of the machine.
    fn clipboard(&self) -> PathBuf {
        self.dir.path().join(&self.clipboard_name)
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

/// The help page of the built tool.
///
/// The page comes out of a run of the binary and never out of the source, so
/// every test below reads what a user reads. The run goes through [`run`], so
/// it names a clipboard file of its own like every other child of this file.
fn help_page() -> String {
    let scratch = Scratch::new();
    let output = run(&scratch, &[HELP]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    stdout(&output)
}

/// The two invocations `help` gives under `label`: the idiom, then the alias.
///
/// The page names each shell two times. The NOTE paragraph gives the idiom
/// itself, and the TIP paragraph gives the alias that holds it. Both lines
/// carry the same label, so [`ALIAS`] is what tells them apart.
///
/// The count of each is asserted. A page that lost one of the two lines would
/// otherwise leave a test with nothing to read, and a test that reads nothing
/// reports clean for the wrong reason.
fn invocations(help: &str, label: &str) -> (String, String) {
    let mut idioms = Vec::new();
    let mut aliases = Vec::new();

    for line in help.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix(label) else {
            continue;
        };
        let given = rest.trim().to_string();
        if given.starts_with(ALIAS) {
            aliases.push(given);
        } else {
            idioms.push(given);
        }
    }

    assert_eq!(idioms.len(), 1, "one {label} idiom is in the help: {help}");
    assert_eq!(aliases.len(), 1, "one {label} alias is in the help: {help}");
    (idioms.remove(0), aliases.remove(0))
}

/// `idiom` with a reference to the built binary in place of the word the help
/// spells the tool with.
///
/// The path itself never enters the text. It goes to the shell as an argument,
/// and [`BINARY_REFERENCE`] names it, so the characters of the build directory
/// change nothing. The quoting the help teaches is then the only quoting the
/// shell reads.
fn with_built_binary(idiom: &str) -> String {
    assert_eq!(
        idiom.matches(TOOL_WORD).count(),
        1,
        "the idiom names the tool one time: {idiom}"
    );
    idiom.replace(TOOL_WORD, BINARY_REFERENCE)
}

/// The text of the README of this repository.
///
/// `CARGO_MANIFEST_DIR` names `src/dirc`, so the root of the repository is two
/// directories above it. A read that fails stops the test, because a test that
/// found no file must not report that the file agrees with the help.
fn readme() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(README);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|failure| panic!("{README} is read from {path:?}: {failure}"))
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
/// refusals name the path and what the operating system said after it. The
/// whole message comes back, so a caller that wants more of it reads the rest.
fn assert_paste_refuses(scratch: &Scratch, message: &str) -> String {
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
    said
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
fn a_clipboard_file_whose_name_is_not_text_is_the_file_the_tool_opens() {
    // A Unix kernel takes a path that no Rust string holds, and `std::env::var`
    // gives nothing back for such a value. A tool that drops the value opens
    // the clipboard of the machine, which is the one outcome this variable
    // exists to prevent: the caller names a file, and the shared clipboard gets
    // the work.
    //
    // The mode is paste, because paste mode only reads. A run that falls back
    // here reads the clipboard of the machine and changes nothing on it, where
    // a copy that falls back destroys what the person at the keyboard put
    // there.
    //
    // The clipboard file is never made. APFS refuses every file name that is
    // not text, so no test makes one on macOS. The file sits under a plain file
    // instead, and the kernel answers a read of that path with "Not a
    // directory". The refusal then names the path the tool opened, byte for
    // byte, where a run that fell back to the clipboard of the machine names no
    // path at all.
    let named = PathBuf::from(PLAIN_FILE).join(OsString::from_vec(vec![NOT_TEXT]));
    let scratch = Scratch::with_clipboard_name(named);
    std::fs::write(scratch.path().join(PLAIN_FILE), b"").expect("the file is made");

    let said = assert_paste_refuses(&scratch, UNREACHABLE);

    // `Debug` writes the path, so the byte that is not text arrives as an
    // escape. A tool that made the value text again writes a replacement
    // character in place of that escape.
    assert!(said.contains(&format!("{:?}", scratch.clipboard())), "{said}");
}

#[test]
fn the_shell_runs_the_cd_line_of_a_name_that_holds_a_quote() {
    // The shell gets the printed line as one argument, which is what the
    // quoted substitution of `eval "$(dirc --paste)"` gives it. A quote in the
    // name is shell syntax until the line escapes it, so a line that dropped
    // the escape leaves the shell where it was. Only a shell shows that, and it
    // is what the escape is worth having for.
    //
    // The test below runs the idiom of the help page whole, over a name that
    // holds a tab, and that is the test which reads the quoting of the
    // substitution. This one reads the quoting of the path.
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
fn the_shell_runs_the_idiom_the_help_teaches_into_a_name_that_holds_a_tab() {
    // The command this test runs comes out of the help page. The page and the
    // test thus cannot drift apart: a page that teaches a command which does
    // not work is a test that fails.
    //
    // The name of the directory holds a tab, which is one of the three
    // characters the field separator of a shell holds. A substitution the line
    // leaves unquoted is split at that tab, and `eval` puts the pieces back
    // together with a space between them, so `cd` gets a name that is not
    // there. The shell then stays in the temporary directory, and `pwd` says
    // so.
    let scratch = Scratch::new();
    let child = scratch.directory(TAB_NAME);
    scratch.write_clipboard(text(&child));

    let (idiom, _alias) = invocations(&help_page(), BASH_LABEL);
    let script = format!("{}{REPORT_WHERE}", with_built_binary(&idiom));

    let landed = Command::new(SHELL)
        .env_clear()
        // The children of this shell read the clipboard of this test. No child
        // of this file reaches the clipboard of the machine.
        .env(CLIPBOARD_FILE_ENV, scratch.clipboard())
        .current_dir(scratch.path())
        .args([
            SHELL_SCRIPT_FLAG,
            script.as_str(),
            SHELL_NAME,
            env!("CARGO_BIN_EXE_dirc"),
        ])
        .output()
        .expect("the shell starts");

    let said = stdout(&landed);
    let there = Path::new(said.trim_end());
    // Both paths are resolved before they are compared, because `pwd` gives
    // back the path `cd` was given and macOS puts a temporary directory under
    // a symlink.
    assert_eq!(
        resolved(there),
        resolved(&child),
        "the shell ran {script:?}, landed in {there:?}, and said {:?}",
        stderr(&landed)
    );
}

#[test]
fn the_alias_of_the_help_holds_the_idiom_of_the_help() {
    // A reader installs the alias of the TIP paragraph and runs the idiom of
    // the NOTE paragraph. The test above runs the idiom, so the alias has to
    // hold that same idiom. Two paragraphs that drifted apart would teach two
    // commands, and only one of them would be under test.
    let help = help_page();

    for label in [BASH_LABEL, FISH_LABEL] {
        let (idiom, alias) = invocations(&help, label);
        assert!(
            alias.contains(&idiom),
            "the {label} alias is {alias:?} and the {label} idiom is {idiom:?}"
        );
    }
}

#[test]
fn the_readme_gives_the_alias_the_help_gives() {
    // The repository documents this tool two times, and a reader takes the
    // alias out of whichever one is in front of them. The README leaves the
    // `alias` word out, because the sentence around it already says the line is
    // an alias, so the test reads the rest of the line.
    let (_idiom, alias) = invocations(&help_page(), BASH_LABEL);
    let body = alias
        .strip_prefix(ALIAS)
        .expect("the alias line opens with the alias word");

    let readme = readme();
    let named: Vec<&str> = readme
        .lines()
        .filter(|line| line.contains(ALIAS_NAME))
        .collect();

    // A README that names the alias no times, or more than one time, stops the
    // test. The assertion below would otherwise pass over a line that is gone.
    assert_eq!(named.len(), 1, "{README} names {ALIAS_NAME} one time");
    assert!(named[0].contains(body), "{README} says {:?}", named[0]);
}

#[test]
fn the_fish_lines_collect_the_substitution() {
    // Fish splits a substitution at every newline, and a directory name can
    // hold one. `string collect` makes the whole output one word again.
    //
    // The text is all this test reads, because fish is not on the machine that
    // runs these tests. The shell at SHELL is the only shell any test here
    // starts.
    let help = help_page();
    let (idiom, alias) = invocations(&help, FISH_LABEL);

    for line in [&idiom, &alias] {
        assert!(line.contains(COLLECT), "the fish line is {line:?}");
    }
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
