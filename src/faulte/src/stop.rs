//! The stop of `faulte kill`: the question that the person answers, the check
//! immediately before each signal, and the sequence of the two signals.
//!
//! A stop is not reversible, and the plan can be minutes old when the person
//! answers. Thus the question and the check are pure functions over plain
//! values, and the sequence reads this Mac through one trait. No unit test
//! signals a real process.

use occ::{SessionRecord, SessionStatus};

use crate::plan::Candidate;
use crate::table::ProcessRow;

/// The one short answer that confirms.
const SHORT_YES: &str = "y";

/// The one long answer that confirms.
const LONG_YES: &str = "yes";

/// Tells whether `answer` confirms the question `Stop N sessions? [y/N]`.
///
/// Only `y` and `yes` confirm, in any case, after the spaces come off. The
/// issue states that rule, and no flag skips the question. `None` is the end
/// of the input, which is no answer at all.
///
/// The comparison reads ASCII alone. Text of another script that looks like
/// `yes` is not `yes`, and a stop is not reversible.
#[must_use]
pub fn confirms(answer: Option<&str>) -> bool {
    answer.is_some_and(|text| {
        let answer = text.trim();
        answer.eq_ignore_ascii_case(SHORT_YES) || answer.eq_ignore_ascii_case(LONG_YES)
    })
}

/// What the check immediately before a signal decided about one candidate.
///
/// The plan can be minutes old when the person answers the question. Each
/// variant other than [`Recheck::Proceed`] is a fact that the plan could not
/// know, and each one stops the signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recheck {
    /// Nothing about the session changed. The signal goes.
    Proceed,
    /// The process is gone. Its PID has no row, or its row is a zombie.
    Exited,
    /// The PID has a row of another process. The process of the plan exited,
    /// and the operating system gave its number to a new process.
    PidReused,
    /// The session is not the idle session that the plan read. It has no
    /// record now, or another status, or another time of the last status
    /// change.
    StatusChanged,
    /// The session has a live descendant. It runs a tool or a shell now,
    /// which rule 3 of the plan refuses.
    DescendantStarted,
}

/// Reads `candidate` again, immediately before `faulte` signals it.
///
/// `fresh_table` and `fresh_record` come from a read that is newer than the
/// plan. The person takes time to answer the question, and a session that the
/// person started to use again in that time must survive.
///
/// The order of the tests is the order of the answers. The table answers
/// first, because a PID that another process took answers nothing about the
/// session that the plan named, and a record under such a PID is about a
/// session that is gone.
#[must_use]
pub fn recheck(
    candidate: &Candidate,
    fresh_table: &[ProcessRow],
    fresh_record: Option<&SessionRecord>,
) -> Recheck {
    // A zombie is not a live process. It stopped already, and its parent did
    // not collect its exit status yet, so no signal reaches it.
    let Some(process) = fresh_table
        .iter()
        .find(|process| process.pid == candidate.row.pid && !process.zombie)
    else {
        return Recheck::Exited;
    };
    if process.started_at_epoch_secs != candidate.started_at_epoch_secs {
        return Recheck::PidReused;
    }
    let still_idle = fresh_record.is_some_and(|record| {
        matches!(record.status, Some(SessionStatus::Idle))
            && record.status_changed_at == candidate.status_changed_at
    });
    if !still_idle {
        return Recheck::StatusChanged;
    }
    Recheck::Proceed
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    use occ::{SessionId, SessionStatus};

    use crate::pid::{Pid, Uid};
    use crate::ranking::{ClaudeView, RankedRow};
    use crate::state::SessionState;

    /// The time now in the tests, in seconds since the Unix epoch.
    const NOW: u64 = 1_780_000_000;

    /// The time when each process of the tests started, in seconds since the
    /// Unix epoch: eight days ago.
    const STARTED: u64 = NOW - 8 * 86_400;

    /// The time since the session of the tests became idle, in seconds.
    const IDLE_SECONDS: u64 = 3_600;

    /// The UID of the account that runs the tests.
    const VIEWER_UID: u32 = 501;

    /// The PID of the parent of every process of the tests: `launchd`.
    const LAUNCHD_PID: u32 = 1;

    /// The working directory of a session in the tests.
    const DIRECTORY: &str = "/Volumes/SamsungSSDs/code/tools";

    /// Gives the time now in the tests.
    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(NOW)
    }

    /// Gives the time when the session of the tests became idle.
    fn changed_at() -> SystemTime {
        now() - Duration::from_secs(IDLE_SECONDS)
    }

    /// Gives the session of the process `pid`. Each PID gives a different
    /// session, so an assertion names the session that it expects.
    fn session_id(pid: u32) -> SessionId {
        SessionId::parse(&format!("d3b0d921-f0a1-41fc-b309-{pid:012}"))
            .expect("the text of the test session is a UUID")
    }

    /// Gives the candidate of the process `pid`, which the plan selected.
    fn candidate(pid: u32) -> Candidate {
        Candidate {
            row: RankedRow {
                pid: Pid::new(pid),
                uid: Uid::new(VIEWER_UID),
                faults: 1_000,
                rss_kib: Some(831_488),
                started_at_epoch_secs: Some(STARTED),
                command: format!("claude --process {pid}"),
                claude: ClaudeView::Session {
                    id: session_id(pid),
                    state: SessionState::Idle {
                        for_: Some(Duration::from_secs(IDLE_SECONDS)),
                    },
                    directory: Some(PathBuf::from(DIRECTORY)),
                    status_changed_at: Some(changed_at()),
                },
            },
            session: session_id(pid),
            started_at_epoch_secs: STARTED,
            status_changed_at: Some(changed_at()),
        }
    }

    /// Gives the table row of the process `pid`, whose parent is `ppid`.
    fn process(pid: u32, ppid: u32) -> ProcessRow {
        ProcessRow {
            pid: Pid::new(pid),
            ppid: Pid::new(ppid),
            uid: Uid::new(VIEWER_UID),
            rss_kib: 831_488,
            zombie: false,
            started_at_epoch_secs: STARTED,
            command: format!("claude --process {pid}"),
        }
    }

    /// Gives the registry record of the session of `pid`, which became idle
    /// at `changed_at`.
    fn record(pid: u32, changed_at: Option<SystemTime>) -> SessionRecord {
        SessionRecord {
            session: session_id(pid),
            status: Some(SessionStatus::Idle),
            status_changed_at: changed_at,
            directory: Some(PathBuf::from(DIRECTORY)),
        }
    }

    /// A process that is gone is never signalled. Its PID has no row of the
    /// fresh table, or its row is a zombie. A zombie stopped already, and its
    /// parent did not collect its exit status yet.
    ///
    /// The table decides, and never the signal. A signal to a PID that no
    /// process holds does nothing, but the operating system gives that number
    /// to a new process, and a signal then stops the wrong one.
    #[test]
    fn a_process_that_is_gone_is_not_signalled() {
        let candidate = candidate(30);
        let zombie = ProcessRow {
            zombie: true,
            ..process(30, LAUNCHD_PID)
        };
        let record = record(30, Some(changed_at()));
        let cases = [
            ("no row of the PID", vec![process(31, LAUNCHD_PID)]),
            ("a zombie row of the PID", vec![zombie]),
        ];

        for (table_holds, table) in cases {
            assert_eq!(
                recheck(&candidate, &table, Some(&record)),
                Recheck::Exited,
                "the table holds {table_holds}"
            );
        }
    }

    /// A PID that another process holds is never signalled. The operating
    /// system gives the number of a process that exited to a new process, and
    /// the plan can be minutes old when the person answers the question.
    ///
    /// A PID alone is no identity. The start time of the row says which
    /// process holds the number now, and the signal goes to the number.
    #[test]
    fn a_pid_that_another_process_took_is_not_signalled() {
        let candidate = candidate(30);
        let reused = ProcessRow {
            started_at_epoch_secs: NOW - 60,
            command: "/usr/bin/vim notes.txt".to_owned(),
            ..process(30, LAUNCHD_PID)
        };
        let record = record(30, Some(changed_at()));

        assert_eq!(
            recheck(&candidate, &[reused], Some(&record)),
            Recheck::PidReused,
            "the row of the PID states another start time"
        );
    }

    /// A session whose status changed is never signalled. The acceptance
    /// criteria demand this test by name: a session that the person started to
    /// use again in the time before the answer must survive.
    ///
    /// Each of these facts says that the session is not the idle session that
    /// the plan read: no record at all, a status that is not idle, and another
    /// time of the last status change. A time that no record gives is no
    /// answer, on either side of the comparison, so two absent times are not
    /// one time.
    #[test]
    fn a_session_whose_status_changed_is_not_signalled() {
        let candidate = candidate(30);
        let table = [process(30, LAUNCHD_PID)];
        let idle = record(30, Some(changed_at()));
        let with_status = |status| SessionRecord {
            status,
            ..idle.clone()
        };
        let busy = with_status(Some(SessionStatus::Busy));
        let waiting = with_status(Some(SessionStatus::Waiting));
        let shell = with_status(Some(SessionStatus::Other("shell".to_owned())));
        let no_status = with_status(None);
        let one_second_later = record(30, Some(changed_at() + Duration::from_secs(1)));
        let no_time = record(30, None);
        let cases: [(&str, Option<&SessionRecord>); 7] = [
            ("no record of the session", None),
            ("the status busy", Some(&busy)),
            ("the status waiting", Some(&waiting)),
            ("the status shell", Some(&shell)),
            ("no status", Some(&no_status)),
            ("a change one second later", Some(&one_second_later)),
            ("no time of the change", Some(&no_time)),
        ];

        for (registry_gives, fresh_record) in cases {
            assert_eq!(
                recheck(&candidate, &table, fresh_record),
                Recheck::StatusChanged,
                "the registry gives {registry_gives}"
            );
        }
    }

    /// A candidate that carries no time of the last status change is never
    /// signalled, and a record that gives no such time does not make one.
    ///
    /// Rule 2 of the plan gives no such candidate: an idle time that no record
    /// states is not long enough. The check states the rule of its own,
    /// because two absent times compare as equal and say nothing at all.
    #[test]
    fn a_candidate_with_no_time_of_the_change_is_not_signalled() {
        let candidate = Candidate {
            status_changed_at: None,
            ..candidate(30)
        };
        let table = [process(30, LAUNCHD_PID)];

        assert_eq!(
            recheck(&candidate, &table, Some(&record(30, None))),
            Recheck::StatusChanged,
            "two absent times are not one time"
        );
    }

    /// A session that did not change proceeds: its PID holds the same process
    /// that the plan read, the registry still says that it is idle, the time
    /// of the last status change is the same, and it started no process.
    #[test]
    fn a_session_that_did_not_change_proceeds() {
        let candidate = candidate(30);
        let table = [process(30, LAUNCHD_PID), process(31, LAUNCHD_PID)];
        let record = record(30, Some(changed_at()));

        assert_eq!(
            recheck(&candidate, &table, Some(&record)),
            Recheck::Proceed,
            "nothing about the session changed"
        );
    }

    /// Only `y` and `yes` confirm, in any case, after the spaces come off.
    ///
    /// Every other answer stops nothing: the end of the input, no text, a
    /// word that holds `yes` inside it, the letters of `yes` apart, and text
    /// of another script. The issue states this rule, and no flag skips the
    /// question.
    #[test]
    fn only_y_and_yes_confirm() {
        let cases = [
            (Some("y"), true),
            (Some("Y"), true),
            (Some("yes"), true),
            (Some("YES"), true),
            (Some("YeS"), true),
            (Some("  yes  "), true),
            (Some("\ty\n"), true),
            (Some(""), false),
            (Some("   "), false),
            (Some("n"), false),
            (Some("no"), false),
            (Some("N"), false),
            (Some("Y E S"), false),
            (Some("yes please"), false),
            (Some("yep"), false),
            (Some("ok"), false),
            (Some("1"), false),
            (Some("ja"), false),
            (Some("ｙｅｓ"), false),
            (Some("はい"), false),
            (None, false),
        ];

        for (answer, confirmed) in cases {
            assert_eq!(confirms(answer), confirmed, "the answer {answer:?}");
        }
    }
}
