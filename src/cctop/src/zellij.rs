//! Focus step 1: asks Zellij to focus the pane of a session.

use crate::state::ZellijLocation;

/// Makes the argv that lists the Zellij sessions.
///
/// The argv has no `--short`, because `--short` does not mark an exited
/// session.
#[must_use]
pub fn list_sessions_argv() -> Vec<String> {
    ["zellij", "list-sessions", "--no-formatting"]
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// Makes the argv that focuses the pane at `location`.
#[must_use]
pub fn focus_argv(location: &ZellijLocation) -> Vec<String> {
    ["zellij", "--session", &location.session, "action", "focus-pane-id"]
        .iter()
        .map(ToString::to_string)
        .chain(std::iter::once(location.pane.terminal_target()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ZellijPaneId;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_list_argv_has_no_short_flag() {
        assert_eq!(
            list_sessions_argv(),
            argv(&["zellij", "list-sessions", "--no-formatting"])
        );
    }

    #[test]
    fn the_focus_argv_names_the_session_and_the_terminal_pane() {
        let location = ZellijLocation {
            session: "my tools".to_string(),
            pane: ZellijPaneId::parse("310").unwrap(),
        };
        assert_eq!(
            focus_argv(&location),
            argv(&[
                "zellij",
                "--session",
                "my tools",
                "action",
                "focus-pane-id",
                "terminal_310"
            ])
        );
    }
}
