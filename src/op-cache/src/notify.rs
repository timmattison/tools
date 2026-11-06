//! The macOS notification that op-cache shows before it asks 1Password.

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
    let _ = (
        attempt,
        max_attempts,
        reference,
        claude_name,
        zellij_session,
        repo_dir_name,
    );
    Notification {
        title: String::new(),
        subtitle: String::new(),
        message: String::new(),
    }
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
}
