//! Paste mode: the `cd` line for the directory the clipboard names.
//!
//! A shell runs this line, so every character of the path is shell syntax until
//! it is quoted. A single-quoted word is the strongest quote a POSIX shell has,
//! because it takes every character literally, the dollar sign and the
//! backslash included. The one character it cannot hold is the single quote
//! itself, so [`escape_single_quotes`] closes the word, gives the shell a
//! backslash-escaped quote, and opens the word again.
//!
//! The checks come before the line, not after it. A path that is not there, or
//! that names a file, gets a message that names the path and names `dirc`. An
//! unchecked `cd` gets the message of the shell instead, and the reader must
//! then find which of the two programs said it.
//!
//! Nothing here reads the clipboard. The text of the clipboard comes in as an
//! argument, so the whole of paste mode is tested without one. A test that
//! reads the real clipboard races the person at the keyboard, and a test that
//! writes it destroys what that person copied.

use std::path::Path;

/// The cause [`PasteError::Absolute`] names when the absolute path is not text.
///
/// The clipboard holds text, so a path that comes out of it is UTF-8. The
/// directory of the process is not text, and `std::path::absolute` puts that
/// directory in front of a relative path. The result can thus hold bytes that
/// are not UTF-8. A `cd` line built from replacement characters points at a
/// different directory, so such a path is refused instead.
const NOT_UTF8: &str = "the absolute path is not valid UTF-8";

/// What paste mode could not do.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum PasteError {
    /// The clipboard holds nothing at all.
    #[error("Clipboard is empty")]
    Empty,
    /// The clipboard holds whitespace and nothing else.
    #[error("Clipboard contains only whitespace")]
    OnlyWhitespace,
    /// The path could not be read. It is not there, or it cannot be reached.
    #[error("Invalid directory path in clipboard: {path} ({cause})")]
    InvalidPath {
        /// The path, with the whitespace around it dropped.
        path: String,
        /// What the operating system said.
        cause: String,
    },
    /// The path names a file, a socket, or another thing that is not a
    /// directory.
    #[error("Path in clipboard is not a directory: {path}")]
    NotADirectory {
        /// The path, with the whitespace around it dropped.
        path: String,
    },
    /// The path could not be made absolute.
    #[error("Failed to resolve an absolute path: {path} ({cause})")]
    Absolute {
        /// The path, with the whitespace around it dropped.
        path: String,
        /// Why the absolute path could not be made or could not be written.
        cause: String,
    },
}

/// `path` with every single quote escaped for a single-quoted shell word.
///
/// Each `'` becomes `'\''`: the word closes, one backslash-escaped quote stands
/// on its own, and the word opens again. The caller puts the single quotes
/// around the result.
///
/// The replacement works on characters and never on bytes, so a path in any
/// language passes through whole.
#[must_use]
pub fn escape_single_quotes(path: &str) -> String {
    todo!("escape every single quote for a single-quoted shell word")
}

/// The `cd` line for the directory that `copied` names.
///
/// `copied` is the text of the clipboard. The whitespace around it is dropped,
/// because a path that was copied out of a document or out of a terminal
/// carries a newline with it more often than not.
///
/// The line carries no newline of its own. The caller prints it, and only the
/// caller knows what it prints to.
///
/// # Errors
///
/// Gives [`PasteError::Empty`] for a clipboard with nothing in it,
/// [`PasteError::OnlyWhitespace`] for a clipboard that holds only whitespace,
/// [`PasteError::InvalidPath`] when the path cannot be read,
/// [`PasteError::NotADirectory`] when the path names something that is not a
/// directory, and [`PasteError::Absolute`] when the path cannot be made
/// absolute.
pub fn cd_command(copied: &str) -> Result<String, PasteError> {
    todo!("give the cd line for the directory the clipboard names")
}

/// The `cd` line for `absolute`, the resolved form of `path`.
///
/// Split out of [`cd_command`] so that a test gives it a path that is not
/// UTF-8. The other way to make one is to move the process into a directory
/// whose name is not UTF-8, and the directory of a process is one piece of
/// state that every thread of that process shares.
///
/// # Errors
///
/// Gives [`PasteError::Absolute`] when `absolute` is not text.
fn cd_line(path: &str, absolute: &Path) -> Result<String, PasteError> {
    todo!("write the escaped absolute path into a cd line")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{PathBuf, MAIN_SEPARATOR};

    /// A new temporary directory that holds a directory named `name`.
    ///
    /// The temporary directory comes back with the child, and it removes itself
    /// when it is dropped, so the caller holds it for as long as it reads the
    /// path. `tempfile` names it after the process and a random word, which is
    /// what keeps two runs of these tests at the same time out of each other's
    /// way.
    fn directory_named(name: &str) -> (tempfile::TempDir, PathBuf) {
        let base = tempfile::tempdir().expect("a temporary directory");
        let child = base.path().join(name);
        std::fs::create_dir(&child).expect("the directory is made");
        (base, child)
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

    #[test]
    fn a_path_with_no_quote_passes_through() {
        assert_eq!(escape_single_quotes("/tmp/plain"), "/tmp/plain");
        assert_eq!(escape_single_quotes(""), "");
    }

    #[test]
    fn one_quote_closes_the_word_and_opens_it_again() {
        assert_eq!(escape_single_quotes("/tmp/it's"), r"/tmp/it'\''s");
    }

    #[test]
    fn every_quote_in_the_path_is_escaped() {
        assert_eq!(escape_single_quotes("'a'b'"), r"'\''a'\''b'\''");
        assert_eq!(escape_single_quotes("''"), r"'\'''\''");
    }

    #[test]
    fn a_space_is_left_to_the_quotes_around_the_word() {
        // The word is single-quoted, so the shell already reads a space as part
        // of the word. Only the quote itself needs the escape.
        assert_eq!(escape_single_quotes("/tmp/two words"), "/tmp/two words");
    }

    #[test]
    fn characters_of_any_language_pass_through_whole() {
        assert_eq!(escape_single_quotes("日本語"), "日本語");
        assert_eq!(escape_single_quotes("café"), "café");
        assert_eq!(escape_single_quotes("🎉"), "🎉");
        assert_eq!(
            escape_single_quotes("日本語/it's/café/🎉"),
            r"日本語/it'\''s/café/🎉"
        );
    }

    #[test]
    fn a_directory_whose_name_holds_a_quote_is_quoted_for_the_shell() {
        let (base, child) = directory_named("it's here");
        let line = cd_command(text(&child)).expect("the path names a directory");
        assert_eq!(
            line,
            format!("cd '{}{MAIN_SEPARATOR}it'\\''s here'", quoteless(&base))
        );
    }

    #[test]
    fn a_directory_whose_name_holds_a_space_is_quoted_for_the_shell() {
        let (base, child) = directory_named("two words");
        let line = cd_command(text(&child)).expect("the path names a directory");
        assert_eq!(
            line,
            format!("cd '{}{MAIN_SEPARATOR}two words'", quoteless(&base))
        );
    }

    #[test]
    fn a_directory_whose_name_holds_multi_byte_characters_is_quoted_for_the_shell() {
        let (base, child) = directory_named("日本語 café 🎉");
        let line = cd_command(text(&child)).expect("the path names a directory");
        assert_eq!(
            line,
            format!("cd '{}{MAIN_SEPARATOR}日本語 café 🎉'", quoteless(&base))
        );
    }

    #[test]
    fn the_line_carries_no_newline() {
        // The caller prints the newline. A line that carried its own would give
        // a blank line to every reader that adds one.
        let (_base, child) = directory_named("plain");
        let line = cd_command(text(&child)).expect("the path names a directory");
        assert!(!line.ends_with('\n'), "{line}");
    }

    #[test]
    fn the_whitespace_around_the_path_is_dropped() {
        // A path copied out of a terminal or out of a document carries a
        // newline with it more often than not.
        let (base, child) = directory_named("plain");
        let copied = format!(" \t\n{}\r\n ", text(&child));
        let line = cd_command(&copied).expect("the path names a directory");
        assert_eq!(
            line,
            format!("cd '{}{MAIN_SEPARATOR}plain'", quoteless(&base))
        );
    }

    #[test]
    fn an_empty_clipboard_is_refused() {
        let err = cd_command("").expect_err("the clipboard holds nothing");
        assert_eq!(err, PasteError::Empty);
        assert_eq!(err.to_string(), "Clipboard is empty");
    }

    #[test]
    fn a_clipboard_of_whitespace_is_refused() {
        let err = cd_command(" \t\r\n ").expect_err("the clipboard holds no path");
        assert_eq!(err, PasteError::OnlyWhitespace);
        assert_eq!(err.to_string(), "Clipboard contains only whitespace");
    }

    #[test]
    fn a_path_that_is_not_there_is_refused() {
        let base = tempfile::tempdir().expect("a temporary directory");
        let missing = base.path().join("no-such-directory");
        let missing_text = text(&missing);
        let err = cd_command(missing_text).expect_err("the path is not there");
        assert!(
            matches!(err, PasteError::InvalidPath { .. }),
            "{err:?}"
        );
        // The cause is the text of the operating system, so the test pins the
        // shape of the message and the path in it, and not the cause.
        let message = err.to_string();
        assert!(
            message.starts_with(&format!(
                "Invalid directory path in clipboard: {missing_text} ("
            )),
            "{message}"
        );
        assert!(message.ends_with(')'), "{message}");
    }

    #[test]
    fn a_path_that_names_a_file_is_refused() {
        let base = tempfile::tempdir().expect("a temporary directory");
        let file = base.path().join("not-a-directory");
        std::fs::write(&file, b"").expect("the file is made");
        let file_text = text(&file);
        let err = cd_command(file_text).expect_err("the path names a file");
        assert_eq!(
            err,
            PasteError::NotADirectory {
                path: file_text.to_string()
            }
        );
        assert_eq!(
            err.to_string(),
            format!("Path in clipboard is not a directory: {file_text}")
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_absolute_path_that_is_not_text_is_refused() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        // 0xFF is not a byte of any UTF-8 sequence, so this is a path a Unix
        // kernel accepts and a Rust string cannot hold.
        let absolute = PathBuf::from(OsString::from_vec(vec![b'/', 0xff, b'x']));
        let err = cd_line("here", &absolute).expect_err("the absolute path is not text");
        assert_eq!(
            err,
            PasteError::Absolute {
                path: "here".to_string(),
                cause: NOT_UTF8.to_string()
            }
        );
        assert_eq!(
            err.to_string(),
            format!("Failed to resolve an absolute path: here ({NOT_UTF8})")
        );
    }
}
