//! The origin of a definition: the file that defined an alias or a function,
//! and the rule that says which definitions are mine.
//!
//! The shell gives each origin as text. [`Origin::from_trace`] reads the
//! `file:line` text that zsh gives for an alias. [`Origin::from_file`] reads
//! the file that zsh or bash gives for a function. [`Places`] holds the
//! directories that decide which definitions are mine. Each function of this
//! module is pure.

use std::path::{Path, PathBuf};

/// The character between the file and the line of a trace.
const LINE_SEPARATOR: char = ':';

/// The text that stands for the home directory in the text of an origin.
const HOME_MARK: &str = "~";

/// The file that defined an alias or a function, and the line of an alias.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub struct Origin {
    /// The absolute path of the file.
    pub file: PathBuf,
    /// The line of the definition in the file. A function has no line.
    pub line: Option<u32>,
}

impl Origin {
    /// Reads a trace in the form `file:line`, as `funcfiletrace` of zsh gives
    /// it.
    ///
    /// The line is the text after the last `:`, when that text is only ASCII
    /// digits. Else the whole text is the file, with no line. Gives `None` when
    /// the file is not an absolute path, for example for an empty text or for
    /// `(eval)`, because the location of such a file is not known.
    #[must_use]
    pub fn from_trace(text: &str) -> Option<Self> {
        let split = text.rsplit_once(LINE_SEPARATOR).and_then(|(file, line)| {
            let digits = !line.is_empty() && line.chars().all(|c| c.is_ascii_digit());
            digits
                .then(|| line.parse::<u32>().ok())
                .flatten()
                .map(|line| (file, line))
        });
        match split {
            Some((file, line)) => Self::at(file, Some(line)),
            None => Self::at(text, None),
        }
    }

    /// Reads the path of a file, as `functions_source` of zsh or `declare -F`
    /// of bash gives it. The whole text is the file, with no line.
    ///
    /// Gives `None` when the text is not an absolute path. bash gives
    /// `(null)` or `environment` for a function that no file defined.
    #[must_use]
    pub fn from_file(text: &str) -> Option<Self> {
        Self::at(text, None)
    }

    /// Makes an origin in `file`, or gives `None` when `file` is not an
    /// absolute path.
    fn at(file: &str, line: Option<u32>) -> Option<Self> {
        place(file).map(|file| Self { file, line })
    }
}

/// Gives the path in `text` when it is an absolute path. Gives `None` for an
/// empty text and for a relative path, because a relative path names a
/// different file in each directory.
#[must_use]
pub fn place(text: &str) -> Option<PathBuf> {
    let path = Path::new(text);
    path.is_absolute().then(|| path.to_path_buf())
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
    ///
    /// A definition is mine when its origin is unknown, when its file is under
    /// the custom directory, or when its file is under the home directory and
    /// not under the oh-my-zsh directory. An unknown origin is mine, so that a
    /// report of my definitions never hides it. "Under" compares whole path
    /// components, thus `/home/me2` is not under `/home/me`.
    #[must_use]
    pub fn is_mine(&self, origin: Option<&Origin>) -> bool {
        let Some(origin) = origin else {
            return true;
        };
        let under =
            |place: Option<&Path>| place.is_some_and(|place| origin.file.starts_with(place));
        under(self.zsh_custom.as_deref())
            || (under(self.home.as_deref()) && !under(self.zsh.as_deref()))
    }

    /// Gives the text of an origin for a person: the file, with `~` for the
    /// home directory, then `:` and the line when the origin has a line.
    #[must_use]
    pub fn show(&self, origin: &Origin) -> String {
        let below_home = self
            .home
            .as_deref()
            .and_then(|home| origin.file.strip_prefix(home).ok());
        let file = match below_home {
            Some(rest) if rest.as_os_str().is_empty() => HOME_MARK.to_string(),
            Some(rest) => format!("{HOME_MARK}/{}", rest.display()),
            None => origin.file.display().to_string(),
        };
        match origin.line {
            Some(line) => format!("{file}{LINE_SEPARATOR}{line}"),
            None => file,
        }
    }
}
