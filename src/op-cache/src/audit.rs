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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // An older or hand-made file may be open to the group or others.
            let mode = file.metadata()?.permissions().mode();
            if mode & 0o077 != 0 {
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
        }
        Ok(Self { file })
    }

    /// Writes `record` as one JSON line.
    ///
    /// The record and its newline go into one buffer, and one `write` call
    /// sends the buffer. The file is opened for appending, so the kernel puts
    /// one write at the end of the file as a unit, and records from several
    /// writers never interleave. A short write is an error.
    pub(crate) fn append<T: Serialize>(&mut self, record: &T) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(record).map_err(std::io::Error::other)?;
        line.push(b'\n');
        let written = self.file.write(&line)?;
        if written == line.len() {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                format!(
                    "short write to the audit trail: {written} of {} bytes",
                    line.len()
                ),
            ))
        }
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

/// The longest prefix of `text` that is at most `max_bytes` bytes and ends on
/// a char boundary. It never slices inside a multi-byte character.
pub(crate) fn truncate_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default()
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

        #[test]
        fn open_narrows_an_existing_file_to_0600() {
            let root = tempfile::tempdir().unwrap();
            let file = root.path().join("audit.jsonl");
            std::fs::write(&file, "").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();

            AuditTrail::open(&file).unwrap();

            assert_eq!(mode_of(&file), 0o600);
        }
    }

    #[derive(serde::Serialize)]
    struct Probe {
        thread: usize,
        sequence: usize,
        padding: String,
    }

    #[test]
    fn concurrent_writers_never_interleave_records() {
        const THREADS: usize = 16;
        const RECORDS: usize = 50;
        const PADDING_BYTES: usize = 2000;

        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("op-cache").join("audit.jsonl");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();

        std::thread::scope(|scope| {
            for thread in 0..THREADS {
                let file = &file;
                scope.spawn(move || {
                    let mut trail = AuditTrail::open(file).unwrap();
                    for sequence in 0..RECORDS {
                        let padding = "x".repeat(PADDING_BYTES);
                        trail
                            .append(&Probe {
                                thread,
                                sequence,
                                padding,
                            })
                            .unwrap();
                    }
                });
            }
        });

        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text.lines().count(), THREADS * RECORDS);
        for line in text.lines() {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|error| panic!("interleaved line: {error}"));
        }
    }

    #[test]
    fn ascii_longer_than_the_limit_is_cut_to_the_limit() {
        assert_eq!(truncate_at_char_boundary("abcdefgh", 5), "abcde");
    }

    #[test]
    fn text_shorter_than_the_limit_is_unchanged() {
        assert_eq!(truncate_at_char_boundary("abc", 5), "abc");
    }

    #[test]
    fn japanese_is_cut_on_a_char_boundary() {
        assert_eq!(truncate_at_char_boundary("日本語", 4), "日");
    }

    #[test]
    fn emoji_is_cut_on_a_char_boundary() {
        assert_eq!(truncate_at_char_boundary("🎉🎉", 5), "🎉");
    }

    #[test]
    fn accented_text_is_cut_on_a_char_boundary() {
        assert_eq!(truncate_at_char_boundary("café", 4), "caf");
    }

    #[test]
    fn a_limit_of_zero_gives_the_empty_string() {
        assert_eq!(truncate_at_char_boundary("日本語", 0), "");
    }
}
