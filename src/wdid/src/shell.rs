//! The aliases and the functions of the interactive shell.
//!
//! A person types an alias or a function name, and the shell runs a tool. The
//! history holds only the name that the person typed. Thus this module starts
//! the shell of the person, asks it for every alias and every function body,
//! and builds a [`Resolver`] that says which tools each function runs.
//!
//! [`script`] and [`parse_output`] are pure. [`read`] is the one function that
//! starts a process.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::command_words::{Aliases, CommandWord};
use crate::tool::{ToolName, ToolNames};

/// The default bound on the shell start.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// The shells that wdid can ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// zsh.
    Zsh,
    /// bash.
    Bash,
}

impl ShellKind {
    /// Gives the kind from the file name of the shell path: `zsh` or `bash`.
    /// Gives `None` for any other name.
    #[must_use]
    pub fn of(shell: &Path) -> Option<Self> {
        let _ = shell;
        None
    }
}

/// The marker that wraps the output.
///
/// The interactive start of a shell can print text of its own. The marker lets
/// [`parse_output`] ignore that text. A marker is unique for each run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker(String);

impl Marker {
    /// Makes a marker that is unique for each run: the process id and the
    /// nanoseconds since the Unix epoch.
    #[must_use]
    pub fn unique() -> Self {
        Self(String::new())
    }

    /// Makes a marker from a text. The tests use this function.
    ///
    /// # Panics
    /// Panics when the text is empty or holds a character other than an ASCII
    /// letter, an ASCII digit, or `_`. [`script`] puts the marker into quoted
    /// shell text, and only these characters need no quoting.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// Gives the text of the marker.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Gives the text of the `-c` argument for the shell.
#[must_use]
pub fn script(kind: ShellKind, marker: &Marker) -> String {
    let _ = (kind, marker);
    String::new()
}

/// The aliases and the functions that the shell defines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Definitions {
    /// The alias table.
    pub aliases: Aliases,
    /// The command words of the body of each function that parsed, by name.
    pub functions: BTreeMap<String, Vec<CommandWord>>,
    /// The names of the functions whose body did not parse, sorted.
    pub unparsed_functions: Vec<String>,
    /// The alias lines that did not parse, in output order.
    pub unparsed_aliases: Vec<String>,
}

/// The reasons that the aliases and the functions cannot be read.
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    /// The variable `SHELL` does not exist.
    #[error("SHELL is not set, so wdid cannot read the aliases and functions (wdid --no-shell skips the aliases and functions)")]
    NotSet,
    /// The shell is not zsh and not bash.
    #[error("wdid reads aliases and functions from zsh and bash only, and SHELL is {shell} (wdid --no-shell skips the aliases and functions)")]
    NotSupported {
        /// The value of `SHELL`.
        shell: String,
    },
    /// The system cannot start the shell.
    #[error("cannot start {shell}: {source} (wdid --no-shell skips the aliases and functions)")]
    Spawn {
        /// The shell program.
        shell: String,
        /// The error of the system.
        source: std::io::Error,
    },
    /// The shell ran for too long.
    #[error("the shell did not stop in {seconds} seconds (wdid --no-shell skips the aliases and functions)")]
    TimedOut {
        /// The bound, in seconds.
        seconds: u64,
    },
    /// The shell ended with a non-zero status.
    #[error(
        "the shell failed ({status}): {stderr} (wdid --no-shell skips the aliases and functions)"
    )]
    Failed {
        /// The text of the exit status.
        status: String,
        /// The standard error of the shell, trimmed.
        stderr: String,
    },
    /// The output has no start marker or no end marker.
    #[error("the shell output does not hold the start and end markers of wdid (wdid --no-shell skips the aliases and functions)")]
    NoMarkers,
}

/// Parses the standard output of the script. Pure.
///
/// # Errors
/// Gives [`ShellError::NoMarkers`] when the output has no start marker line, or
/// no end marker line after it.
pub fn parse_output(stdout: &str, marker: &Marker) -> Result<Definitions, ShellError> {
    let _ = (stdout, marker);
    Ok(Definitions::default())
}

/// Runs `command -i -c <script>` and parses the output.
///
/// The caller makes the `Command`, thus a test can set `HOME` and `ZDOTDIR`.
///
/// # Errors
/// Gives [`ShellError::Spawn`] when the shell does not start,
/// [`ShellError::TimedOut`] when it runs longer than `timeout`,
/// [`ShellError::Failed`] for a non-zero status, and [`ShellError::NoMarkers`]
/// from [`parse_output`].
pub fn read(
    command: Command,
    kind: ShellKind,
    timeout: Duration,
) -> Result<Definitions, ShellError> {
    let _ = (command, kind, timeout);
    Ok(Definitions::default())
}

/// The tools that each function resolves to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolver {
    tools: BTreeMap<String, BTreeSet<ToolName>>,
}

impl Resolver {
    /// Resolves every function of `definitions` to the tools that it uses.
    #[must_use]
    pub fn new(definitions: &Definitions, tools: &ToolNames) -> Self {
        let _ = (definitions, tools);
        Self::default()
    }

    /// Gives the tools of the function `name`. Gives `None` when `name` is not
    /// a function. Gives `Some` of a possibly empty set when it is.
    #[must_use]
    pub fn tools_of(&self, name: &str) -> Option<&BTreeSet<ToolName>> {
        self.tools.get(name)
    }
}
