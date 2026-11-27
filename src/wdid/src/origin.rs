//! The origin of a definition: the file that defined an alias or a function,
//! and the rule that says which definitions are mine.
//!
//! The shell gives each origin as text. [`Origin::from_trace`] reads the
//! `file:line` text that zsh gives for an alias. [`Origin::from_file`] reads
//! the file that zsh or bash gives for a function. [`Places`] holds the
//! directories that decide which definitions are mine. Each function of this
//! module is pure.

use std::path::PathBuf;

/// The file that defined an alias or a function, and the line of an alias.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub struct Origin {
    /// The absolute path of the file.
    pub file: PathBuf,
    /// The line of the definition in the file. A function has no line.
    pub line: Option<u32>,
}

impl Origin {
    /// Reads a trace in the form `file:line`.
    #[must_use]
    pub fn from_trace(_text: &str) -> Option<Self> {
        None
    }

    /// Reads the path of a file.
    #[must_use]
    pub fn from_file(_text: &str) -> Option<Self> {
        None
    }
}

/// Gives the path in `text`.
#[must_use]
pub fn place(_text: &str) -> Option<PathBuf> {
    None
}

/// The directories that decide which definitions are mine, as the shell gives
/// them after its start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Places {
    /// The home directory (`$HOME`).
    pub home: Option<PathBuf>,
    /// The oh-my-zsh directory (`$ZSH`).
    pub zsh: Option<PathBuf>,
    /// The custom directory of oh-my-zsh (`$ZSH_CUSTOM`).
    pub zsh_custom: Option<PathBuf>,
}

impl Places {
    /// Says if a definition with this origin is mine.
    #[must_use]
    pub fn is_mine(&self, _origin: Option<&Origin>) -> bool {
        false
    }

    /// Gives the text of an origin for a person.
    #[must_use]
    pub fn show(&self, _origin: &Origin) -> String {
        String::new()
    }
}
