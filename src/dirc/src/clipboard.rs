//! The clipboard, and the one seam that keeps the tests off it.
//!
//! The clipboard is one shared resource of the whole machine. A test that reads
//! it races the person at the keyboard, and a test that writes it destroys what
//! that person copied. Two tests that write it at the same time destroy each
//! other. So [`Clipboard`] is a trait, [`SystemClipboard`] is the clipboard of
//! the machine, and [`FileClipboard`] is one file that stands in for it. A test
//! points [`open_named`] at a file of its own and touches nothing the machine
//! shares.
//!
//! # What a write of the clipboard leaves behind
//!
//! [`SystemClipboard::write`] calls `set_text` and nothing more. On X11 the
//! content of the clipboard belongs to the process that owns the selection, so
//! a tool that writes the clipboard and exits at once loses what it wrote.
//! `arboard` answers that with `SetExtLinux::wait()`, which holds the selection
//! until another program takes it. That call blocks, and a `dirc` that blocks
//! never gives the shell its prompt back. `clipboard-random` and `clipboardmon`
//! are the other tools of this workspace that write the clipboard, and both
//! make the same trade: the plain call, and a clipboard that a bare X11 session
//! can drop. A desktop with a clipboard manager keeps the text, because the
//! manager takes the selection.

use std::ffi::OsStr;
use std::fmt;
use std::path::PathBuf;

/// The variable that names a file to use in place of the clipboard of the
/// machine.
pub const CLIPBOARD_FILE_ENV: &str = "DIRC_CLIPBOARD_FILE";

/// The clipboard could not be reached.
///
/// The cause is kept as text and not as the error that made it. That error is a
/// different type on every platform, a machine with no display has no clipboard
/// to name a type of, and the file clipboard fails with an error of the file
/// system. The reader wants to know why the clipboard could not be reached, not
/// which crate said so.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("Failed to reach the clipboard: {0}")]
pub struct ClipboardError(String);

/// The clipboard, as `dirc` uses it.
pub trait Clipboard {
    /// The text the clipboard holds. A clipboard that holds nothing gives an
    /// empty string.
    ///
    /// # Errors
    ///
    /// Gives [`ClipboardError`] when the clipboard cannot be reached or holds
    /// something that is not text.
    fn read(&mut self) -> Result<String, ClipboardError>;

    /// Puts `text` on the clipboard.
    ///
    /// # Errors
    ///
    /// Gives [`ClipboardError`] when the clipboard cannot be written.
    fn write(&mut self, text: &str) -> Result<(), ClipboardError>;
}

/// The clipboard of the machine.
pub struct SystemClipboard(arboard::Clipboard);

/// A clipboard that is one file.
///
/// The file holds the text a write put there, and a read gives it back. This is
/// what makes a test of `dirc` hermetic: the file belongs to the one test that
/// made it, so no other test and no person at the keyboard sees it.
pub struct FileClipboard {
    /// The file that stands in for the clipboard.
    path: PathBuf,
}

impl ClipboardError {
    /// The failure `cause` names.
    ///
    /// The space around the cause is dropped, and so is a period at the end of
    /// it. `arboard` writes a whole sentence with a period on it, and every
    /// other message of `dirc` ends without one. A cause that is nothing but a
    /// period is kept as it is, because an error that names no cause at all
    /// reads as a bug in the tool rather than as a state of the machine.
    fn new(cause: &impl fmt::Display) -> Self {
        let written = cause.to_string();
        let clause = written.trim();
        let shortened = clause.strip_suffix('.').unwrap_or(clause);
        Self(
            if shortened.is_empty() {
                clause
            } else {
                shortened
            }
            .to_string(),
        )
    }
}

impl SystemClipboard {
    /// Opens the clipboard of the machine.
    ///
    /// The clipboard is opened once and kept. The Go tool calls
    /// `clipboard.Init()` before it does anything and stops when that fails, so
    /// a `dirc` that cannot reach the clipboard says so before it does any
    /// work.
    ///
    /// # Errors
    ///
    /// Gives [`ClipboardError`] when the clipboard cannot be opened. A machine
    /// with no display is such a machine, and so is a session over SSH.
    pub fn new() -> Result<Self, ClipboardError> {
        arboard::Clipboard::new()
            .map(Self)
            .map_err(|cause| ClipboardError::new(&cause))
    }
}

impl Clipboard for SystemClipboard {
    fn read(&mut self) -> Result<String, ClipboardError> {
        from_arboard(self.0.get_text())
    }

    /// Puts `text` on the clipboard of the machine.
    ///
    /// The plain `set_text`, with no `SetExtLinux`. The module comment says
    /// what that costs on X11 and why the other choice costs more.
    fn write(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.0
            .set_text(text)
            .map_err(|cause| ClipboardError::new(&cause))
    }
}

impl FileClipboard {
    /// A clipboard that is the file at `path`.
    ///
    /// The file does not have to be there. A file that is not there is a
    /// clipboard that holds nothing, which is what a clipboard that was never
    /// written holds.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The file, named for a message.
    ///
    /// `Debug` and not `Display`, because a path that is not text loses bytes
    /// to `Display` and keeps them as escapes under `Debug`. The reader of the
    /// message set this path, so the name is what tells them which file the
    /// failure was about.
    fn named(&self) -> String {
        format!("{:?}", self.path)
    }
}

impl Clipboard for FileClipboard {
    fn read(&mut self) -> Result<String, ClipboardError> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Ok(text),
            // A clipboard that was never written holds nothing, and a file
            // that is not there is that clipboard. Every other failure is a
            // clipboard that could not be reached, the file of bytes that are
            // not text included.
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(cause) => Err(ClipboardError::new(&format!("{}: {cause}", self.named()))),
        }
    }

    fn write(&mut self, text: &str) -> Result<(), ClipboardError> {
        std::fs::write(&self.path, text)
            .map_err(|cause| ClipboardError::new(&format!("{}: {cause}", self.named())))
    }
}

/// The clipboard that `value` names.
///
/// `value` is the value of [`CLIPBOARD_FILE_ENV`]. The caller reads the
/// environment, so a test of this function touches no process-global state.
///
/// # Errors
///
/// Gives [`ClipboardError`] when `value` names no file and the clipboard of the
/// machine cannot be opened.
pub fn open_named(value: Option<&OsStr>) -> Result<Box<dyn Clipboard>, ClipboardError> {
    match named_file(value) {
        Some(path) => Ok(Box::new(FileClipboard::new(path))),
        None => Ok(Box::new(SystemClipboard::new()?)),
    }
}

/// The clipboard the environment names.
///
/// # Errors
///
/// Gives [`ClipboardError`] when the environment names no file and the
/// clipboard of the machine cannot be opened.
pub fn open() -> Result<Box<dyn Clipboard>, ClipboardError> {
    // `var_os` and not `var`. A Unix kernel takes a path that no Rust string
    // holds, and `var` gives nothing back for such a value. The tool then opens
    // the clipboard of the machine, which is the one outcome this variable
    // exists to prevent: the caller names a file, and the shared clipboard gets
    // the work.
    let named = std::env::var_os(CLIPBOARD_FILE_ENV);
    open_named(named.as_deref())
}

/// The file `value` names, or `None` when it names none.
///
/// A value that is empty, or that holds only whitespace, names no file. An
/// exported but empty variable is a common accident, and the clipboard of the
/// machine is the friendlier answer to it.
///
/// The value comes back as it was written, and not trimmed. A file name can end
/// with a space, so the whitespace is read as an answer to one question only:
/// did the person name a file at all.
///
/// `OsStr` has no `trim`, so the test for whitespace reads the lossy text of
/// the value. A value that is not text becomes replacement characters there,
/// and a replacement character is not whitespace, so such a value names a file.
/// Only the test reads that text. The value itself goes on byte for byte,
/// because a Unix kernel takes a path that no Rust string holds.
fn named_file(value: Option<&OsStr>) -> Option<&OsStr> {
    value.filter(|named| !named.to_string_lossy().trim().is_empty())
}

/// The read `result` as text.
///
/// Pure, so the mapping is tested without a clipboard. The one judgment in it
/// is `ContentNotAvailable`: `arboard` says that for a clipboard with nothing
/// in it and for a clipboard that holds an image, and neither of them is a
/// failure to a tool that wants a path.
///
/// # Errors
///
/// Gives [`ClipboardError`] for every other failure.
fn from_arboard(result: Result<String, arboard::Error>) -> Result<String, ClipboardError> {
    match result {
        Ok(text) => Ok(text),
        Err(arboard::Error::ContentNotAvailable) => Ok(String::new()),
        // `arboard::Error` is `#[non_exhaustive]`, so this arm is required and
        // a new variant of a later version arrives here as a failure that
        // names itself, rather than as a build that stops.
        Err(cause) => Err(ClipboardError::new(&cause)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path `dirc` moves in these tests.
    const COPIED: &str = "/tmp/somewhere";

    /// A new temporary directory, and the path of a clipboard file in it.
    ///
    /// The file is not made. The caller makes it, or reads a clipboard that has
    /// no file behind it. `tempfile` names the directory after the process and
    /// a random word, which is what keeps two runs of these tests at the same
    /// time out of each other's way.
    fn clipboard_file() -> (tempfile::TempDir, PathBuf) {
        let base = tempfile::tempdir().expect("a temporary directory");
        let path = base.path().join("clipboard.txt");
        (base, path)
    }

    #[test]
    fn a_file_clipboard_gives_back_what_it_was_written() {
        let (_base, path) = clipboard_file();
        let mut clipboard = FileClipboard::new(&path);
        clipboard.write(COPIED).expect("the file is written");
        assert_eq!(clipboard.read().expect("the file is read"), COPIED);
    }

    #[test]
    fn a_file_clipboard_with_no_file_holds_nothing() {
        // A clipboard that was never written holds nothing, and a file that is
        // not there is that clipboard.
        let (_base, path) = clipboard_file();
        let mut clipboard = FileClipboard::new(&path);
        assert_eq!(clipboard.read().expect("the file is read"), "");
    }

    #[test]
    fn a_write_replaces_what_the_file_held() {
        let (_base, path) = clipboard_file();
        let mut clipboard = FileClipboard::new(&path);
        clipboard
            .write("/tmp/the-first-directory")
            .expect("the file is written");
        clipboard.write(COPIED).expect("the file is written again");
        assert_eq!(clipboard.read().expect("the file is read"), COPIED);
    }

    #[test]
    fn a_file_of_bytes_that_are_not_text_cannot_be_read() {
        // The clipboard holds text. A file that holds something else is a
        // clipboard that cannot be read, and not an empty one.
        let (_base, path) = clipboard_file();
        std::fs::write(&path, [0xff, 0xfe]).expect("the file is made");
        let mut clipboard = FileClipboard::new(&path);
        let message = clipboard
            .read()
            .expect_err("the file holds no text")
            .to_string();
        assert!(
            message.starts_with("Failed to reach the clipboard: "),
            "{message}"
        );
        assert!(
            message.contains(path.to_str().expect("a temporary path is UTF-8")),
            "{message}"
        );
    }

    #[test]
    fn a_named_file_is_the_clipboard() {
        let (_base, path) = clipboard_file();
        std::fs::write(&path, COPIED).expect("the file is made");
        let mut clipboard = open_named(Some(path.as_os_str())).expect("a file clipboard opens");
        assert_eq!(clipboard.read().expect("the file is read"), COPIED);
    }

    #[test]
    fn a_value_that_names_no_file_falls_back_to_the_machine() {
        // The assertion is on the decision and not on the clipboard it opens.
        // Opening the other answer would reach the clipboard of the machine,
        // and no test of this workspace does that.
        assert_eq!(named_file(None), None);
        assert_eq!(named_file(Some(OsStr::new(""))), None);
        assert_eq!(named_file(Some(OsStr::new("   "))), None);
        assert_eq!(named_file(Some(OsStr::new(" \t\n "))), None);
    }

    #[test]
    fn a_value_with_a_path_in_it_names_that_file() {
        let (_base, path) = clipboard_file();
        let written = path.as_os_str();
        assert_eq!(named_file(Some(written)), Some(written));
    }

    #[cfg(unix)]
    #[test]
    fn a_value_that_is_not_text_names_the_file_it_holds() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        // 0xFF is not a byte of any UTF-8 sequence, so this is a path a Unix
        // kernel accepts and a Rust string cannot hold. The value names a file
        // like any other value does, and it comes back byte for byte. A tool
        // that dropped it here opens the clipboard of the machine instead.
        let not_text = OsString::from_vec(vec![b'/', 0xff, b'x']);
        assert_eq!(named_file(Some(&not_text)), Some(not_text.as_os_str()));
    }

    #[test]
    fn text_from_the_clipboard_arrives_whole() {
        assert_eq!(from_arboard(Ok(COPIED.to_string())), Ok(COPIED.to_string()));
    }

    #[test]
    fn a_clipboard_with_no_content_holds_nothing() {
        assert_eq!(
            from_arboard(Err(arboard::Error::ContentNotAvailable)),
            Ok(String::new())
        );
    }

    #[test]
    fn a_platform_with_no_clipboard_cannot_be_read() {
        let message = from_arboard(Err(arboard::Error::ClipboardNotSupported))
            .expect_err("the platform has no clipboard")
            .to_string();
        assert!(message.contains("not supported"), "{message}");
        // `arboard` writes a whole sentence, period included. Every other
        // message of `dirc` ends without one.
        assert!(!message.ends_with('.'), "{message}");
    }

    #[test]
    fn an_unknown_failure_carries_its_description() {
        let message = from_arboard(Err(arboard::Error::Unknown {
            description: "the window server said no".to_string(),
        }))
        .expect_err("the read failed")
        .to_string();
        // `arboard` writes its own sentence around the description, so the
        // test pins the description and the shape of the message, and not the
        // wording `arboard` puts between them.
        assert!(
            message.starts_with("Failed to reach the clipboard: "),
            "{message}"
        );
        assert!(message.contains("the window server said no"), "{message}");
    }
}
