//! Focus of a session in two steps. Step 1 asks Zellij to focus the pane.
//! Step 2 asks WezTerm to focus the window and the pane of the terminal.

use std::sync::Arc;

use proctree::Pid;

use crate::board::Session;
use crate::runner::Runner;
use crate::state::ZellijLocation;
use crate::wezterm::TtyWriter;

/// The session to focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusTarget {
    /// The process id of the Claude Code process.
    pub pid: Pid,
    /// The text that names the session on the screen.
    pub label: String,
    /// The Zellij session and pane of the session. `None` when the state file
    /// does not name both.
    pub zellij: Option<ZellijLocation>,
}

impl FocusTarget {
    /// Makes the target of `session`.
    #[must_use]
    pub fn of(_session: &Session) -> Self {
        Self {
            pid: Pid::new(0),
            label: String::new(),
            zellij: None,
        }
    }
}

/// The things that a focus needs from the system.
#[derive(Clone)]
pub struct FocusTools {
    /// Runs the external commands.
    pub runner: Arc<dyn Runner>,
    /// Writes the focus request to a terminal device.
    pub writer: Arc<dyn TtyWriter>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{parse_state_file, ZellijPaneId};

    pub(super) fn session(pid: u32, name: Option<&str>, zellij: Option<(&str, &str)>) -> Session {
        let (zellij_session, zellij_pane) = zellij.map_or(
            ("null".to_string(), "null".to_string()),
            |(session, pane)| (format!("\"{session}\""), format!("\"{pane}\"")),
        );
        let json = format!(
            r#"{{"version":1,"pid":{pid},"sessionId":"s","startedAt":1,"state":"needs-you","stamp":2,"cwd":"/work/tools","zellijSession":{zellij_session},"zellijPane":{zellij_pane}}}"#
        );
        let file = parse_state_file(&json).unwrap();
        Session {
            file,
            name: name.map(ToString::to_string),
            since: crate::state::EpochMillis::new(2),
        }
    }

    #[test]
    fn the_target_of_a_session_in_zellij_has_its_pid_label_and_location() {
        let target = FocusTarget::of(&session(29130, Some("docs"), Some(("tools", "310"))));
        assert_eq!(
            target,
            FocusTarget {
                pid: Pid::new(29130),
                label: "docs".to_string(),
                zellij: Some(ZellijLocation {
                    session: "tools".to_string(),
                    pane: ZellijPaneId::parse("310").unwrap(),
                }),
            }
        );
    }

    #[test]
    fn the_target_of_a_session_outside_zellij_has_no_location_and_the_directory_label() {
        let target = FocusTarget::of(&session(51234, None, None));
        assert_eq!(target.pid, Pid::new(51234));
        assert_eq!(target.label, "tools");
        assert_eq!(target.zellij, None);
    }
}
