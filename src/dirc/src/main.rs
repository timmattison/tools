//! Directory Clipboard.
//!
//! The command line, and the two modes it dispatches to. The path work is in
//! [`dirc::mode`], and the clipboard is in [`dirc::clipboard`]. What is left
//! lives here: the flag, the output, and the status the run gives back.
//!
//! Every failure writes one line to standard error and gives status 1. The Go
//! tool ends each of its failures with `logger.Fatal`, which exits 1 as well,
//! so a shell that reads the status of `dirc` gets the same answer from both.
//!
//! Each mode writes to a writer the caller gives it, and never with `println!`.
//! A test then reads what a mode wrote without starting a process, and standard
//! output is locked one time for the whole run rather than one time for each
//! line.

use buildinfo::version_string;
use clap::Parser;
use dirc::clipboard::{self, Clipboard};
use dirc::mode;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

/// What `dirc` does, as `-h` and `--help` both print it.
///
/// This text says two things the usage of the Go tool does not, and both
/// changes are deliberate.
///
/// The Go usage names a `-copy` mode. No `-copy` flag is there, in that tool or
/// in this one: copying is what happens when `--paste` is absent. The Go usage
/// also names the two modes in the order that makes paste mode look like the
/// default, and it is not. This port corrects the text and adds no flag,
/// because a `--copy` flag gives a second spelling for the behavior a user
/// already gets by typing nothing.
///
/// The whole text is the `about`, and no `long_about` is set, so `-h` and
/// `--help` print the same thing. A short help that says less hides the two
/// lines that tell a reader how to run the tool at all.
const ABOUT: &str = "\
Directory Clipboard.

A tool that can either:
1. Copy the current working directory to the clipboard (the default)
2. Give a 'cd' command for the directory path in the clipboard (--paste)";

/// How to run `dirc`, printed under the options.
///
/// Every invocation here spells the flag `--paste`, with two dashes. The Go
/// `flag` package takes `-paste` and `--paste` for one flag, and `clap` reads
/// `-paste` as five short flags and refuses it. So the single-dash spelling of
/// the Go tool stops working, and a user who kept the old alias reads the
/// refusal of `clap` through `eval`. The text gives the spelling that works.
///
/// Every substitution here stays one word, and each shell needs a different
/// thing for that. Bash and Zsh split the output of an unquoted `$(...)` at
/// every character the field separator holds, so a directory name that holds a
/// tab or a newline reaches `cd` with that character turned into a space. The
/// double quotes stop the split. Fish splits a substitution at every newline
/// alone, and `string collect` puts the output back together. A page that
/// taught either form without this undoes the quoting
/// [`dirc::mode::cd_command`] does.
///
/// The last paragraph names [`dirc::clipboard::CLIPBOARD_FILE_ENV`]. That
/// variable is what makes an end-to-end test of this tool hermetic, and a user
/// who reads the help is told the tool has such a seam.
const AFTER_HELP: &str = "\
NOTE: This tool cannot directly change your shell's directory.
To use it effectively, you need to evaluate its output in your shell:

  Bash/Zsh: eval \"$(dirc --paste)\"
  Fish:     eval (dirc --paste | string collect)

TIP: Add this alias to your shell config:
  Bash/Zsh: alias dirp='eval \"$(dirc --paste)\"'
  Fish:     alias dirp='eval (dirc --paste | string collect)'

DIRC_CLIPBOARD_FILE names a file to read and write in place of the clipboard
of the machine.";

/// The command line of `dirc`: one flag, because there are two modes and copy
/// mode is the default, so a run with no arguments copies.
//
// The documentation comment above stays one paragraph, and this note is a plain
// comment so that it stays out of it. `clap` derives `about` from the first
// paragraph of a documentation comment and `long_about` from the whole of it. A
// derived `long_about` wins over ABOUT in `--help`, while `-h` keeps ABOUT, so
// a second paragraph here makes the longer help say less than the short one.
// The test `the_short_help_and_the_long_help_say_the_same_thing` fails the day
// one arrives.
#[derive(Debug, Parser)]
#[command(name = "dirc", version = version_string!(), about = ABOUT, after_help = AFTER_HELP)]
struct Cli {
    /// Give a 'cd' command for the directory path in the clipboard
    #[arg(long)]
    paste: bool,
}

/// What a run could not do.
#[derive(Debug, thiserror::Error)]
enum RunError {
    /// The clipboard could not be reached, read, or written.
    #[error(transparent)]
    Clipboard(#[from] dirc::clipboard::ClipboardError),
    /// Copy mode could not make the path of the shell absolute.
    #[error(transparent)]
    Copy(#[from] dirc::mode::CopyError),
    /// Paste mode refused the text the clipboard holds.
    #[error(transparent)]
    Paste(#[from] dirc::mode::PasteError),
    /// The directory of the shell could not be read.
    #[error("Failed to get the current directory: {0}")]
    CurrentDirectory(#[source] std::io::Error),
    /// The output could not be written. A closed pipe is the usual cause.
    #[error("Failed to write to standard output: {0}")]
    Output(#[source] std::io::Error),
}

/// Copy mode: puts the absolute current directory on the clipboard.
///
/// The path is made absolute before it is written, because the shell that reads
/// the clipboard is another shell in another directory. The line that names the
/// path goes to `out` after the write, so a reader who sees that line knows the
/// clipboard already holds the path.
///
/// # Errors
///
/// Gives [`RunError::Copy`] when the path cannot be made absolute,
/// [`RunError::Clipboard`] when the clipboard cannot be written, and
/// [`RunError::Output`] when the line cannot be written.
fn copy(
    clipboard: &mut dyn Clipboard,
    current_dir: &Path,
    out: &mut dyn Write,
) -> Result<(), RunError> {
    let copied = mode::copied_path(current_dir)?;
    clipboard.write(&copied)?;
    writeln!(out, "Copied to clipboard: {copied}").map_err(RunError::Output)
}

/// Paste mode: writes the `cd` line for the directory the clipboard names.
///
/// [`dirc::mode::cd_command`] reads the text and quotes the path, so this
/// function adds the newline and nothing else. The newline belongs here because
/// only the caller knows what it writes to.
///
/// # Errors
///
/// Gives [`RunError::Clipboard`] when the clipboard cannot be read,
/// [`RunError::Paste`] when the clipboard names no directory this tool can go
/// to, and [`RunError::Output`] when the line cannot be written.
fn paste(clipboard: &mut dyn Clipboard, out: &mut dyn Write) -> Result<(), RunError> {
    let copied = clipboard.read()?;
    let line = mode::cd_command(&copied)?;
    writeln!(out, "{line}").map_err(RunError::Output)
}

/// The mode `cli` names, over the clipboard the environment names.
///
/// The directory of the shell is read in copy mode and nowhere else. Paste mode
/// does not need it, and a shell whose directory was removed can still paste an
/// absolute path. A read in both modes would take that away.
///
/// # Errors
///
/// Gives the failure of the mode it dispatched to, [`RunError::Clipboard`] when
/// the clipboard cannot be opened, and [`RunError::CurrentDirectory`] when copy
/// mode cannot read the directory of the shell.
fn run(cli: &Cli, out: &mut dyn Write) -> Result<(), RunError> {
    let mut clipboard = clipboard::open()?;

    if cli.paste {
        return paste(clipboard.as_mut(), out);
    }

    let current_dir = std::env::current_dir().map_err(RunError::CurrentDirectory)?;
    copy(clipboard.as_mut(), &current_dir, out)
}

/// Runs one mode and gives the status the shell reads.
///
/// Standard output is locked one time and handed to the mode. A run writes one
/// line, so the lock costs nothing, and the mode stays a function a test drives
/// over a `Vec<u8>`.
fn main() -> ExitCode {
    let cli = Cli::parse();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    match run(&cli, &mut out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("{failure}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use dirc::clipboard::FileClipboard;
    use dirc::mode::PasteError;
    use std::path::{PathBuf, MAIN_SEPARATOR};

    /// A new temporary directory, and a file clipboard that lives in it.
    ///
    /// The clipboard belongs to the one test that made it, so no test of this
    /// crate reads or writes the clipboard of the machine. `tempfile` names the
    /// directory after the process and a random word, which is what keeps two
    /// runs of these tests at the same time out of each other's way.
    ///
    /// The file behind the clipboard is not made. A file that is not there is a
    /// clipboard that holds nothing, which is the state a test of an empty
    /// clipboard wants.
    fn scratch() -> (tempfile::TempDir, FileClipboard) {
        let base = tempfile::tempdir().expect("a temporary directory");
        let clipboard = FileClipboard::new(base.path().join("clipboard.txt"));
        (base, clipboard)
    }

    /// A directory named `name`, made inside `base`.
    fn directory(base: &tempfile::TempDir, name: &str) -> PathBuf {
        let child = base.path().join(name);
        std::fs::create_dir(&child).expect("the directory is made");
        child
    }

    /// `path` as text.
    fn text(path: &Path) -> &str {
        path.to_str().expect("a temporary path is UTF-8")
    }

    /// The text of the temporary directory `base`, checked to hold no quote.
    ///
    /// Every test that writes an expected line by hand builds it on this text.
    /// A quote in it would be escaped by the code under test, and the test
    /// would then fail for a reason that has nothing to do with the code.
    fn quoteless(base: &tempfile::TempDir) -> &str {
        let written = text(base.path());
        assert!(
            !written.contains('\''),
            "the temporary path holds a quote: {written}"
        );
        written
    }

    /// What a mode wrote, as text.
    fn written(out: Vec<u8>) -> String {
        String::from_utf8(out).expect("the output is text")
    }

    #[test]
    fn copy_mode_puts_the_absolute_path_on_the_clipboard_and_names_it() {
        let (base, mut clipboard) = scratch();
        let child = directory(&base, "plain");
        let mut out = Vec::new();

        copy(&mut clipboard, &child, &mut out).expect("the directory is copied");

        assert_eq!(
            clipboard.read().expect("the clipboard is read"),
            text(&child)
        );
        assert_eq!(
            written(out),
            format!("Copied to clipboard: {}\n", text(&child))
        );
    }

    #[test]
    fn copy_mode_keeps_a_name_of_multi_byte_characters_whole() {
        // A path that lost a byte names a different directory, and the shell
        // that reads the clipboard then goes somewhere else or goes nowhere.
        let (base, mut clipboard) = scratch();
        let child = directory(&base, "日本語 café 🎉");
        let mut out = Vec::new();

        copy(&mut clipboard, &child, &mut out).expect("the directory is copied");

        assert_eq!(
            clipboard.read().expect("the clipboard is read"),
            text(&child)
        );
        assert!(!written(out).contains('\u{fffd}'));
    }

    #[test]
    fn paste_mode_writes_the_cd_line_and_nothing_else() {
        let (base, mut clipboard) = scratch();
        let child = directory(&base, "plain");
        clipboard
            .write(text(&child))
            .expect("the clipboard is written");
        let mut out = Vec::new();

        paste(&mut clipboard, &mut out).expect("the clipboard names a directory");

        assert_eq!(
            written(out),
            format!("cd '{}{MAIN_SEPARATOR}plain'\n", quoteless(&base))
        );
    }

    #[test]
    fn paste_mode_quotes_a_name_that_holds_a_quote() {
        // The shell runs this line, so a quote in the name is shell syntax
        // until the line escapes it.
        let (base, mut clipboard) = scratch();
        let child = directory(&base, "it's here");
        clipboard
            .write(text(&child))
            .expect("the clipboard is written");
        let mut out = Vec::new();

        paste(&mut clipboard, &mut out).expect("the clipboard names a directory");

        assert_eq!(
            written(out),
            format!("cd '{}{MAIN_SEPARATOR}it'\\''s here'\n", quoteless(&base))
        );
    }

    #[test]
    fn an_empty_clipboard_stops_paste_mode() {
        // The clipboard file was never written, which is the clipboard of a
        // session where nobody copied anything.
        let (_base, mut clipboard) = scratch();
        let mut out = Vec::new();

        let failure = paste(&mut clipboard, &mut out).expect_err("the clipboard holds nothing");

        assert!(
            matches!(failure, RunError::Paste(PasteError::Empty)),
            "{failure:?}"
        );
        assert_eq!(failure.to_string(), "Clipboard is empty");
        assert_eq!(written(out), "");
    }

    #[test]
    fn a_clipboard_of_whitespace_stops_paste_mode() {
        let (_base, mut clipboard) = scratch();
        clipboard
            .write(" \t\r\n ")
            .expect("the clipboard is written");
        let mut out = Vec::new();

        let failure = paste(&mut clipboard, &mut out).expect_err("the clipboard holds no path");

        assert!(
            matches!(failure, RunError::Paste(PasteError::OnlyWhitespace)),
            "{failure:?}"
        );
        assert_eq!(failure.to_string(), "Clipboard contains only whitespace");
        assert_eq!(written(out), "");
    }

    #[test]
    fn a_path_that_is_not_there_stops_paste_mode() {
        let (base, mut clipboard) = scratch();
        let missing = base.path().join("no-such-directory");
        clipboard
            .write(text(&missing))
            .expect("the clipboard is written");
        let mut out = Vec::new();

        let failure = paste(&mut clipboard, &mut out).expect_err("the path is not there");

        assert!(
            matches!(failure, RunError::Paste(PasteError::InvalidPath { .. })),
            "{failure:?}"
        );
        // The cause is the text of the operating system, so the assertion pins
        // the shape of the message and the path in it, and not the cause.
        let message = failure.to_string();
        assert!(
            message.starts_with("Invalid directory path in clipboard"),
            "{message}"
        );
        assert!(message.contains(text(&missing)), "{message}");
        assert_eq!(written(out), "");
    }

    #[test]
    fn a_path_that_names_a_file_stops_paste_mode() {
        let (base, mut clipboard) = scratch();
        let file = base.path().join("not-a-directory");
        std::fs::write(&file, b"").expect("the file is made");
        clipboard
            .write(text(&file))
            .expect("the clipboard is written");
        let mut out = Vec::new();

        let failure = paste(&mut clipboard, &mut out).expect_err("the path names a file");

        assert!(
            matches!(failure, RunError::Paste(PasteError::NotADirectory { .. })),
            "{failure:?}"
        );
        let message = failure.to_string();
        assert!(
            message.starts_with("Path in clipboard is not a directory"),
            "{message}"
        );
        assert!(message.contains(text(&file)), "{message}");
        assert_eq!(written(out), "");
    }

    #[test]
    fn the_command_line_takes_the_paste_flag() {
        // `try_parse_from` reads a list this test builds, so no process starts
        // and no clipboard is touched.
        let pasting = Cli::try_parse_from(["dirc", "--paste"]).expect("--paste is taken");
        assert!(pasting.paste);

        let copying = Cli::try_parse_from(["dirc"]).expect("no argument is taken");
        assert!(!copying.paste, "copy mode is the default");
    }

    #[test]
    fn the_single_dash_spelling_of_paste_is_refused() {
        // The Go `flag` package took `-paste`. `clap` reads it as five short
        // flags, so a user with the old alias gets a refusal. The help says
        // `--paste` everywhere for that reason.
        Cli::try_parse_from(["dirc", "-paste"]).expect_err("-paste is five short flags");
    }

    #[test]
    fn the_short_help_and_the_long_help_say_the_same_thing() {
        // `-h` and `--help` must both carry the whole text. A doc comment of
        // two paragraphs on `Cli` makes clap derive a `long_about` out of it,
        // and `--help` then prints that doc comment in place of ABOUT, while
        // `-h` keeps ABOUT. The longer help would say less than the short one,
        // and neither would name the two modes.
        let mut command = Cli::command();
        assert!(
            command.get_long_about().is_none(),
            "{:?}",
            command.get_long_about()
        );

        let short = command.render_help().to_string();
        let long = command.render_long_help().to_string();
        for help in [&short, &long] {
            assert!(help.contains(ABOUT), "{help}");
            assert!(help.contains(AFTER_HELP), "{help}");
        }
    }

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }
}
