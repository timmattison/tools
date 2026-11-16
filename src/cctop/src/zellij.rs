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

/// Reads the names of the live sessions from the output of `list-sessions`.
///
/// A live line has no `(EXITED`. The name is the text before ` [Created`.
#[must_use]
pub fn parse_live_sessions(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.contains("(EXITED"))
        .map(|line| {
            let name = line.split_once(" [Created").map_or(line, |(name, _)| name);
            name.trim().to_string()
        })
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
    fn the_live_sessions_of_the_real_output_are_the_four_live_names() {
        let stdout = include_str!("../fixtures/zellij-list-sessions.txt");
        assert_eq!(
            parse_live_sessions(stdout),
            argv(&["nvr-tools", "tools", "megarepo", "dbt"])
        );
    }

    #[test]
    fn blank_lines_are_skipped_and_a_name_keeps_its_inner_space() {
        let stdout = "\n  \nmy session [Created 1h ago] \n\nold [Created 2d ago] (EXITED - attach to resurrect)\n";
        assert_eq!(parse_live_sessions(stdout), argv(&["my session"]));
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
