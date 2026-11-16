//! Focus step 2: finds the WezTerm pane of a terminal, and asks WezTerm to
//! focus it.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Component, Path};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

/// The directory that holds the terminal devices.
const DEVICE_DIRECTORY: &str = "/dev/";

/// The name of the user variable that the WezTerm handler reads.
pub const FOCUS_USER_VAR: &str = "claude-focus-pane";

/// Makes the escape sequence that asks WezTerm to focus the pane `pane_id`.
///
/// The sequence sets the user variable [`FOCUS_USER_VAR`] to the Base64 text of
/// the decimal pane number. The handler of WezTerm reads only the value, so
/// any pane can send the sequence.
#[must_use]
pub fn focus_request(pane_id: u64) -> String {
    let value = STANDARD.encode(pane_id.to_string());
    format!("\x1b]1337;SetUserVar={FOCUS_USER_VAR}={value}\x07")
}

/// Writes bytes to a terminal device.
pub trait TtyWriter: Send + Sync {
    /// Writes `bytes` to the device at `tty`.
    ///
    /// # Errors
    ///
    /// Returns the error of the write, or of the refusal of the path.
    fn write(&self, tty: &Path, bytes: &[u8]) -> io::Result<()>;
}

/// The writer of the real system. It writes to a device under `/dev/` and
/// refuses each other path.
///
/// The writer opens the device for writing only. It never creates a file and
/// never truncates one.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeviceWriter;

impl TtyWriter for DeviceWriter {
    fn write(&self, tty: &Path, bytes: &[u8]) -> io::Result<()> {
        let has_parent_step = tty
            .components()
            .any(|part| matches!(part, Component::ParentDir));
        if !tty.starts_with(DEVICE_DIRECTORY) || has_parent_step {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a device under /dev/", tty.display()),
            ));
        }
        let mut device = OpenOptions::new().write(true).open(tty)?;
        device.write_all(bytes)
    }
}

/// Makes the argv that lists the WezTerm panes as JSON.
#[must_use]
pub fn list_argv() -> Vec<String> {
    ["wezterm", "cli", "list", "--format", "json"]
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// One pane in the list of WezTerm.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct WeztermPane {
    /// The number of the pane.
    pub pane_id: u64,
    /// The terminal device of the pane, for example `/dev/ttys231`.
    #[serde(default)]
    pub tty_name: Option<String>,
}

/// Reads the panes from the output of `wezterm cli list --format json`.
///
/// The function ignores the other keys of each pane.
///
/// # Errors
///
/// Returns an error when the text is not a JSON list of panes.
pub fn parse_panes(json: &str) -> Result<Vec<WeztermPane>, serde_json::Error> {
    serde_json::from_str(json)
}

/// Finds the pane that shows the terminal device `tty`.
#[must_use]
pub fn pane_for_tty(panes: &[WeztermPane], tty: &str) -> Option<u64> {
    panes
        .iter()
        .find(|pane| pane.tty_name.as_deref() == Some(tty))
        .map(|pane| pane.pane_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = include_str!("../fixtures/wezterm-list.json");

    fn pane(id: u64, tty: Option<&str>) -> WeztermPane {
        WeztermPane {
            pane_id: id,
            tty_name: tty.map(ToString::to_string),
        }
    }

    #[test]
    fn the_list_argv_asks_for_json() {
        assert_eq!(list_argv(), vec!["wezterm", "cli", "list", "--format", "json"]);
    }

    #[test]
    fn the_fixture_has_three_panes() {
        assert_eq!(
            parse_panes(LIST).unwrap(),
            vec![
                pane(378, Some("/dev/ttys354")),
                pane(239, Some("/dev/ttys231")),
                pane(235, Some("/dev/ttys097")),
            ]
        );
    }

    #[test]
    fn a_pane_without_a_tty_name_parses_with_none() {
        assert_eq!(
            parse_panes(r#"[{"pane_id": 7, "title": "x"}]"#).unwrap(),
            vec![pane(7, None)]
        );
    }

    #[test]
    fn bad_json_is_an_error() {
        assert!(parse_panes("not json").is_err());
        assert!(parse_panes(r#"{"pane_id": 7}"#).is_err());
    }

    #[test]
    fn a_tty_finds_its_pane() {
        let panes = parse_panes(LIST).unwrap();
        assert_eq!(pane_for_tty(&panes, "/dev/ttys231"), Some(239));
        assert_eq!(pane_for_tty(&panes, "/dev/ttys354"), Some(378));
    }

    #[test]
    fn an_unknown_tty_has_no_pane() {
        let panes = parse_panes(LIST).unwrap();
        assert_eq!(pane_for_tty(&panes, "/dev/ttys999"), None);
        assert_eq!(pane_for_tty(&[pane(7, None)], "/dev/ttys231"), None);
    }

    #[test]
    fn the_focus_request_of_pane_239_is_exact() {
        assert_eq!(
            focus_request(239),
            "\x1b]1337;SetUserVar=claude-focus-pane=MjM5\x07"
        );
    }

    #[test]
    fn the_value_of_the_focus_request_decodes_to_the_pane_number() {
        let request = focus_request(378);
        let value = request
            .strip_prefix("\x1b]1337;SetUserVar=claude-focus-pane=")
            .and_then(|rest| rest.strip_suffix('\x07'))
            .unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(value)
            .unwrap();
        assert_eq!(decoded, b"378");
        assert_eq!(FOCUS_USER_VAR, "claude-focus-pane");
    }

    #[test]
    fn a_path_outside_dev_is_refused_and_the_file_stays_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("existing");
        std::fs::write(&existing, "old").unwrap();
        let error = DeviceWriter.write(&existing, b"new").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "old");

        let missing = dir.path().join("missing");
        let error = DeviceWriter.write(&missing, b"new").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!missing.exists());
    }

    #[test]
    fn a_write_to_dev_null_succeeds() {
        assert!(DeviceWriter.write(Path::new("/dev/null"), b"x").is_ok());
    }

    #[test]
    fn a_device_that_does_not_exist_is_an_error_and_is_not_created() {
        let path = format!("/dev/cctop-no-such-device-{}", std::process::id());
        let path = Path::new(&path);
        assert!(DeviceWriter.write(path, b"x").is_err());
        assert!(!path.exists());
    }

    #[test]
    fn a_path_that_leaves_dev_with_a_parent_step_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "old").unwrap();
        let sneaky = Path::new("/dev/..").join(target.strip_prefix("/").unwrap());
        let error = DeviceWriter.write(&sneaky, b"new").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
    }
}
