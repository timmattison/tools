//! The whole pipeline through the public interface: process facts and the
//! session records on disk in, an ordered report out.

use occ::report::SessionReport;
use occ::{build, ProcessFact, SessionRegistry};
use std::path::PathBuf;

const OLD_SESSION: &str = "11111111-1111-4111-8111-111111111111";
const NEW_SESSION: &str = "22222222-2222-4222-8222-222222222222";
const OTHER_SESSION: &str = "33333333-3333-4333-8333-333333333333";

/// When every process in these fixtures started, in seconds since the epoch.
const PROCESS_START: u64 = 1_782_902_997;

/// A `~/.claude` tree laid out under a temporary home.
///
/// Each test gets its own home from `tempfile`, so concurrent runs cannot read
/// or truncate one another's fixtures.
struct Machine {
    home: tempfile::TempDir,
}

impl Machine {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().expect("temporary home"),
        }
    }

    /// Records `session` for `pid`, as a live session does for itself.
    fn record(&self, pid: u32, session: &str, directory: &str, started_epoch_secs: u64) {
        let folder = self.home.path().join(".claude").join("sessions");
        std::fs::create_dir_all(&folder).expect("sessions folder");
        let record = format!(
            r#"{{"pid":{pid},"sessionId":"{session}","cwd":"{directory}",
               "startedAt":{},"version":"2.1.232","kind":"interactive",
               "name":"a session","status":"idle"}}"#,
            started_epoch_secs * 1_000
        );
        std::fs::write(folder.join(format!("{pid}.json")), record).expect("session record");
    }

    fn registry(&self) -> SessionRegistry {
        SessionRegistry::for_home(self.home.path())
    }
}

/// A session process on `release`, working in `directory`.
fn session(pid: u32, release: &str, directory: &str, argv: &[&str]) -> ProcessFact {
    ProcessFact {
        pid,
        accounting_name: release.to_string(),
        exe: Some(PathBuf::from(format!(
            "/Users/u/.local/share/claude/versions/{release}"
        ))),
        argv: argv.iter().map(|a| (*a).to_string()).collect(),
        cwd: Some(PathBuf::from(directory)),
        uptime_secs: 60,
        start_time_epoch_secs: PROCESS_START,
    }
}

fn releases(rows: &[SessionReport]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            row.version
                .as_ref()
                .map_or_else(|| "?".to_string(), |v| v.as_str().to_string())
        })
        .collect()
}

fn named(rows: &[SessionReport]) -> Vec<Option<String>> {
    rows.iter()
        .map(|row| row.session.as_ref().map(ToString::to_string))
        .collect()
}

#[test]
fn reports_running_sessions_oldest_release_first() {
    let machine = Machine::new();
    machine.record(100, OLD_SESSION, "/work/one", PROCESS_START + 1);
    machine.record(200, NEW_SESSION, "/work/two", PROCESS_START + 1);

    let facts = [
        session(200, "2.1.232", "/work/two", &["claude"]),
        session(100, "2.1.196", "/work/one", &["claude"]),
    ];

    let rows = build(&facts, &machine.registry()).sessions;

    assert_eq!(releases(&rows), ["2.1.196", "2.1.232"]);
    assert_eq!(rows[0].pid, 100);
    assert_eq!(rows[1].pid, 200);
}

#[test]
fn names_each_session_from_the_record_it_wrote() {
    let machine = Machine::new();
    machine.record(100, OLD_SESSION, "/work/one", PROCESS_START + 1);
    machine.record(200, NEW_SESSION, "/work/two", PROCESS_START + 1);

    let facts = [
        session(100, "2.1.196", "/work/one", &["claude"]),
        session(200, "2.1.232", "/work/two", &["claude"]),
    ];

    let rows = build(&facts, &machine.registry()).sessions;

    assert_eq!(
        named(&rows),
        [Some(OLD_SESSION.to_string()), Some(NEW_SESSION.to_string())]
    );
}

#[test]
fn sessions_sharing_a_directory_and_a_release_are_each_named() {
    // Nothing outside these processes separates them: one directory, one
    // release, one uptime. Each recorded its own identity, so each is named.
    let machine = Machine::new();
    machine.record(100, OLD_SESSION, "/work", PROCESS_START + 1);
    machine.record(101, NEW_SESSION, "/work", PROCESS_START + 1);
    machine.record(102, OTHER_SESSION, "/work", PROCESS_START + 1);

    let facts = [
        session(100, "2.1.204", "/work", &["claude"]),
        session(101, "2.1.204", "/work", &["claude"]),
        session(102, "2.1.204", "/work", &["claude"]),
    ];

    let rows = build(&facts, &machine.registry()).sessions;

    assert_eq!(
        named(&rows),
        [
            Some(OLD_SESSION.to_string()),
            Some(NEW_SESSION.to_string()),
            Some(OTHER_SESSION.to_string()),
        ]
    );
}

#[test]
fn a_record_left_by_a_dead_process_names_nothing() {
    // The identifier was reused. The record is about the process that held it
    // before, and giving this process that session's id would be a claim
    // nothing in the report contradicts.
    let machine = Machine::new();
    machine.record(100, OLD_SESSION, "/work", PROCESS_START - 90_000);

    let facts = [session(100, "2.1.196", "/work", &["claude"])];

    let rows = build(&facts, &machine.registry()).sessions;

    assert_eq!(rows.len(), 1);
    assert_eq!(named(&rows), [None]);
}

#[test]
fn a_session_that_recorded_nothing_is_still_reported() {
    // Seven of 126 live sessions on a real machine had no record. The release
    // is what this tool reports, and it is readable whatever the record says.
    let machine = Machine::new();

    let facts = [session(100, "2.1.196", "/work", &["claude"])];

    let rows = build(&facts, &machine.registry()).sessions;

    assert_eq!(rows.len(), 1);
    assert_eq!(releases(&rows), ["2.1.196"]);
    assert_eq!(named(&rows), [None]);
}

#[test]
fn support_processes_and_spawned_tools_are_left_out() {
    let machine = Machine::new();
    machine.record(100, OLD_SESSION, "/work/one", PROCESS_START + 1);

    let facts = [
        session(100, "2.1.196", "/work/one", &["claude"]),
        session(101, "2.1.196", "/work/one", &["claude", "daemon", "run"]),
        session(
            102,
            "2.1.196",
            "/work/one",
            &["claude bg-spare", "--bg-spare"],
        ),
        session(103, "2.1.196", "/work/one", &["ugrep", "-G"]),
    ];

    let rows = build(&facts, &machine.registry()).sessions;

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pid, 100);
}
