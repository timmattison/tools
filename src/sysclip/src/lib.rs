//! The clipboard of a tool.

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
    pub(crate) fn new(cause: &impl fmt::Display) -> Self {
        Self(cause.to_string())
    }
}

/// The clipboard.
pub struct Clipboard(route::Router<native::Arboard>);

impl Clipboard {
    /// Opens the clipboard by the order rule.
    ///
    /// # Errors
    ///
    /// Gives [`ClipboardError`] when no clipboard can be reached.
    pub fn open() -> Result<Self, ClipboardError> {
        route::Router::open(route::Environment::from_process(), native::Arboard::open).map(Self)
    }

    /// Opens the native clipboard only.
    ///
    /// # Errors
    ///
    /// Gives [`ClipboardError`] when the native clipboard cannot be opened.
    pub fn open_native() -> Result<Self, ClipboardError> {
        Ok(Self(route::Router::native_only(native::Arboard::open()?)))
    }

    /// The text on the clipboard.
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
        assert_eq!(ClipboardError::new(&"  spaced out \n").to_string(), "spaced out");
    }

    #[test]
    fn error_new_strips_one_trailing_period() {
        assert_eq!(ClipboardError::new(&"a sentence.").to_string(), "a sentence");
        assert_eq!(ClipboardError::new(&"dots..").to_string(), "dots.");
    }

    #[test]
    fn error_new_keeps_a_lone_period() {
        assert_eq!(ClipboardError::new(&" . ").to_string(), ".");
    }
}
