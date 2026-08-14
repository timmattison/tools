//! Assembling the answer: one row per running session, oldest release first.

use crate::process::{classify, version_of, ProcessFact, Role};
use crate::session::{attribute, Session, Transcript};
use crate::ClaudeVersion;
use std::path::{Path, PathBuf};

/// Where recorded transcripts are read from.
///
/// The report is assembled against this trait rather than against the
/// filesystem so that every ordering and attribution rule can be tested with
/// transcripts that would be laborious to lay down on disk.
pub trait Transcripts {
    /// Every transcript recorded for `directory`.
    fn for_directory(&self, directory: &Path) -> Vec<Transcript>;
}

/// One running Claude Code session.
#[derive(Debug, Clone)]
pub struct SessionReport {
    /// The process identifier.
    pub pid: u32,
    /// The release the session runs, when it could be read.
    pub version: Option<ClaudeVersion>,
    /// The directory the session works in, when it could be read.
    pub directory: Option<PathBuf>,
    /// The session the process belongs to.
    pub session: Session,
    /// Seconds the session has been open.
    pub uptime_secs: u64,
}

/// Renders a duration as the two largest units that carry information.
///
/// # Examples
///
/// ```
/// use occ::format_uptime;
///
/// assert_eq!(format_uptime(45), "45s");
/// assert_eq!(format_uptime(3_930), "1h 5m");
/// ```
#[must_use]
pub fn format_uptime(seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    if seconds >= DAY {
        format!("{}d {}h", seconds / DAY, (seconds % DAY) / HOUR)
    } else if seconds >= HOUR {
        format!("{}h {}m", seconds / HOUR, (seconds % HOUR) / MINUTE)
    } else if seconds >= MINUTE {
        format!("{}m {}s", seconds / MINUTE, seconds % MINUTE)
    } else {
        format!("{seconds}s")
    }
}

/// Builds the report: every running session, oldest release first.
///
/// Sessions are ordered by release, oldest first, because a session left on an
/// old release is the one worth acting on. A session whose release could not be
/// read sorts last rather than first, so an unreadable release never
/// impersonates the oldest one. Within a release the longest-open session comes
/// first, and the process identifier settles the rest so that two runs over an
/// unchanged machine agree.
#[must_use]
pub fn build(facts: &[ProcessFact], transcripts: &dyn Transcripts) -> Vec<SessionReport> {
    let sessions: Vec<(&ProcessFact, Option<ClaudeVersion>)> = facts
        .iter()
        .filter(|fact| classify(fact) == Role::Session)
        .map(|fact| (fact, version_of(fact)))
        .collect();

    // A session competes for a transcript only with the sessions sharing its
    // directory and its release, so the peer count is taken over that pair.
    let peers_of = |directory: Option<&Path>, version: Option<&ClaudeVersion>| -> usize {
        sessions
            .iter()
            .filter(|(other, other_version)| {
                other.cwd.as_deref() == directory && other_version.as_ref() == version
            })
            .count()
    };

    let mut rows: Vec<SessionReport> = sessions
        .iter()
        .map(|(fact, version)| {
            let candidates = fact
                .cwd
                .as_deref()
                .map(|directory| transcripts.for_directory(directory))
                .unwrap_or_default();
            let session = attribute(
                &fact.argv,
                version.as_ref(),
                fact.start_time_epoch_secs,
                &candidates,
                peers_of(fact.cwd.as_deref(), version.as_ref()),
            );
            SessionReport {
                pid: fact.pid,
                version: version.clone(),
                directory: fact.cwd.clone(),
                session,
                uptime_secs: fact.uptime_secs,
            }
        })
        .collect();

    rows.sort_by(|left, right| {
        // `None` is the unreadable release, and it sorts last: an unknown
        // release must never be presented as the oldest one on the machine.
        let by_release = match (left.version.as_ref(), right.version.as_ref()) {
            (Some(a), Some(b)) => a.cmp(b),
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, None) => std::cmp::Ordering::Equal,
        };
        by_release
            .then_with(|| right.uptime_secs.cmp(&left.uptime_secs))
            .then_with(|| left.pid.cmp(&right.pid))
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::{build, format_uptime, SessionReport, Transcripts};
    use crate::session::{Session, SessionId, Transcript};
    use crate::{ClaudeVersion, ProcessFact};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    const SESSION_A: &str = "d3b0d921-f0a1-41fc-b309-c11aa30c1173";
    const SESSION_B: &str = "ed84c8c7-0117-4670-936c-98e0f0d2c80b";
    const VERSIONED_ROOT: &str = "/Users/u/.local/share/claude/versions";

    /// Transcripts held in memory, keyed by directory.
    #[derive(Default)]
    struct FakeTranscripts(HashMap<PathBuf, Vec<Transcript>>);

    impl FakeTranscripts {
        fn with(mut self, directory: &str, transcripts: Vec<Transcript>) -> Self {
            self.0.insert(PathBuf::from(directory), transcripts);
            self
        }
    }

    impl Transcripts for FakeTranscripts {
        fn for_directory(&self, directory: &Path) -> Vec<Transcript> {
            self.0.get(directory).cloned().unwrap_or_default()
        }
    }

    fn transcript(session: &str, release: &str, created: u64) -> Transcript {
        Transcript {
            id: SessionId::parse(session).expect("test id should parse"),
            version: ClaudeVersion::parse(release),
            created_epoch_secs: created,
        }
    }

    /// A session process on `release`, working in `directory`.
    fn session_fact(pid: u32, release: &str, directory: &str, uptime_secs: u64) -> ProcessFact {
        ProcessFact {
            pid,
            accounting_name: release.to_string(),
            exe: Some(PathBuf::from(format!("{VERSIONED_ROOT}/{release}"))),
            argv: vec!["claude".to_string()],
            cwd: Some(PathBuf::from(directory)),
            uptime_secs,
            start_time_epoch_secs: 1_000,
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
    fn renders_a_duration_in_its_two_largest_units() {
        assert_eq!(format_uptime(0), "0s");
        assert_eq!(format_uptime(45), "45s");
        assert_eq!(format_uptime(90), "1m 30s");
        assert_eq!(format_uptime(3_600), "1h 0m");
        assert_eq!(format_uptime(3_930), "1h 5m");
        assert_eq!(format_uptime(86_400), "1d 0h");
        // The longest-running session observed on a live machine: 45 days.
        assert_eq!(format_uptime(3_913_140), "45d 6h");
    }

    #[test]
    fn reports_only_sessions() {
        // A support process and a spawned tool both run a Claude Code image and
        // must not appear as sessions.
        let mut daemon = session_fact(2, "2.1.232", "/work", 10);
        daemon.argv = vec![
            "claude".to_string(),
            "daemon".to_string(),
            "run".to_string(),
        ];
        let mut tool = session_fact(3, "2.1.232", "/work", 10);
        tool.argv = vec!["ugrep".to_string(), "-G".to_string()];

        let facts = [session_fact(1, "2.1.232", "/work", 10), daemon, tool];
        let rows = build(&facts, &FakeTranscripts::default());

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 1);
    }

    #[test]
    fn orders_releases_oldest_first() {
        let facts = [
            session_fact(1, "2.1.232", "/a", 10),
            session_fact(2, "2.1.99", "/b", 10),
            session_fact(3, "2.1.204", "/c", 10),
            session_fact(4, "2.1.196", "/d", 10),
        ];
        let rows = build(&facts, &FakeTranscripts::default());
        assert_eq!(releases(&rows), ["2.1.99", "2.1.196", "2.1.204", "2.1.232"]);
    }

    #[test]
    fn an_unreadable_release_sorts_last() {
        // Sorting it first would make it look like the oldest session on the
        // machine, which is the one claim this tool must not get wrong.
        let mut unknown = session_fact(9, "claude", "/a", 10);
        unknown.exe = Some(PathBuf::from("/Users/u/.local/bin/claude"));

        let facts = [unknown, session_fact(1, "2.1.232", "/b", 10)];
        let rows = build(&facts, &FakeTranscripts::default());

        assert_eq!(releases(&rows), ["2.1.232", "?"]);
    }

    #[test]
    fn the_longest_open_session_leads_its_release() {
        let facts = [
            session_fact(1, "2.1.204", "/a", 100),
            session_fact(2, "2.1.204", "/b", 9_000),
            session_fact(3, "2.1.204", "/c", 500),
        ];
        let rows = build(&facts, &FakeTranscripts::default());
        assert_eq!(rows.iter().map(|r| r.pid).collect::<Vec<_>>(), [2, 3, 1]);
    }

    #[test]
    fn a_lone_session_in_a_directory_is_attributed() {
        let facts = [session_fact(1, "2.1.205", "/work", 10)];
        let transcripts =
            FakeTranscripts::default().with("/work", vec![transcript(SESSION_A, "2.1.205", 5_000)]);

        let rows = build(&facts, &transcripts);

        assert_eq!(
            rows[0].session,
            Session::Matched(SessionId::parse(SESSION_A).expect("id"))
        );
    }

    #[test]
    fn sessions_of_one_release_in_one_directory_stay_ambiguous() {
        // The real shape this guards: several sessions of the same release in
        // one worktree. Neither may be given the other's id.
        let facts = [
            session_fact(1, "2.1.202", "/work", 10),
            session_fact(2, "2.1.202", "/work", 20),
        ];
        let transcripts = FakeTranscripts::default().with(
            "/work",
            vec![
                transcript(SESSION_A, "2.1.202", 5_000),
                transcript(SESSION_B, "2.1.202", 5_000),
            ],
        );

        let rows = build(&facts, &transcripts);

        for row in &rows {
            assert_eq!(
                row.session,
                Session::Ambiguous {
                    candidates: 2,
                    peers: 2
                }
            );
        }
    }

    #[test]
    fn a_release_separates_two_sessions_sharing_a_directory() {
        // Two sessions in one directory, on different releases: each is pinned
        // to its own transcript, so both are attributed.
        let facts = [
            session_fact(1, "2.1.202", "/work", 10),
            session_fact(2, "2.1.218", "/work", 20),
        ];
        let transcripts = FakeTranscripts::default().with(
            "/work",
            vec![
                transcript(SESSION_A, "2.1.202", 5_000),
                transcript(SESSION_B, "2.1.218", 5_000),
            ],
        );

        let rows = build(&facts, &transcripts);
        let attributed: Vec<&Session> = rows.iter().map(|row| &row.session).collect();

        assert_eq!(
            attributed,
            [
                &Session::Matched(SessionId::parse(SESSION_A).expect("id")),
                &Session::Matched(SessionId::parse(SESSION_B).expect("id")),
            ]
        );
    }

    #[test]
    fn a_session_without_a_readable_directory_is_still_reported() {
        // The directory is what attribution needs, not what the session is. A
        // session that will not give up its directory still has a release, a
        // process identifier, and an uptime worth reporting.
        let mut hidden = session_fact(7, "2.1.196", "/work", 42);
        hidden.cwd = None;

        let rows = build(&[hidden], &FakeTranscripts::default());

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 7);
        assert_eq!(rows[0].directory, None);
        assert_eq!(rows[0].session, Session::Unknown);
    }
}
