//! The whole pipeline through the public interface: process facts and
//! transcripts on disk in, an ordered report out.

use occ::report::SessionReport;
use occ::scan::encode_directory;
use occ::session::Session;
use occ::{build, ProcessFact, ProjectTranscripts};
use std::path::{Path, PathBuf};

const OLD_SESSION: &str = "11111111-1111-4111-8111-111111111111";
const NEW_SESSION: &str = "22222222-2222-4222-8222-222222222222";
const NAMED_SESSION: &str = "33333333-3333-4333-8333-333333333333";

/// A `projects` tree laid out under a temporary root.
///
/// Each test gets its own root from `tempfile`, so concurrent runs cannot read
/// or truncate one another's fixtures.
struct Machine {
    root: tempfile::TempDir,
}

impl Machine {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().expect("temporary root"),
        }
    }

    fn projects(&self) -> PathBuf {
        self.root.path().join("projects")
    }

    /// Writes a transcript recording `directory` and `release`.
    fn record(&self, directory: &str, session: &str, release: &str) {
        let folder = self.projects().join(encode_directory(Path::new(directory)));
        std::fs::create_dir_all(&folder).expect("transcript folder");
        let line =
            format!("{{\"type\":\"user\",\"cwd\":\"{directory}\",\"version\":\"{release}\"}}\n");
        std::fs::write(folder.join(format!("{session}.jsonl")), line).expect("transcript");
    }

    fn transcripts(&self) -> ProjectTranscripts {
        ProjectTranscripts::new(self.projects())
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
        // Older than any transcript the fixtures write, so every transcript is
        // a candidate on creation time.
        start_time_epoch_secs: 0,
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

#[test]
fn reports_running_sessions_oldest_release_first() {
    let machine = Machine::new();
    machine.record("/work/one", OLD_SESSION, "2.1.196");
    machine.record("/work/two", NEW_SESSION, "2.1.232");

    let facts = [
        session(200, "2.1.232", "/work/two", &["claude"]),
        session(100, "2.1.196", "/work/one", &["claude"]),
    ];

    let rows = build(&facts, &machine.transcripts());

    assert_eq!(releases(&rows), ["2.1.196", "2.1.232"]);
    assert_eq!(rows[0].pid, 100);
    assert_eq!(rows[1].pid, 200);
}

#[test]
fn attributes_each_session_to_its_own_transcript() {
    let machine = Machine::new();
    machine.record("/work/one", OLD_SESSION, "2.1.196");
    machine.record("/work/two", NEW_SESSION, "2.1.232");

    let facts = [
        session(100, "2.1.196", "/work/one", &["claude"]),
        session(200, "2.1.232", "/work/two", &["claude"]),
    ];

    let rows = build(&facts, &machine.transcripts());
    let named: Vec<String> = rows
        .iter()
        .map(|row| match &row.session {
            Session::Named(id) | Session::Matched(id) => id.to_string(),
            other => format!("{other:?}"),
        })
        .collect();

    assert_eq!(named, [OLD_SESSION, NEW_SESSION]);
}

#[test]
fn a_command_line_session_id_wins_over_the_transcripts() {
    // The named id is authoritative even where the directory holds transcripts
    // that would otherwise be matched.
    let machine = Machine::new();
    machine.record("/work/one", OLD_SESSION, "2.1.196");

    let facts = [session(
        100,
        "2.1.196",
        "/work/one",
        &["claude", "--session-id", NAMED_SESSION],
    )];

    let rows = build(&facts, &machine.transcripts());

    assert_eq!(
        rows[0].session,
        Session::Named(occ::SessionId::parse(NAMED_SESSION).expect("id"))
    );
}

#[test]
fn support_processes_and_spawned_tools_are_left_out() {
    let machine = Machine::new();
    machine.record("/work/one", OLD_SESSION, "2.1.196");

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

    let rows = build(&facts, &machine.transcripts());

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pid, 100);
}

#[test]
fn a_transcript_from_another_directory_is_never_borrowed() {
    // The folder name is lossy, so `/work/a.b` and `/work/a-b` share one folder.
    // A session in one of them must not be given the other's id.
    let machine = Machine::new();
    machine.record("/work/a.b", OLD_SESSION, "2.1.196");

    let facts = [session(100, "2.1.196", "/work/a-b", &["claude"])];

    let rows = build(&facts, &machine.transcripts());

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].session,
        Session::Unknown,
        "the transcript belongs to /work/a.b, not /work/a-b"
    );
}
