//! The state file that Claude Code writes for each session, and the types
//! that describe its content.

use std::path::PathBuf;

use proctree::Pid;

/// The state of one Claude Code session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaneState {
    /// The session works.
    Working,
    /// The session waits for the person.
    NeedsYou,
    /// The session waits for a new prompt.
    Idle,
    /// The session waits for its agents.
    Agents,
    /// The session waits for its shells.
    Shells,
}

/// A time in milliseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EpochMillis(u64);

impl EpochMillis {
    /// The time that `ms` milliseconds after the epoch names.
    #[must_use]
    pub const fn new(ms: u64) -> Self {
        Self(ms)
    }

    /// The number of milliseconds since the epoch.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// The number of a Zellij terminal pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ZellijPaneId(u32);

impl ZellijPaneId {
    /// The pane that `text` names, or `None` when `text` is not a number.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        text.parse::<u32>().ok().map(Self)
    }
}

/// The place of a session in Zellij.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZellijLocation {
    /// The name of the Zellij session.
    pub session: String,
    /// The pane in that session.
    pub pane: ZellijPaneId,
}

/// The content of one state file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateFile {
    /// The process id of the Claude Code process.
    pub pid: Pid,
    /// The id of the Claude Code session.
    pub session_id: String,
    /// The time when the session started.
    pub started_at: EpochMillis,
    /// The state of the session.
    pub state: PaneState,
    /// The time of the last write of the file.
    pub stamp: EpochMillis,
    /// The working directory of the session.
    pub cwd: PathBuf,
    /// The Zellij session, when the session runs in Zellij.
    pub zellij_session: Option<String>,
    /// The Zellij pane, when the session runs in Zellij.
    pub zellij_pane: Option<ZellijPaneId>,
}

impl StateFile {
    /// The place in Zellij, when the file names both a session and a pane.
    #[must_use]
    pub fn zellij_location(&self) -> Option<ZellijLocation> {
        None
    }
}

/// The reasons why a state file does not parse.
#[derive(Debug, thiserror::Error)]
pub enum StateFileError {
    /// The text is not JSON of the contract.
    #[error("the state file is not valid JSON of the contract: {0}")]
    Json(#[from] serde_json::Error),
    /// The file has a version that cctop does not read.
    #[error("the state file has version {0}, and cctop reads version 1 only")]
    Version(u64),
    /// The pane is not a number.
    #[error("the Zellij pane {0:?} is not a number")]
    Pane(String),
    /// The file names pid 0.
    #[error("the state file names pid 0")]
    ZeroPid,
}

/// Parse the text of one state file.
///
/// # Errors
///
/// Returns a [`StateFileError`] when the text does not follow the contract.
pub fn parse_state_file(text: &str) -> Result<StateFile, StateFileError> {
    let _ = text;
    Ok(StateFile {
        pid: Pid::new(0),
        session_id: String::new(),
        started_at: EpochMillis::new(0),
        state: PaneState::Working,
        stamp: EpochMillis::new(0),
        cwd: PathBuf::new(),
        zellij_session: None,
        zellij_pane: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZELLIJ: &str = include_str!("../fixtures/state-file-zellij.json");
    const OUTSIDE: &str = include_str!("../fixtures/state-file-outside.json");

    #[test]
    fn parses_the_zellij_fixture_into_every_field() {
        let file = parse_state_file(ZELLIJ).unwrap();
        assert_eq!(file.pid, Pid::new(80033));
        assert_eq!(file.session_id, "b91d041e-fc7b-4865-83a6-59c1dd96fa08");
        assert_eq!(file.started_at, EpochMillis::new(1_794_823_262_488));
        assert_eq!(file.state, PaneState::NeedsYou);
        assert_eq!(file.stamp, EpochMillis::new(1_794_824_265_058));
        assert_eq!(file.cwd, PathBuf::from("/path/to/worktree"));
        assert_eq!(file.zellij_session.as_deref(), Some("tools"));
        assert_eq!(file.zellij_pane, ZellijPaneId::parse("310"));
        assert_eq!(
            file.zellij_location(),
            Some(ZellijLocation {
                session: "tools".to_string(),
                pane: ZellijPaneId::parse("310").unwrap(),
            })
        );
    }

    #[test]
    fn parses_the_outside_fixture_with_no_zellij_fields() {
        let file = parse_state_file(OUTSIDE).unwrap();
        assert_eq!(file.pid, Pid::new(51234));
        assert_eq!(file.state, PaneState::Working);
        assert_eq!(file.zellij_session, None);
        assert_eq!(file.zellij_pane, None);
        assert_eq!(file.zellij_location(), None);
    }

    #[test]
    fn a_pane_with_a_null_session_has_no_location() {
        let text = ZELLIJ.replace("\"zellijSession\":\"tools\"", "\"zellijSession\":null");
        let file = parse_state_file(&text).unwrap();
        assert_eq!(file.zellij_pane, ZellijPaneId::parse("310"));
        assert_eq!(file.zellij_location(), None);
    }

    #[test]
    fn each_state_string_gives_its_variant() {
        let cases = [
            ("working", PaneState::Working),
            ("needs-you", PaneState::NeedsYou),
            ("idle", PaneState::Idle),
            ("agents", PaneState::Agents),
            ("shells", PaneState::Shells),
        ];
        for (text, expected) in cases {
            let json = ZELLIJ.replace("needs-you", text);
            assert_eq!(parse_state_file(&json).unwrap().state, expected, "{text}");
        }
    }

    #[test]
    fn an_unknown_state_string_is_an_error() {
        let json = ZELLIJ.replace("needs-you", "sleeping");
        assert!(matches!(
            parse_state_file(&json),
            Err(StateFileError::Json(_))
        ));
    }

    #[test]
    fn an_unknown_extra_key_is_ignored() {
        let json = ZELLIJ.replace("\"version\":1,", "\"version\":1,\"future\":[1,2],");
        assert_eq!(parse_state_file(&json).unwrap().pid, Pid::new(80033));
    }

    #[test]
    fn a_missing_key_is_a_json_error() {
        let json = ZELLIJ.replace("\"stamp\":1794824265058,", "");
        assert!(matches!(
            parse_state_file(&json),
            Err(StateFileError::Json(_))
        ));
    }
}
