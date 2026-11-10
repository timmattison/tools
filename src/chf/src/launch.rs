//! Resume a session through the `crap` shell function.
//!
//! `ch` is the alias `crap --here`, and `che` is the alias `crap --here
//! --expand`. They exist only in an interactive shell. The `crap` binary alone
//! does not start Claude. The `crap` shell function reads its output, starts
//! Claude through `CRAP_LAUNCHER`, and removes the import. So chf replaces its
//! own process with `$SHELL -ic <script>`, and the script calls the `crap`
//! function. chf calls `crap --here`, not the alias, because a person can have
//! the function and no `ch` alias.
//!
//! chf has no `--shell-setup`, because `--here` does not change the directory
//! of the parent shell.

use std::ffi::OsStr;
use std::path::Path;

use claude_transcript::SessionId;

use crate::app::Version;

/// A shell that chf can start with `-ic`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// GNU bash.
    Bash,
    /// The Z shell.
    Zsh,
}

/// The base name of the value of $SHELL is exactly `bash` or `zsh`. Each other value, an empty value, or no value gives None.
#[must_use]
pub fn shell_of(_value: Option<&OsStr>) -> Option<Shell> {
    None
}

/// The text of the `-c` script for that session and version.
#[must_use]
pub fn script(_id: &SessionId, _version: Version) -> String {
    "exit 0".to_string()
}

/// The command a person pastes: `crap --here '<id>'` or `crap --here --expand '<id>'`.
#[must_use]
pub fn paste_command(_id: &SessionId, _version: Version) -> String {
    String::new()
}

/// Replace this process with `<shell_path> -ic <script>`. Gives back only the error of a failed exec.
#[must_use]
pub fn exec(_shell_path: &Path, _script: &str) -> std::io::Error {
    std::io::Error::other("stub")
}
