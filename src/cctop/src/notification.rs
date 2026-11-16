//! Sends a macOS notification through `osascript`.

use crate::board::Change;
use crate::runner::{Runner, COMMAND_LIMIT};

/// The title of each notification.
pub const NOTIFICATION_TITLE: &str = "cctop";

/// Makes the argv that shows a notification.
///
/// The text goes in as arguments after `--`, never inside the `AppleScript`
/// source. Thus no value needs quotes.
#[must_use]
pub fn notification_argv(title: &str, message: &str) -> Vec<String> {
    [
        "osascript",
        "-e",
        "on run argv",
        "-e",
        "display notification (item 2 of argv) with title (item 1 of argv)",
        "-e",
        "end run",
        "--",
        title,
        message,
    ]
    .iter()
    .map(ToString::to_string)
    .collect()
}

/// Makes the text of the notification for a session that needs the user.
#[must_use]
pub fn needs_you_message(change: &Change) -> String {
    format!("{} needs you", change.label)
}

/// Shows a notification with `message`.
///
/// # Errors
///
/// Returns the reason as text when `osascript` fails or does not run.
pub fn send(runner: &dyn Runner, message: &str) -> Result<(), String> {
    let argv = notification_argv(NOTIFICATION_TITLE, message);
    let output = runner
        .run(&argv, COMMAND_LIMIT)
        .map_err(|error| error.to_string())?;
    if output.succeeded() {
        Ok(())
    } else {
        Err(format!("osascript failed: {}", output.stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use proctree::Pid;

    use super::*;
    use crate::runner::fake::FakeRunner;
    use crate::runner::{CommandOutput, RunError};
    use crate::state::{EpochMillis, PaneState};

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_argv_passes_the_text_as_arguments() {
        assert_eq!(
            notification_argv("cctop", "tools needs you"),
            argv(&[
                "osascript",
                "-e",
                "on run argv",
                "-e",
                "display notification (item 2 of argv) with title (item 1 of argv)",
                "-e",
                "end run",
                "--",
                "cctop",
                "tools needs you",
            ])
        );
    }

    #[test]
    fn the_argv_keeps_each_hard_value_as_one_argument() {
        for value in ["say \"hi\"", "a\\b", "-x", "日本語 🎉"] {
            let result = notification_argv(value, value);
            assert_eq!(result.len(), 10, "{value}");
            assert_eq!(result[8], value);
            assert_eq!(result[9], value);
        }
    }

    #[test]
    fn the_message_names_the_session() {
        let change = Change {
            at: EpochMillis::new(1),
            pid: Pid::new(7),
            label: "日本語 tools".to_string(),
            old: Some(PaneState::Working),
            new: Some(PaneState::NeedsYou),
        };
        assert_eq!(needs_you_message(&change), "日本語 tools needs you");
    }

    fn expected_argv(message: &str) -> Vec<String> {
        notification_argv(NOTIFICATION_TITLE, message)
    }

    #[test]
    fn send_runs_the_notification_once_and_accepts_a_zero_exit() {
        let words = expected_argv("tools needs you");
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let runner = FakeRunner::new().answer(&words, Ok(CommandOutput::ok("")));
        assert_eq!(send(&runner, "tools needs you"), Ok(()));
        assert_eq!(runner.calls(), vec![expected_argv("tools needs you")]);
    }

    #[test]
    fn send_reports_the_stderr_of_a_failed_command() {
        let words = expected_argv("m");
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let runner =
            FakeRunner::new().answer(&words, Ok(CommandOutput::failed(1, "  no permission \n")));
        assert_eq!(
            send(&runner, "m"),
            Err("osascript failed: no permission".to_string())
        );
    }

    #[test]
    fn send_reports_a_run_error() {
        let words = expected_argv("m");
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let error = RunError::NotInstalled {
            program: "osascript".to_string(),
        };
        let runner = FakeRunner::new().answer(&words, Err(error.clone()));
        assert_eq!(send(&runner, "m"), Err(error.to_string()));
    }
}
