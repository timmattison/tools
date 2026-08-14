//! The session Claude Code recorded for a running process.
//!
//! A live session writes `~/.claude/sessions/<pid>.json` and keeps it current,
//! so the session a process belongs to is a recorded fact rather than something
//! to be reconstructed. `claude agents --json` prints these same files.
//!
//! The file is read here rather than through that command for two reasons. The
//! command costs a subprocess on every run, and it drops the `version` field,
//! which is the one fact this tool exists to report.

use crate::process::ProcessFact;
use crate::SessionId;
use std::path::{Path, PathBuf};

/// How far the recorded session start can lie from the process start.
///
/// A registry file is named for a process identifier, and an identifier is
/// reused after the process holding it dies. The recorded start is what
/// separates this process from the dead one that held the identifier before:
/// the two started at different times, and a file about the dead process is
/// wrong by however long the machine ran between them.
///
/// Measured on a live machine, a session registered itself between one and nine
/// seconds after its process started, over 119 sessions. The window is set far
/// above that, because it does not have to be tight: for a stale file to pass
/// it, the machine would have to issue every process identifier it has and come
/// back to the same one inside two minutes.
const REGISTRATION_WINDOW_SECS: u64 = 120;

/// Where the session recorded for a running process is read from.
///
/// [`build`](crate::build) is written against this trait rather than against
/// the filesystem, so the rules that assemble and order the report are testable
/// without laying registry files down on disk.
pub trait Registry {
    /// The session `process` belongs to, or `None` when it recorded none.
    fn session_of(&self, process: &ProcessFact) -> Option<SessionId>;
}

/// The registry under a `~/.claude/sessions` folder.
pub struct SessionRegistry {
    /// The folder holding one file per live session, named for its process.
    root: PathBuf,
}

impl SessionRegistry {
    /// Reads sessions from an explicit folder.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Reads sessions from `home/.claude/sessions`.
    #[must_use]
    pub fn for_home(home: &Path) -> Self {
        Self::new(home.join(".claude").join("sessions"))
    }
}

impl Registry for SessionRegistry {
    fn session_of(&self, process: &ProcessFact) -> Option<SessionId> {
        let file = self.root.join(format!("{}.json", process.pid));
        let contents = std::fs::read_to_string(file).ok()?;
        session_in(&contents, process.pid, process.start_time_epoch_secs)
    }
}

/// Reads the session out of one registry file.
///
/// Returns `None` unless the file is about this process and names a session.
/// Every check here fails closed, because naming the wrong session is the worst
/// answer available: nothing in the output would say the name is wrong.
#[must_use]
fn session_in(contents: &str, pid: u32, start_time_epoch_secs: u64) -> Option<SessionId> {
    /// Milliseconds in a second, the unit the recorded start is written in.
    const MILLIS: u64 = 1_000;

    let record: serde_json::Value = serde_json::from_str(contents).ok()?;

    // The name of the file is not evidence. A file that records another
    // process is about another process, whatever it is called.
    if record.get("pid").and_then(serde_json::Value::as_u64)? != u64::from(pid) {
        return None;
    }

    let started = record
        .get("startedAt")
        .and_then(serde_json::Value::as_u64)?
        / MILLIS;
    if started.abs_diff(start_time_epoch_secs) > REGISTRATION_WINDOW_SECS {
        return None;
    }

    SessionId::parse(
        record
            .get("sessionId")
            .and_then(serde_json::Value::as_str)?,
    )
}

#[cfg(test)]
mod tests {
    use super::{session_in, Registry, SessionRegistry, REGISTRATION_WINDOW_SECS};
    use crate::process::ProcessFact;
    use crate::SessionId;
    use std::path::PathBuf;

    const SESSION: &str = "ed84c8c7-0117-4670-936c-98e0f0d2c80b";
    const PID: u32 = 13319;
    const PROCESS_START: u64 = 1_782_902_997;

    /// A registry file in the shape Claude Code writes, taken from a live one.
    fn file(pid: u32, session: &str, started_millis: u64) -> String {
        format!(
            r#"{{"pid":{pid},"sessionId":"{session}","cwd":"/Volumes/HDDRAID/Downloads/temp",
               "startedAt":{started_millis},"procStart":"Wed Jul  1 10:49:57 2026",
               "version":"2.1.197","peerProtocol":1,"kind":"bg","entrypoint":"cli",
               "name":"Identify missing data points","jobId":"ed84c8c7","status":"idle",
               "updatedAt":{started_millis},"statusUpdatedAt":{started_millis}}}"#
        )
    }

    fn id(text: &str) -> SessionId {
        SessionId::parse(text).expect("test id should parse")
    }

    /// A process that started at `PROCESS_START`.
    fn process(pid: u32) -> ProcessFact {
        ProcessFact {
            pid,
            accounting_name: "2.1.197".to_string(),
            exe: Some(PathBuf::from(
                "/Users/u/.local/share/claude/versions/2.1.197",
            )),
            argv: vec!["claude".to_string()],
            cwd: Some(PathBuf::from("/Volumes/HDDRAID/Downloads/temp")),
            uptime_secs: 3_600,
            start_time_epoch_secs: PROCESS_START,
        }
    }

    #[test]
    fn reads_the_session_a_process_recorded() {
        let recorded = file(PID, SESSION, (PROCESS_START + 1) * 1_000);
        assert_eq!(session_in(&recorded, PID, PROCESS_START), Some(id(SESSION)));
    }

    #[test]
    fn a_file_left_by_a_process_that_died_names_no_session() {
        // The identifier was reused. The file records a session that started
        // when the dead process did, which is not when this process started.
        let stale = file(PID, SESSION, (PROCESS_START - 90_000) * 1_000);
        assert_eq!(session_in(&stale, PID, PROCESS_START), None);
    }

    #[test]
    fn a_registration_inside_the_window_still_names_its_session() {
        let slow = file(
            PID,
            SESSION,
            (PROCESS_START + REGISTRATION_WINDOW_SECS) * 1_000,
        );
        assert_eq!(session_in(&slow, PID, PROCESS_START), Some(id(SESSION)));
    }

    #[test]
    fn a_file_naming_another_process_names_no_session() {
        // The name of the file is not evidence. The record inside it is.
        let other = file(PID + 1, SESSION, (PROCESS_START + 1) * 1_000);
        assert_eq!(session_in(&other, PID, PROCESS_START), None);
    }

    #[test]
    fn a_file_that_does_not_parse_names_no_session() {
        // A file caught halfway through being written is truncated, not absent.
        let truncated = r#"{"pid":13319,"sessionId":"ed84c8"#;
        assert_eq!(session_in(truncated, PID, PROCESS_START), None);
        assert_eq!(session_in("", PID, PROCESS_START), None);
    }

    #[test]
    fn a_record_missing_what_it_needs_names_no_session() {
        for incomplete in [
            r#"{"pid":13319,"startedAt":1782902998000}"#,
            r#"{"sessionId":"ed84c8c7-0117-4670-936c-98e0f0d2c80b","startedAt":1782902998000}"#,
            r#"{"pid":13319,"sessionId":"ed84c8c7-0117-4670-936c-98e0f0d2c80b"}"#,
            r#"{"pid":13319,"sessionId":"not-a-session","startedAt":1782902998000}"#,
        ] {
            assert_eq!(
                session_in(incomplete, PID, PROCESS_START),
                None,
                "{incomplete} must name no session"
            );
        }
    }

    #[test]
    fn reads_a_session_from_a_folder_of_registry_files() {
        let folder = tempfile::tempdir().expect("temporary folder");
        std::fs::write(
            folder.path().join(format!("{PID}.json")),
            file(PID, SESSION, (PROCESS_START + 1) * 1_000),
        )
        .expect("registry file");

        let registry = SessionRegistry::new(folder.path().to_path_buf());
        assert_eq!(registry.session_of(&process(PID)), Some(id(SESSION)));
    }

    #[test]
    fn a_process_that_registered_nothing_has_no_session() {
        let folder = tempfile::tempdir().expect("temporary folder");
        let registry = SessionRegistry::new(folder.path().to_path_buf());
        assert_eq!(registry.session_of(&process(PID)), None);
    }
}
