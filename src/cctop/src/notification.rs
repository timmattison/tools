//! Sends a macOS notification through `osascript`.

use crate::board::Change;

/// The title of each notification.
pub const NOTIFICATION_TITLE: &str = "cctop";

/// Makes the argv that shows a notification.
///
/// The text goes in as arguments after `--`, never inside the AppleScript
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
pub fn needs_you_message(_change: &Change) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use proctree::Pid;

    use super::*;
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
}
