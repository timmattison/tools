//! Assembling the answer: one row per running session, oldest release first,
//! and a count of every Claude Code process the rows leave out.

use crate::process::{classify, version_of, ProcessFact, Role};
use crate::registry::Registry;
use crate::session::SessionId;
use crate::ClaudeVersion;
use std::path::PathBuf;

/// One running Claude Code session.
#[derive(Debug, Clone)]
pub struct SessionReport {
    /// The process identifier.
    pub pid: u32,
    /// The release the session runs, when it could be read.
    pub version: Option<ClaudeVersion>,
    /// The directory the session works in, when it could be read.
    pub directory: Option<PathBuf>,
    /// The session the process recorded for itself, when it recorded one.
    pub session: Option<SessionId>,
    /// Seconds the session has been open.
    pub uptime_secs: u64,
}

/// Everything one run has to say about a machine.
///
/// The rows are the sessions to show. The counts are the Claude Code processes
/// the rows deliberately leave out, and they are reported so the footer can say
/// what was left out. A tool that quietly drops processes it cannot read would
/// report a clean machine that is not clean.
#[derive(Debug, Clone)]
pub struct Report {
    /// Every running session, oldest release first.
    pub sessions: Vec<SessionReport>,
    /// Processes Claude Code runs for its own account.
    pub support: usize,
    /// Claude Code processes another account owns, which this user cannot read.
    pub unreadable: usize,
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

/// Builds the report: every running session, oldest release first, and a count
/// of what the rows leave out.
///
/// Sessions are ordered by release, oldest first, because a session left on an
/// old release is the one worth acting on. A session whose release could not be
/// read sorts last rather than first, so an unreadable release never
/// impersonates the oldest one. Within a release the longest-open session comes
/// first, and the process identifier settles the rest so that two runs over an
/// unchanged machine agree.
///
/// The support and unreadable counts come from the same pass that selects the
/// rows. One pass decides what each process is, so the counts and the rows can
/// never disagree about a process, and the caller learns everything about the
/// machine from one value.
#[must_use]
pub fn build(facts: &[ProcessFact], registry: &dyn Registry) -> Report {
    let mut report = Report {
        sessions: Vec::new(),
        support: 0,
        unreadable: 0,
    };

    for fact in facts {
        match classify(fact) {
            Role::Session => report.sessions.push(SessionReport {
                pid: fact.pid,
                version: version_of(fact),
                directory: fact.cwd.clone(),
                session: registry.session_of(fact),
                uptime_secs: fact.uptime_secs,
            }),
            Role::Support(_) => report.support += 1,
            Role::Unreadable => report.unreadable += 1,
            Role::SpawnedTool | Role::Unrelated => {}
        }
    }

    report.sessions.sort_by(|left, right| {
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
    report
}

#[cfg(test)]
mod tests {
    use super::{build, format_uptime, SessionReport};
    use crate::registry::Registry;
    use crate::session::SessionId;
    use crate::ProcessFact;
    use std::collections::HashMap;
    use std::path::PathBuf;

    const SESSION_A: &str = "d3b0d921-f0a1-41fc-b309-c11aa30c1173";
    const SESSION_B: &str = "ed84c8c7-0117-4670-936c-98e0f0d2c80b";
    const VERSIONED_ROOT: &str = "/Users/u/.local/share/claude/versions";

    /// Registered sessions held in memory, keyed by process identifier.
    ///
    /// Whether a file really belongs to the process it is named for is decided
    /// in [`crate::registry`] and tested there. What this fake stands in for is
    /// the answer, so that the rules here are testable without a filesystem.
    #[derive(Default)]
    struct FakeRegistry(HashMap<u32, SessionId>);

    impl FakeRegistry {
        fn with(mut self, pid: u32, session: &str) -> Self {
            self.0.insert(
                pid,
                SessionId::parse(session).expect("test id should parse"),
            );
            self
        }
    }

    impl Registry for FakeRegistry {
        fn session_of(&self, process: &ProcessFact) -> Option<SessionId> {
            self.0.get(&process.pid).cloned()
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

    fn named(rows: &[SessionReport]) -> Vec<Option<String>> {
        rows.iter()
            .map(|row| row.session.as_ref().map(SessionId::to_string))
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
        let rows = build(&facts, &FakeRegistry::default()).sessions;

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 1);
    }

    #[test]
    fn counts_what_it_leaves_out() {
        // The footer says what the table does not show, so the count of every
        // kind left out is part of the answer. A machine reported as clean while
        // sixty processes of another account run on it is the failure this
        // prevents.
        let mut daemon = session_fact(3, "2.1.232", "/work", 10);
        daemon.argv = vec![
            "claude".to_string(),
            "daemon".to_string(),
            "run".to_string(),
        ];
        let mut pty_host = session_fact(4, "2.1.232", "/work", 10);
        pty_host.argv = vec!["claude".to_string(), "--bg-pty-host".to_string()];
        let mut other_account = session_fact(5, "2.1.232", "/work", 10);
        other_account.argv = Vec::new();
        let mut tool = session_fact(6, "2.1.232", "/work", 10);
        tool.argv = vec!["ugrep".to_string(), "-G".to_string()];
        let mut unrelated = session_fact(7, "2.1.232", "/work", 10);
        unrelated.exe = Some(PathBuf::from("/usr/bin/vim"));

        let facts = [
            session_fact(1, "2.1.196", "/work", 10),
            session_fact(2, "2.1.232", "/work", 10),
            daemon,
            pty_host,
            other_account,
            tool,
            unrelated,
        ];
        let report = build(&facts, &FakeRegistry::default());

        assert_eq!(report.sessions.len(), 2);
        assert_eq!(report.support, 2);
        assert_eq!(report.unreadable, 1);
    }

    #[test]
    fn counts_nothing_when_nothing_is_left_out() {
        // A machine that runs sessions and nothing else must report no support
        // process and no unreadable one, so the footer stays silent.
        let facts = [
            session_fact(1, "2.1.196", "/a", 10),
            session_fact(2, "2.1.232", "/b", 10),
        ];
        let report = build(&facts, &FakeRegistry::default());

        assert_eq!(report.sessions.len(), 2);
        assert_eq!(report.support, 0);
        assert_eq!(report.unreadable, 0);
    }

    #[test]
    fn orders_releases_oldest_first() {
        let facts = [
            session_fact(1, "2.1.232", "/a", 10),
            session_fact(2, "2.1.99", "/b", 10),
            session_fact(3, "2.1.204", "/c", 10),
            session_fact(4, "2.1.196", "/d", 10),
        ];
        let rows = build(&facts, &FakeRegistry::default()).sessions;
        assert_eq!(releases(&rows), ["2.1.99", "2.1.196", "2.1.204", "2.1.232"]);
    }

    #[test]
    fn an_unreadable_release_sorts_last() {
        // Sorting it first would make it look like the oldest session on the
        // machine, which is the one claim this tool must not get wrong.
        let mut unknown = session_fact(9, "claude", "/a", 10);
        unknown.exe = Some(PathBuf::from("/Users/u/.local/bin/claude"));

        let facts = [unknown, session_fact(1, "2.1.232", "/b", 10)];
        let rows = build(&facts, &FakeRegistry::default()).sessions;

        assert_eq!(releases(&rows), ["2.1.232", "?"]);
    }

    #[test]
    fn the_longest_open_session_leads_its_release() {
        let facts = [
            session_fact(1, "2.1.204", "/a", 100),
            session_fact(2, "2.1.204", "/b", 9_000),
            session_fact(3, "2.1.204", "/c", 500),
        ];
        let rows = build(&facts, &FakeRegistry::default()).sessions;
        assert_eq!(rows.iter().map(|r| r.pid).collect::<Vec<_>>(), [2, 3, 1]);
    }

    #[test]
    fn a_session_is_named_by_what_it_recorded() {
        let facts = [session_fact(1, "2.1.205", "/work", 10)];
        let registry = FakeRegistry::default().with(1, SESSION_A);

        let rows = build(&facts, &registry).sessions;

        assert_eq!(named(&rows), [Some(SESSION_A.to_string())]);
    }

    #[test]
    fn sessions_sharing_a_directory_and_a_release_are_each_named() {
        // The shape that defeats every attempt to reconstruct this: several
        // sessions of one release in one worktree. Each one recorded its own
        // identity, so nothing has to be worked out from the outside.
        let facts = [
            session_fact(1, "2.1.202", "/work", 10),
            session_fact(2, "2.1.202", "/work", 20),
        ];
        let registry = FakeRegistry::default()
            .with(1, SESSION_A)
            .with(2, SESSION_B);

        let rows = build(&facts, &registry).sessions;

        // Row order is by uptime, so the longer-open process 2 comes first.
        assert_eq!(
            named(&rows),
            [Some(SESSION_B.to_string()), Some(SESSION_A.to_string())]
        );
    }

    #[test]
    fn a_session_that_recorded_nothing_is_named_by_nothing() {
        // Seven of 126 live sessions on a real machine had no record. A blank
        // is the whole answer: a guess in its place is wrong more often than
        // it is right, and nothing in the output would say which.
        let facts = [session_fact(1, "2.1.205", "/work", 10)];

        let rows = build(&facts, &FakeRegistry::default()).sessions;

        assert_eq!(named(&rows), [None]);
    }

    #[test]
    fn a_session_without_a_readable_directory_is_still_reported() {
        // A session that will not give up its directory still has a release, a
        // process identifier, and an uptime worth reporting.
        let mut hidden = session_fact(7, "2.1.196", "/work", 42);
        hidden.cwd = None;

        let rows = build(&[hidden], &FakeRegistry::default().with(7, SESSION_A)).sessions;

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 7);
        assert_eq!(rows[0].directory, None);
        assert_eq!(named(&rows), [Some(SESSION_A.to_string())]);
    }
}
