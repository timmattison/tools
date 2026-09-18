//! The texts that popstop shows to the user.
//!
//! The functions here are pure, so the tests reach every text without a
//! terminal, a lock, or an audio device.

use std::path::Path;

/// The command that stops the copy that runs.
const STOP_COMMAND: &str = "popstop --stop";

/// The flag that names the state directory.
const STATE_DIR_FLAG: &str = "--state-dir";

/// Gives the command that stops the copy that runs.
///
/// A copy that runs with `--state-dir` holds the lock in that directory, so
/// the command names the same directory. The path is quoted for a shell, so
/// the user can copy the command as it is.
#[must_use]
pub fn stop_command(state_dir: Option<&Path>) -> String {
    match state_dir {
        None => STOP_COMMAND.to_owned(),
        Some(dir) => format!(
            "{STOP_COMMAND} {STATE_DIR_FLAG} {}",
            shellquote::shell_quote(&dir.to_string_lossy())
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::stop_command;
    use std::path::Path;

    #[test]
    fn the_stop_command_names_a_state_directory_quoted_for_a_shell() {
        assert_eq!(stop_command(None), "popstop --stop");
        assert_eq!(
            stop_command(Some(Path::new("/tmp/state dir"))),
            "popstop --stop --state-dir '/tmp/state dir'"
        );
        assert_eq!(
            stop_command(Some(Path::new("/tmp/it's here"))),
            r"popstop --stop --state-dir '/tmp/it'\''s here'"
        );
    }
}
