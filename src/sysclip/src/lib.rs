//! The clipboard of a tool, for a machine and for a mosh-rs session.
//!
//! # Why this crate exists
//!
//! The mosh-rs server puts a `pbcopy` shim and a `pbpaste` shim on the `PATH`
//! of a session. It also sets `MOSH_OPEN_SOCKET`. Each shim sends the text to
//! the clipboard of the client computer, where the person sits.
//!
//! `arboard` does not know the shims. It opens the clipboard of the server
//! instead. On a macOS server with no GUI session, that fails with "The
//! selected clipboard is not supported with the current system configuration".
//! On a headless Linux server, it waits for an X11 display and times out. On a
//! Mac server with a GUI login, it works, but it writes the pasteboard of the
//! wrong machine.
//!
//! # The order rule
//!
//! 1. In a mosh session, `MOSH_OPEN_SOCKET` is set and it is not empty. The
//!    native clipboard never opens. Each read runs `pbpaste` and each write
//!    runs `pbcopy`, both found on `PATH`. A failure of the command is the
//!    answer. [`Clipboard::open`] fails when `PATH` holds neither command.
//! 2. Outside a mosh session, the native clipboard goes first. When it fails,
//!    or when it does not open, and `PATH` holds the command, the command is
//!    the fallback. When both fail, the message names both causes. When `PATH`
//!    holds no command, the error of the native clipboard is the error.
//!
//! A `PATH` entry that is not an absolute path is skipped. A shell reads an
//! empty entry as the current directory, and this crate must not run
//! `./pbcopy`.
//!
//! # Why [`Clipboard::open_native`] exists
//!
//! A tool that polls the clipboard, such as `clipboardmon`, reads it each 500
//! ms. The mosh-rs client allows 5 pastes in 10 seconds, and each paste shows
//! a notification bar. A poll through `pbpaste` would hit that limit at once.
//! [`Clipboard::open_native`] uses the native clipboard only and never runs a
//! command.
//!
//! # What a write leaves behind
//!
//! A write calls `set_text` and nothing more. On X11 the content of the
//! clipboard belongs to the process that owns the selection. A tool that
//! writes the clipboard and exits at once loses what it wrote. `arboard`
//! answers that with `SetExtLinux::wait()`, which holds the selection until
//! another program takes it. That call blocks, and a tool that blocks never
//! gives the shell its prompt back. This crate makes the trade of the other
//! tools of this workspace: the plain call, and a clipboard that a bare X11
//! session can drop. A desktop with a clipboard manager keeps the text,
//! because the manager takes the selection.
//!
//! # Example
//!
//! ```no_run
//! let mut clipboard = sysclip::Clipboard::open()?;
//! clipboard.write("hello")?;
//! assert_eq!(clipboard.read()?, "hello");
//! # Ok::<(), sysclip::ClipboardError>(())
//! ```

use std::fmt;

mod command;
mod native;
mod route;

/// The variable that the mosh-rs server sets in a session.
pub const MOSH_OPEN_SOCKET_ENV: &str = "MOSH_OPEN_SOCKET";
/// The command that copies text to the clipboard.
pub const COPY_COMMAND: &str = "pbcopy";
/// The command that pastes text from the clipboard.
pub const PASTE_COMMAND: &str = "pbpaste";

/// Holds a message, as text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ClipboardError(String);

impl ClipboardError {
    /// The failure that `cause` names.
    ///
    /// The space around the cause goes away, and so does one period at the end
    /// of it. `arboard` writes a whole sentence with a period on it, and this
    /// crate joins causes into longer messages. A cause that is only a period
    /// stays as it is, because an error with no cause at all reads as a bug in
    /// the tool.
    pub(crate) fn new(cause: &impl fmt::Display) -> Self {
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

/// The clipboard, with the order rule of the crate.
///
/// The crate documentation holds the rule. Nothing of `arboard` is public.
pub struct Clipboard(route::Router<native::Arboard>);

impl Clipboard {
    /// Opens the clipboard by the order rule, from the environment of this
    /// process. It reads `MOSH_OPEN_SOCKET` and `PATH`.
    ///
    /// # Errors
    ///
    /// Gives [`ClipboardError`] when no clipboard can be reached.
    pub fn open() -> Result<Self, ClipboardError> {
        route::Router::open(route::Environment::from_process(), native::Arboard::open).map(Self)
    }

    /// Opens the native clipboard only. No read or write runs a command.
    ///
    /// Use it for a tool that polls the clipboard.
    ///
    /// # Errors
    ///
    /// Gives [`ClipboardError`] when the native clipboard cannot be opened.
    pub fn open_native() -> Result<Self, ClipboardError> {
        Ok(Self(route::Router::native_only(native::Arboard::open()?)))
    }

    /// The text on the clipboard. An empty string means the clipboard holds no
    /// text.
    ///
    /// # Errors
    ///
    /// Gives [`ClipboardError`] when the clipboard cannot be read.
    pub fn read(&mut self) -> Result<String, ClipboardError> {
        self.0.read()
    }

    /// Puts `text` on the clipboard.
    ///
    /// # Errors
    ///
    /// Gives [`ClipboardError`] when the clipboard cannot be written.
    pub fn write(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.0.write(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_new_trims_the_cause() {
        assert_eq!(
            ClipboardError::new(&"  spaced out \n").to_string(),
            "spaced out"
        );
    }

    #[test]
    fn error_new_strips_one_trailing_period() {
        assert_eq!(
            ClipboardError::new(&"a sentence.").to_string(),
            "a sentence"
        );
        assert_eq!(ClipboardError::new(&"dots..").to_string(), "dots.");
    }

    #[test]
    fn error_new_keeps_a_lone_period() {
        assert_eq!(ClipboardError::new(&" . ").to_string(), ".");
    }
}
