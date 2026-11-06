//! The macOS notification that op-cache shows before it asks 1Password.

use std::ffi::OsStr;
use std::process::{Command, Stdio};

/// The title of the first attempt.
pub(crate) const TITLE: &str = "op-cache → 1Password";

/// The three texts of one notification.
pub(crate) struct Notification {
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) message: String,
}

/// Composes the notification for one attempt.
///
/// The title is [`TITLE`] on attempt 1, and `"op-cache → 1Password (attempt 2
/// of 3)"` after that. The subtitle joins the present, non-empty parts of
/// `claude_name` and `zellij_session` with `" · "`. When both are absent, it is
/// `repo_dir_name`, or an empty string when that is absent too. The message is
/// `reference`.
pub(crate) fn compose(
    attempt: u32,
    max_attempts: u32,
    reference: &str,
    claude_name: Option<&str>,
    zellij_session: Option<&str>,
    repo_dir_name: Option<&str>,
) -> Notification {
    let title = if attempt <= 1 {
        TITLE.to_owned()
    } else {
        format!("{TITLE} (attempt {attempt} of {max_attempts})")
    };
    let parts: Vec<&str> = [claude_name, zellij_session]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect();
    let subtitle = if parts.is_empty() {
        repo_dir_name.unwrap_or_default().to_owned()
    } else {
        parts.join(" · ")
    };
    Notification {
        title,
        subtitle,
        message: reference.to_owned(),
    }
}

/// False only when `OP_CACHE_NOTIFY` is exactly `0`. The caller passes the
/// value, so this function never reads the process environment.
pub(crate) fn notifications_enabled(op_cache_notify: Option<&OsStr>) -> bool {
    op_cache_notify != Some(OsStr::new("0"))
}

/// The constant AppleScript, one line for each `-e`. It holds no value: the
/// three texts reach it as `argv`, so a quote or a backslash in a name cannot
/// change the script.
pub(crate) const SCRIPT: [&str; 3] = [
    "on run argv",
    "display notification (item 3 of argv) with title (item 1 of argv) subtitle (item 2 of argv)",
    "end run",
];

/// The command that shows `notification`: `osascript`, found through PATH,
/// then `-e <line>` for each [`SCRIPT`] line, then the title, the subtitle and
/// the message as three separate arguments. All three standard streams are
/// null. This function does not start the command.
pub(crate) fn notification_command(notification: &Notification) -> Command {
    let _ = notification;
    let _ = Stdio::null();
    Command::new("osascript")
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE: &str = "op://Private/Item/field";

    #[test]
    fn attempt_one_has_the_plain_title() {
        let n = compose(1, 3, REFERENCE, None, None, None);
        assert_eq!(n.title, "op-cache → 1Password");
    }

    #[test]
    fn a_retry_numbers_the_attempt_in_the_title() {
        let n = compose(2, 3, REFERENCE, None, None, None);
        assert_eq!(n.title, "op-cache → 1Password (attempt 2 of 3)");
    }

    #[test]
    fn the_subtitle_joins_claude_and_zellij() {
        let n = compose(
            1,
            3,
            REFERENCE,
            Some("earth-science-d4"),
            Some("earth-science"),
            Some("repo"),
        );
        assert_eq!(n.subtitle, "earth-science-d4 · earth-science");
    }

    #[test]
    fn the_subtitle_keeps_one_present_part_alone() {
        let only_claude = compose(1, 3, REFERENCE, Some("c"), None, Some("repo"));
        assert_eq!(only_claude.subtitle, "c");
        let only_zellij = compose(1, 3, REFERENCE, None, Some("z"), Some("repo"));
        assert_eq!(only_zellij.subtitle, "z");
    }

    #[test]
    fn an_empty_part_counts_as_absent() {
        let n = compose(1, 3, REFERENCE, Some(""), Some("z"), Some("repo"));
        assert_eq!(n.subtitle, "z");
        let both = compose(1, 3, REFERENCE, Some(""), Some(""), Some("repo"));
        assert_eq!(both.subtitle, "repo");
    }

    #[test]
    fn the_subtitle_falls_back_to_the_repo_directory_name() {
        let n = compose(1, 3, REFERENCE, None, None, Some("my-repo"));
        assert_eq!(n.subtitle, "my-repo");
    }

    #[test]
    fn the_subtitle_is_empty_when_everything_is_absent() {
        let n = compose(1, 3, REFERENCE, None, None, None);
        assert_eq!(n.subtitle, "");
    }

    #[test]
    fn the_message_is_the_reference() {
        let n = compose(1, 3, REFERENCE, None, None, None);
        assert_eq!(n.message, REFERENCE);
    }

    #[test]
    fn only_an_exact_zero_turns_notifications_off() {
        assert!(notifications_enabled(None));
        assert!(notifications_enabled(Some(OsStr::new("1"))));
        assert!(notifications_enabled(Some(OsStr::new(""))));
        assert!(notifications_enabled(Some(OsStr::new("false"))));
        assert!(!notifications_enabled(Some(OsStr::new("0"))));
    }

    fn arguments(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    fn notification(subtitle: &str) -> Notification {
        Notification {
            title: TITLE.to_owned(),
            subtitle: subtitle.to_owned(),
            message: "op://Private/Item/field".to_owned(),
        }
    }

    #[test]
    fn the_command_is_osascript_by_name() {
        let command = notification_command(&notification("sub"));
        assert_eq!(command.get_program(), "osascript");
    }

    #[test]
    fn the_arguments_are_the_script_then_the_three_texts() {
        let command = notification_command(&notification("sub"));
        assert_eq!(
            arguments(&command),
            vec![
                "-e",
                SCRIPT[0],
                "-e",
                SCRIPT[1],
                "-e",
                SCRIPT[2],
                TITLE,
                "sub",
                "op://Private/Item/field"
            ]
        );
    }

    #[test]
    fn a_quote_and_a_backslash_in_the_subtitle_stay_in_their_own_argument() {
        let subtitle = r#"a "quoted" \ name"#;
        let command = notification_command(&notification(subtitle));
        let arguments = arguments(&command);
        assert_eq!(arguments[7], subtitle);
        let carriers = arguments.iter().filter(|a| a.contains(subtitle)).count();
        assert_eq!(carriers, 1);
    }

    #[test]
    fn no_script_line_holds_a_double_quote() {
        for line in SCRIPT {
            assert!(!line.contains('"'), "{line}");
        }
    }
}
