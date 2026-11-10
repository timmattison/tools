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
use shellquote::shell_quote;

use crate::app::Version;

/// The first line of the error message when the function is missing.
const MISSING_FUNCTION: &str = "chf: the crap shell function is not defined in this shell.";
/// The line that tells the person how to define the function.
const FIX_COMMAND: &str = "Run: crap --shell-setup";
/// The line that tells the person what to do after the fix.
const FIX_NEXT: &str = "Then open a new shell and run chf again.";

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
pub fn shell_of(value: Option<&OsStr>) -> Option<Shell> {
    let name = Path::new(value?).file_name()?;
    match name.to_str()? {
        "bash" => Some(Shell::Bash),
        "zsh" => Some(Shell::Zsh),
        _ => None,
    }
}

/// The arguments of `crap` for that session and version, as shell text.
/// The id passes through `shell_quote`. `script` and `paste_command` both use this function.
fn crap_arguments(id: &SessionId, version: Version) -> String {
    let flags = match version {
        Version::Compacted => "--here",
        Version::Expanded => "--here --expand",
    };
    format!("{flags} {}", shell_quote(id.as_str()))
}

/// The text of the `-c` script for that session and version.
#[must_use]
pub fn script(id: &SessionId, version: Version) -> String {
    format!(
        "if ! typeset -f crap >/dev/null 2>&1; then\n  printf '%s\\n' {} {} {} >&2\n  exit 1\nfi\ncrap {}\n",
        shell_quote(MISSING_FUNCTION),
        shell_quote(FIX_COMMAND),
        shell_quote(FIX_NEXT),
        crap_arguments(id, version),
    )
}

/// The command a person pastes: `crap --here '<id>'` or `crap --here --expand '<id>'`.
#[must_use]
pub fn paste_command(id: &SessionId, version: Version) -> String {
    format!("crap {}", crap_arguments(id, version))
}

/// Replace this process with `<shell_path> -ic <script>`. Gives back only the error of a failed exec.
#[must_use]
pub fn exec(shell_path: &Path, script: &str) -> std::io::Error {
    exec_on_platform(shell_path, script)
}

#[cfg(unix)]
fn exec_on_platform(shell_path: &Path, script: &str) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(shell_path)
        .arg("-ic")
        .arg(script)
        .exec()
}

#[cfg(not(unix))]
fn exec_on_platform(_shell_path: &Path, _script: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "chf can not replace its process on this platform",
    )
}
