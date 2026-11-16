//! The state files on disk: read one file, read the directory, remove the
//! file of a stale session, and watch the directory for changes.

use std::path::{Path, PathBuf};

use proctree::Pid;

use crate::liveness::ProcessClock;
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

/// Read each `<pid>.json` file in `dir`, sorted by pid.
///
/// Other names are not state files, so the function skips them. This covers
/// `<pid>.json.tmp`, text files, and sub-directories.
///
/// # Errors
///
/// Returns the error of the read of the directory itself.
pub fn read_dir(dir: &Path) -> std::io::Result<Vec<(Pid, ReadOutcome)>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(pid) = path.file_name().and_then(pid_of_file_name) else {
            continue;
        };
        if path.is_dir() {
            continue;
        }
        entries.push((pid, read_state_file(&path)));
    }
    entries.sort_by_key(|(pid, _)| pid.as_u32());
    Ok(entries)
}

/// Remove the state file of `pid` when its session is stale.
///
/// The function reads the file again and checks the stale rule on the new
/// content. A new process can reuse the pid and write a new file before the
/// removal. The function keeps such a file. It also keeps a file that is
/// missing or unreadable. The result is `true` only when it removed the file.
#[must_use]
pub fn remove_if_stale(_dir: &Path, _pid: Pid, _clock: &mut dyn ProcessClock) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/state-file-outside.json");

    /// The JSON of a state file of the contract, outside Zellij.
    fn state_json(pid: u32, state: &str, started_at: u64, stamp: u64) -> String {
        format!(
            r#"{{"version":1,"pid":{pid},"sessionId":"s-{pid}","startedAt":{started_at},"state":"{state}","stamp":{stamp},"cwd":"/path/to/worktree","zellijSession":null,"zellijPane":null}}"#
        )
    }

    #[test]
    fn the_directory_read_takes_only_pid_json_names_sorted_by_pid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        std::fs::write(path.join("300.json"), state_json(300, "idle", 1, 2)).unwrap();
        std::fs::write(path.join("20.json"), state_json(20, "working", 1, 2)).unwrap();
        std::fs::write(path.join("100.json"), "not json").unwrap();
        std::fs::write(path.join("400.json.tmp"), state_json(400, "idle", 1, 2)).unwrap();
        std::fs::write(path.join("notes.txt"), "notes").unwrap();
        std::fs::write(path.join("abc.json"), "{}").unwrap();
        std::fs::create_dir(path.join("500.json")).unwrap();
        let entries = read_dir(path).unwrap();
        let pids: Vec<u32> = entries.iter().map(|(pid, _)| pid.as_u32()).collect();
        assert_eq!(pids, vec![20, 100, 300]);
        assert!(matches!(entries[0].1, ReadOutcome::Present(_)));
        assert!(matches!(entries[1].1, ReadOutcome::Unreadable(_)));
        assert!(matches!(entries[2].1, ReadOutcome::Present(_)));
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_dir(&dir.path().join("none")).is_err());
    }

    /// A process table that the test fills by hand.
    struct FakeClock(std::collections::HashMap<Pid, u64>);

    impl ProcessClock for FakeClock {
        fn start_times(&mut self, pids: &[Pid]) -> std::collections::HashMap<Pid, u64> {
            pids.iter()
                .filter_map(|pid| self.0.get(pid).map(|secs| (*pid, *secs)))
                .collect()
        }
    }

    fn clock_with(pid: u32, start_secs: u64) -> FakeClock {
        FakeClock(std::collections::HashMap::from([(Pid::new(pid), start_secs)]))
    }

    #[test]
    fn the_file_of_a_gone_process_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_file_path(dir.path(), Pid::new(77));
        std::fs::write(&path, state_json(77, "idle", 1_000_000, 2_000_000)).unwrap();
        let mut clock = FakeClock(std::collections::HashMap::new());
        assert!(remove_if_stale(dir.path(), Pid::new(77), &mut clock));
        assert!(!path.exists());
    }

    #[test]
    fn the_file_of_a_process_that_started_after_the_session_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_file_path(dir.path(), Pid::new(77));
        std::fs::write(&path, state_json(77, "idle", 1_000_000, 2_000_000)).unwrap();
        let mut clock = clock_with(77, 5_000);
        assert!(remove_if_stale(dir.path(), Pid::new(77), &mut clock));
        assert!(!path.exists());
    }

    #[test]
    fn a_file_that_is_not_stale_on_a_second_read_stays() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_file_path(dir.path(), Pid::new(77));
        // A new process reused the pid and wrote a newer startedAt.
        std::fs::write(&path, state_json(77, "idle", 9_000_000, 9_100_000)).unwrap();
        let mut clock = clock_with(77, 5_000);
        assert!(!remove_if_stale(dir.path(), Pid::new(77), &mut clock));
        assert!(path.exists());
    }

    #[test]
    fn a_missing_file_gives_false() {
        let dir = tempfile::tempdir().unwrap();
        let mut clock = FakeClock(std::collections::HashMap::new());
        assert!(!remove_if_stale(dir.path(), Pid::new(77), &mut clock));
    }

    #[test]
    fn an_unreadable_file_stays() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_file_path(dir.path(), Pid::new(77));
        std::fs::write(&path, "not json").unwrap();
        let mut clock = FakeClock(std::collections::HashMap::new());
        assert!(!remove_if_stale(dir.path(), Pid::new(77), &mut clock));
        assert!(path.exists());
    }

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
