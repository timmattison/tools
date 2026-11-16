//! The state files on disk: read one file, read the directory, remove the
//! file of a stale session, and watch the directory for changes.

use std::path::{Path, PathBuf};

use proctree::Pid;

use crate::state::{StateFile, parse_state_file, pid_of_file_name};

/// The result of the read of one state file.
#[derive(Debug)]
pub enum ReadOutcome {
    /// The file holds a good state file of the contract.
    Present(StateFile),
    /// The file does not exist.
    Gone,
    /// The file exists, but cctop cannot use it. The text says why.
    Unreadable(String),
}

/// The path of the state file of `pid` in `dir`.
#[must_use]
pub fn state_file_path(dir: &Path, pid: Pid) -> PathBuf {
    dir.join(format!("{}.json", pid.as_u32()))
}

/// Read the state file at `path`.
///
/// A missing file is `Gone`. A file that cannot be read, or that is not a
/// state file of the contract, is `Unreadable`. A file whose pid is not the
/// pid in its name is `Unreadable` too.
#[must_use]
pub fn read_state_file(path: &Path) -> ReadOutcome {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ReadOutcome::Gone,
        Err(error) => return ReadOutcome::Unreadable(format!("cannot read the file: {error}")),
    };
    let file = match parse_state_file(&text) {
        Ok(file) => file,
        Err(error) => return ReadOutcome::Unreadable(error.to_string()),
    };
    let name_pid = path.file_name().and_then(pid_of_file_name);
    if name_pid != Some(file.pid) {
        return ReadOutcome::Unreadable(format!(
            "the file names pid {}, and its name does not",
            file.pid.as_u32()
        ));
    }
    ReadOutcome::Present(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/state-file-outside.json");

    #[test]
    fn a_good_file_is_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("51234.json");
        std::fs::write(&path, FIXTURE).unwrap();
        match read_state_file(&path) {
            ReadOutcome::Present(file) => assert_eq!(file.pid, Pid::new(51234)),
            other => panic!("expected Present, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_file_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = read_state_file(&dir.path().join("51234.json"));
        assert!(matches!(outcome, ReadOutcome::Gone), "{outcome:?}");
    }

    #[test]
    fn bad_json_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("51234.json");
        std::fs::write(&path, "not json").unwrap();
        let outcome = read_state_file(&path);
        assert!(matches!(outcome, ReadOutcome::Unreadable(_)), "{outcome:?}");
    }

    #[test]
    fn a_pid_that_differs_from_the_file_name_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("456.json");
        std::fs::write(&path, FIXTURE).unwrap();
        let outcome = read_state_file(&path);
        assert!(matches!(outcome, ReadOutcome::Unreadable(_)), "{outcome:?}");
    }
}
