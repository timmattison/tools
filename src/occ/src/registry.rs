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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// The status that a session recorded, from the `status` field of its file.
///
/// Only an idle session is safe to stop, so the exact value decides whether a
/// session is active. An unknown value keeps its text. Thus a status that
/// Claude Code adds later never becomes [`SessionStatus::Idle`] by mistake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    /// The session runs no turn. It waits for its next prompt.
    Idle,
    /// The session runs a turn.
    Busy,
    /// The session waits for an answer from its user.
    Waiting,
    /// A value that this crate does not know, exactly as the file records it.
    ///
    /// The value `shell` is one such value on a live machine.
    Other(String),
}

impl SessionStatus {
    /// Reads the text of a `status` field.
    fn from_recorded(text: &str) -> Self {
        match text {
            "idle" => Self::Idle,
            "busy" => Self::Busy,
            "waiting" => Self::Waiting,
            other => Self::Other(other.to_string()),
        }
    }
}

/// What one registry file records about a live session.
///
/// Only the session is necessary. The other fields are the facts that `faulte`
/// uses to decide whether a session is active. A field that is absent, or that
/// holds a value of the wrong type, gives `None`. It never removes the session
/// from the record, because `occ` reports a session without these facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    /// The session that the process belongs to.
    pub session: SessionId,
    /// The status of the session, from the `status` field.
    pub status: Option<SessionStatus>,
    /// When the status last changed, from the `statusUpdatedAt` field.
    ///
    /// The file gives this time in milliseconds since the Unix epoch.
    pub status_changed_at: Option<SystemTime>,
    /// The working directory of the session, from the `cwd` field.
    pub directory: Option<PathBuf>,
}

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
        record_in(&contents, process.pid, process.start_time_epoch_secs)
            .map(|record| record.session)
    }
}

/// Reads the record out of one registry file.
///
/// Returns `None` unless the file is about this process and names a session.
/// Every check here fails closed, because naming the wrong session is the worst
/// answer available: nothing in the output would say the name is wrong.
#[must_use]
fn record_in(contents: &str, pid: u32, start_time_epoch_secs: u64) -> Option<SessionRecord> {
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

    let session = SessionId::parse(
        record
            .get("sessionId")
            .and_then(serde_json::Value::as_str)?,
    )?;

    // The facts below describe the session. A file without one of them still
    // names its session. Thus each fact that cannot be read gives `None`, and
    // the record stays.
    Some(SessionRecord {
        session,
        status: record
            .get("status")
            .and_then(serde_json::Value::as_str)
            .map(SessionStatus::from_recorded),
        status_changed_at: record
            .get("statusUpdatedAt")
            .and_then(serde_json::Value::as_u64)
            .and_then(|millis| UNIX_EPOCH.checked_add(Duration::from_millis(millis))),
        directory: record
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        record_in, Registry, SessionRecord, SessionRegistry, SessionStatus,
        REGISTRATION_WINDOW_SECS,
    };
    use crate::process::ProcessFact;
    use crate::SessionId;
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    const SESSION: &str = "ed84c8c7-0117-4670-936c-98e0f0d2c80b";
    const PID: u32 = 13319;
    const PROCESS_START: u64 = 1_782_902_997;
    const DIRECTORY: &str = "/Volumes/HDDRAID/Downloads/temp";

    /// A registry file in the shape Claude Code writes, taken from a live one.
    fn file(pid: u32, session: &str, started_millis: u64) -> String {
        format!(
            r#"{{"pid":{pid},"sessionId":"{session}","cwd":"{DIRECTORY}",
               "startedAt":{started_millis},"procStart":"Wed Jul  1 10:49:57 2026",
               "version":"2.1.197","peerProtocol":1,"kind":"bg","entrypoint":"cli",
               "name":"Identify missing data points","jobId":"ed84c8c7","status":"idle",
               "updatedAt":{started_millis},"statusUpdatedAt":{started_millis}}}"#
        )
    }

    /// A registry file of `SESSION` for `PID` in `status`, taken from a live one.
    ///
    /// The status changed at `status_changed_millis`. The start and the last
    /// update of the file are at other times, so a test sees which field the
    /// reader takes.
    fn file_in_status(status: &str, status_changed_millis: u64) -> String {
        let started_millis = (PROCESS_START + 1) * 1_000;
        let updated_millis = status_changed_millis + 5_000;
        format!(
            r#"{{"pid":{PID},"sessionId":"{SESSION}","cwd":"{DIRECTORY}",
               "startedAt":{started_millis},"procStart":"Wed Jul  1 10:49:57 2026",
               "version":"2.1.276","peerProtocol":1,"kind":"interactive","entrypoint":"cli",
               "status":"{status}","updatedAt":{updated_millis},
               "statusUpdatedAt":{status_changed_millis}}}"#
        )
    }

    fn id(text: &str) -> SessionId {
        SessionId::parse(text).expect("test id should parse")
    }

    /// The session of the record in `contents`, which is all that `occ` reports.
    fn session_in(contents: &str, pid: u32, start_time_epoch_secs: u64) -> Option<SessionId> {
        record_in(contents, pid, start_time_epoch_secs).map(|record| record.session)
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
            cwd: Some(PathBuf::from(DIRECTORY)),
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
    fn the_record_gives_the_status_its_time_and_the_directory() {
        let changed_millis = (PROCESS_START + 600) * 1_000 + 250;
        let recorded = file_in_status("idle", changed_millis);
        assert_eq!(
            record_in(&recorded, PID, PROCESS_START),
            Some(SessionRecord {
                session: id(SESSION),
                status: Some(SessionStatus::Idle),
                status_changed_at: Some(UNIX_EPOCH + Duration::from_millis(changed_millis)),
                directory: Some(PathBuf::from(DIRECTORY)),
            })
        );
    }

    #[test]
    fn each_recorded_status_gives_its_own_value() {
        // The value `shell` is on a live machine and this crate does not know
        // it. It keeps its text, so that no reader takes it for `idle`.
        for (recorded, expected) in [
            ("idle", SessionStatus::Idle),
            ("busy", SessionStatus::Busy),
            ("waiting", SessionStatus::Waiting),
            ("shell", SessionStatus::Other("shell".to_string())),
        ] {
            let file = file_in_status(recorded, (PROCESS_START + 1) * 1_000);
            assert_eq!(
                record_in(&file, PID, PROCESS_START).and_then(|record| record.status),
                Some(expected),
                "the status {recorded:?}"
            );
        }
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
