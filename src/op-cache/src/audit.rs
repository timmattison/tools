//! The audit trail: a JSON Lines file that records each run of `op`.

use crate::caller::{Ancestor, ClaudeSession};
use serde::Serialize;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// `<state home>/op-cache/audit.jsonl`, or `None` when no state home resolves.
///
/// `XDG_STATE_HOME` wins when it is set, not empty, and an absolute path.
/// Otherwise `$HOME/.local/state` is the state home, when `HOME` is set and
/// not empty. This function is pure: it never reads the process environment.
pub(crate) fn audit_path(xdg_state_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let state_home = match xdg_state_home.map(Path::new) {
        Some(xdg) if xdg.is_absolute() => xdg.to_path_buf(),
        _ => {
            let home = home.filter(|value| !value.is_empty())?;
            Path::new(home).join(".local").join("state")
        }
    };
    Some(state_home.join("op-cache").join("audit.jsonl"))
}

/// The trail file, opened for appending.
pub(crate) struct AuditTrail {
    file: File,
}

impl AuditTrail {
    /// Creates the parent directory (mode 0700 on Unix) and opens the file for
    /// appending (mode 0600 on Unix).
    pub(crate) fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            let mut directories = fs::DirBuilder::new();
            directories.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                directories.mode(0o700);
            }
            directories.create(parent)?;
        }
        let mut options = OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        Ok(Self { file })
    }

    /// Writes `record` as one JSON line.
    pub(crate) fn append<T: Serialize>(&mut self, record: &T) -> std::io::Result<()> {
        serde_json::to_writer(&mut self.file, record).map_err(std::io::Error::other)?;
        self.file.write_all(b"\n")
    }
}

/// The record written before each run of `op`.
#[derive(Serialize)]
pub(crate) struct OpStart {
    pub(crate) time: String,
    pub(crate) event: &'static str,
    pub(crate) request_id: String,
    pub(crate) attempt: u32,
    pub(crate) operation: &'static str,
    pub(crate) reference: String,
    pub(crate) cwd: Option<String>,
    pub(crate) cache_path: String,
    pub(crate) zellij_session: Option<String>,
    pub(crate) zellij_pane: Option<String>,
    pub(crate) term_program: Option<String>,
    pub(crate) claude: Option<ClaudeSession>,
    pub(crate) ancestors: Vec<Ancestor>,
}

/// How a run of `op` ended. It never holds the standard output of `op`.
#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(crate) enum Outcome {
    Ok,
    Failed { op_stderr: String },
    SpawnError { error: String },
}

/// The record written after each run of `op`.
#[derive(Serialize)]
pub(crate) struct OpEnd {
    pub(crate) time: String,
    pub(crate) event: &'static str,
    pub(crate) request_id: String,
    pub(crate) attempt: u32,
    pub(crate) exit_code: Option<i32>,
    pub(crate) duration_ms: u64,
    #[serde(flatten)]
    pub(crate) outcome: Outcome,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::Path;

    fn os(text: &str) -> Option<&OsStr> {
        Some(OsStr::new(text))
    }

    #[test]
    fn an_absolute_xdg_state_home_wins() {
        assert_eq!(
            audit_path(os("/xdg/state"), os("/home/tim")),
            Some(Path::new("/xdg/state/op-cache/audit.jsonl").to_path_buf())
        );
    }

    #[test]
    fn an_empty_xdg_state_home_falls_back_to_home() {
        assert_eq!(
            audit_path(os(""), os("/home/tim")),
            Some(Path::new("/home/tim/.local/state/op-cache/audit.jsonl").to_path_buf())
        );
    }

    #[test]
    fn a_relative_xdg_state_home_falls_back_to_home() {
        assert_eq!(
            audit_path(os("state"), os("/home/tim")),
            Some(Path::new("/home/tim/.local/state/op-cache/audit.jsonl").to_path_buf())
        );
    }

    #[test]
    fn an_unset_xdg_state_home_falls_back_to_home() {
        assert_eq!(
            audit_path(None, os("/home/tim")),
            Some(Path::new("/home/tim/.local/state/op-cache/audit.jsonl").to_path_buf())
        );
    }

    #[test]
    fn no_usable_xdg_and_no_home_gives_none() {
        assert_eq!(audit_path(None, None), None);
        assert_eq!(audit_path(os(""), os("")), None);
        assert_eq!(audit_path(os("relative"), None), None);
    }

    #[cfg(unix)]
    mod modes {
        use super::super::AuditTrail;
        use std::os::unix::fs::PermissionsExt;

        fn mode_of(path: &std::path::Path) -> u32 {
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777
        }

        #[test]
        fn open_makes_the_directory_0700_and_the_file_0600() {
            let root = tempfile::tempdir().unwrap();
            let file = root
                .path()
                .join("state")
                .join("op-cache")
                .join("audit.jsonl");

            AuditTrail::open(&file).unwrap();

            assert_eq!(mode_of(file.parent().unwrap()), 0o700);
            assert_eq!(mode_of(&file), 0o600);
        }
    }
}
