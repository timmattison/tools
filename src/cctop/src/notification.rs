//! Sends a macOS notification through `osascript`.

/// The title of each notification.
pub const NOTIFICATION_TITLE: &str = "cctop";

/// Makes the argv that shows a notification.
///
/// The text goes in as arguments after `--`, never inside the AppleScript
/// source. Thus no value needs quotes.
#[must_use]
pub fn notification_argv(_title: &str, _message: &str) -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
