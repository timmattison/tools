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

/// A color of the palette, in 8-bit channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    /// The red channel.
    pub red: u8,
    /// The green channel.
    pub green: u8,
    /// The blue channel.
    pub blue: u8,
}

impl PaneState {
    /// Each state, in the order of the declaration.
    pub const ALL: [PaneState; 5] = [
        PaneState::Working,
        PaneState::NeedsYou,
        PaneState::Idle,
        PaneState::Agents,
        PaneState::Shells,
    ];

    /// The word that the state file and the screen use for the state.
    #[must_use]
    pub fn label(self) -> &'static str {
        ""
    }

    /// The place of the state in the sort order. A low rank sorts first.
    #[must_use]
    pub fn rank(self) -> u8 {
        0
    }

    /// The color of the state. It is the default palette of the tab bar.
    #[must_use]
    pub fn color(self) -> Rgb {
        Rgb {
            red: 0,
            green: 0,
            blue: 0,
        }
    }
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
        let session = self.zellij_session.clone()?;
        let pane = self.zellij_pane?;
        Some(ZellijLocation { session, pane })
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
    let raw: RawStateFile = serde_json::from_str(text)?;
    if raw.version != 1 {
        return Err(StateFileError::Version(raw.version));
    }
    if raw.pid == 0 {
        return Err(StateFileError::ZeroPid);
    }
    let zellij_pane = match raw.zellij_pane {
        None => None,
        Some(text) => Some(ZellijPaneId::parse(&text).ok_or(StateFileError::Pane(text))?),
    };
    Ok(StateFile {
        pid: Pid::new(raw.pid),
        session_id: raw.session_id,
        started_at: EpochMillis::new(raw.started_at),
        state: raw.state,
        stamp: EpochMillis::new(raw.stamp),
        cwd: raw.cwd,
        zellij_session: raw.zellij_session,
        zellij_pane,
    })
}

/// The JSON shape of a state file, before the checks.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawStateFile {
    version: u64,
    pid: u32,
    session_id: String,
    started_at: u64,
    state: PaneState,
    stamp: u64,
    cwd: PathBuf,
    zellij_session: Option<String>,
    zellij_pane: Option<String>,
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

    #[test]
    fn a_version_other_than_one_is_refused() {
        let json = ZELLIJ.replace("\"version\":1", "\"version\":2");
        assert!(matches!(
            parse_state_file(&json),
            Err(StateFileError::Version(2))
        ));
    }

    #[test]
    fn a_pane_that_is_not_digits_is_refused() {
        let json = ZELLIJ.replace("\"310\"", "\"terminal_3\"");
        match parse_state_file(&json) {
            Err(StateFileError::Pane(text)) => assert_eq!(text, "terminal_3"),
            other => panic!("expected a pane error, got {other:?}"),
        }
    }

    #[test]
    fn pid_zero_is_refused() {
        let json = ZELLIJ.replace("\"pid\":80033", "\"pid\":0");
        assert!(matches!(
            parse_state_file(&json),
            Err(StateFileError::ZeroPid)
        ));
    }

    #[test]
    fn rank_gives_the_sort_order() {
        let mut states = PaneState::ALL;
        states.sort_by_key(|state| state.rank());
        assert_eq!(
            states,
            [
                PaneState::NeedsYou,
                PaneState::Idle,
                PaneState::Agents,
                PaneState::Shells,
                PaneState::Working,
            ]
        );
        let ranks: Vec<u8> = states.iter().map(|state| state.rank()).collect();
        assert_eq!(ranks, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn label_gives_the_contract_strings() {
        let labels: Vec<&str> = PaneState::ALL.iter().map(|state| state.label()).collect();
        assert_eq!(labels, ["working", "needs-you", "idle", "agents", "shells"]);
    }

    #[test]
    fn color_gives_the_palette() {
        let rgb = |red, green, blue| Rgb { red, green, blue };
        assert_eq!(PaneState::Working.color(), rgb(0x2f, 0x9e, 0x44));
        assert_eq!(PaneState::NeedsYou.color(), rgb(0xfa, 0xb0, 0x05));
        assert_eq!(PaneState::Idle.color(), rgb(0xe0, 0x31, 0x31));
        assert_eq!(PaneState::Agents.color(), rgb(0xff, 0xff, 0xff));
        assert_eq!(PaneState::Shells.color(), rgb(0x00, 0xcc, 0xcc));
    }
}
