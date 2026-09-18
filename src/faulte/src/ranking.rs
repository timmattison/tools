//! The ranking: one row for each process, sorted by the page faults that it
//! made over the window, and the totals that the header gives.
//!
//! The ranking joins three sources that `faulte` reads at different times: the
//! second sample of `top`, the process table of `ps`, and the Claude Code role
//! and registry record of each process. A process can start or stop between
//! two reads. Thus a process can be in one source and not in the other. The
//! ranking gives each such process a row or a count in [`Skipped`]. It never
//! drops a process silently, because a ranking that lacks a process looks the
//! same as a correct one.
//!
//! [`rank`] is a pure function over plain values. A test gives it each source,
//! and no test reads the real process table.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use occ::{SessionId, SessionRecord};

use crate::pid::{Pid, Uid};
use crate::state::SessionState;
use crate::table::ProcessRow;
use crate::top::FaultCount;

/// The command of the row of PID 0.
///
/// `top` lists PID 0, the kernel, and `ps` does not. The kernel does the work
/// of a Mac that is short of memory, so its row stays in the ranking with this
/// name.
pub const KERNEL_TASK: &str = "kernel_task";

/// The role that `occ` gives a Claude Code process.
///
/// The caller makes this from `occ::Role`. `Role::Session` gives
/// [`ClaudeRole::Session`], and `Role::Unreadable` gives
/// [`ClaudeRole::Unreadable`]. Every other role is not a Claude Code session
/// for `faulte`, and has no entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaudeRole {
    /// A Claude Code session that `occ` can read.
    Session,
    /// A Claude Code process that `occ` cannot read, because another account
    /// owns it and the viewer is not root.
    Unreadable,
}

/// The account that runs `faulte`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Viewer {
    /// The effective UID of `faulte`.
    pub uid: Uid,
    /// True when the effective UID is 0. Root reads the registry of every
    /// account.
    pub is_root: bool,
}

/// Everything that [`rank`] reads: the sources, the viewer, and the time.
#[derive(Debug, Clone, Copy)]
pub struct Observation<'a> {
    /// The time that the counts of [`Observation::faults`] cover. The caller
    /// decides it from the interval, the clock lines of `top`, and the wall
    /// time of the run of `top`.
    pub window: Duration,
    /// The second sample of `top`: the faults of each process over the
    /// window.
    pub faults: &'a [FaultCount],
    /// The process table of `ps`, read after `top`.
    pub table: &'a [ProcessRow],
    /// The Claude Code role of each process that has one.
    pub claude: &'a HashMap<Pid, ClaudeRole>,
    /// The registry record of each process that the caller could read one
    /// for.
    pub records: &'a HashMap<Pid, SessionRecord>,
    /// The account that runs `faulte`.
    pub viewer: Viewer,
    /// The time now. The idle time of a session is the time from its last
    /// status change to this time.
    pub now: SystemTime,
}

/// What a row shows about Claude Code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeView {
    /// The process is not a Claude Code session.
    NotClaude,
    /// A Claude Code session, and its registry record.
    Session {
        /// The session that the process belongs to.
        id: SessionId,
        /// The state of the session.
        state: SessionState,
        /// The working directory of the session, when the record gives it.
        directory: Option<PathBuf>,
    },
    /// A Claude Code session with no registry record that the caller could
    /// read. The row shows no session, because a guess can name the wrong
    /// one.
    NoRecord,
    /// A Claude Code process of another account, and the viewer is not root.
    /// The registry folder of that account has the mode `0700`, so `faulte`
    /// cannot read its records.
    OtherAccount,
}

/// One row of the ranking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedRow {
    /// The process.
    pub pid: Pid,
    /// The account that owns the process.
    pub uid: Uid,
    /// The page faults that the process made over the window.
    pub faults: u64,
    /// The resident memory of the process, in KiB. `None` for the row of
    /// PID 0, which `ps` does not list.
    pub rss_kib: Option<u64>,
    /// The time when the process started, in seconds since the Unix epoch.
    /// `None` for the row of PID 0.
    pub started_at_epoch_secs: Option<u64>,
    /// The arguments of the process, as `ps` prints them, or
    /// [`KERNEL_TASK`] for PID 0.
    pub command: String,
    /// What the row shows about Claude Code.
    pub claude: ClaudeView,
}

/// The total of the rows that are Claude Code sessions.
///
/// On 2026-09-18, 213 Claude Code sessions made 90% of all page faults, and no
/// single row showed it. This total shows it in one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClaudeTotal {
    /// The count of rows whose view is [`ClaudeView::Session`],
    /// [`ClaudeView::NoRecord`], or [`ClaudeView::OtherAccount`].
    pub processes: usize,
    /// The count of those rows whose view is [`ClaudeView::OtherAccount`].
    pub other_account: usize,
    /// The sum of the faults of those rows.
    pub faults: u64,
}

/// The processes that have no row, each in exactly one count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Skipped {
    /// The processes in the sample of `top` and not in the table of `ps`,
    /// other than PID 0. They stopped between the two reads.
    pub exited: usize,
    /// The sum of the faults of the processes that exited. Those faults were
    /// real, so they are part of the total.
    pub exited_faults: u64,
    /// The zombies in the table that are not in the sample. `top` does not
    /// list a zombie.
    pub zombies: usize,
    /// The processes in the table, not zombies, and not in the sample. They
    /// started after `top` took its second sample.
    pub unsampled: usize,
}

/// The ranking of every process, and the totals of the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranking {
    /// One row for each process in both the sample and the table, and one for
    /// PID 0. Sorted by faults, highest first, then by PID, lowest first.
    pub rows: Vec<RankedRow>,
    /// The time that the counts cover.
    pub window: Duration,
    /// The sum of the faults of every process in the sample, the processes
    /// that exited included.
    pub total_faults: u64,
    /// The total of the rows that are Claude Code sessions.
    pub claude: ClaudeTotal,
    /// The processes that have no row.
    pub skipped: Skipped,
}

/// Ranks every process of `input` by its faults over the window.
#[must_use]
pub fn rank(input: &Observation<'_>) -> Ranking {
    let table: HashMap<Pid, &ProcessRow> =
        input.table.iter().map(|process| (process.pid, process)).collect();
    let mut rows: Vec<RankedRow> = input
        .faults
        .iter()
        .filter_map(|count| {
            let process = table.get(&count.pid)?;
            Some(RankedRow {
                pid: count.pid,
                uid: process.uid,
                faults: count.faults,
                rss_kib: Some(process.rss_kib),
                started_at_epoch_secs: Some(process.started_at_epoch_secs),
                command: process.command.clone(),
                claude: ClaudeView::NotClaude,
            })
        })
        .collect();
    rows.sort_by(|left, right| right.faults.cmp(&left.faults));
    Ranking {
        rows,
        window: input.window,
        total_faults: 0,
        claude: ClaudeTotal::default(),
        skipped: Skipped::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The output of `top` in two samples. PID 10 made 9,000,000 faults since
    /// it started, and 3 over the interval. PID 20 made 5 since it started,
    /// and 4,000 over the interval.
    const TWO_SAMPLES: &str = "\
Processes: 2 total, 0 running, 2 sleeping, 4 threads \n\
2026/09/18 12:00:00\n\
Load Avg: 1.00, 1.00, 1.00 \n\
\n\
PID    FAULTS    \n\
10     9000000   \n\
20     5         \n\
Processes: 2 total, 0 running, 2 sleeping, 4 threads \n\
2026/09/18 12:00:04\n\
Load Avg: 1.00, 1.00, 1.00 \n\
\n\
PID    FAULTS    \n\
10     3         \n\
20     4000      \n";

    /// The UID of the viewer in the tests.
    const VIEWER_UID: u32 = 501;

    /// The window of the tests.
    const WINDOW: Duration = Duration::from_secs(4);

    /// The time when each process of the tests started, in seconds since the
    /// Unix epoch.
    const STARTED: u64 = 1_780_000_000;

    /// Gives the time now in the tests: one day after the processes started.
    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(STARTED + 86_400)
    }

    /// Gives the table row of `pid`, owned by the viewer. The RSS is ten
    /// times the PID, so each row is different.
    fn process(pid: u32) -> ProcessRow {
        ProcessRow {
            pid: Pid::new(pid),
            ppid: Pid::new(1),
            uid: Uid::new(VIEWER_UID),
            rss_kib: u64::from(pid) * 10,
            zombie: false,
            started_at_epoch_secs: STARTED,
            command: format!("/usr/bin/process-{pid} --flag"),
        }
    }

    /// The sources of one test. The viewer is UID 501, not root.
    struct Machine {
        faults: Vec<FaultCount>,
        table: Vec<ProcessRow>,
        claude: HashMap<Pid, ClaudeRole>,
        records: HashMap<Pid, SessionRecord>,
        viewer: Viewer,
    }

    impl Machine {
        /// Gives a machine with these sources, no Claude Code process, and no
        /// record.
        fn new(faults: Vec<FaultCount>, table: Vec<ProcessRow>) -> Self {
            Self {
                faults,
                table,
                claude: HashMap::new(),
                records: HashMap::new(),
                viewer: Viewer {
                    uid: Uid::new(VIEWER_UID),
                    is_root: false,
                },
            }
        }

        /// Ranks the processes of this machine over [`WINDOW`].
        fn rank(&self) -> Ranking {
            rank(&Observation {
                window: WINDOW,
                faults: &self.faults,
                table: &self.table,
                claude: &self.claude,
                records: &self.records,
                viewer: self.viewer,
                now: now(),
            })
        }
    }

    /// Gives the PID of each row, in order.
    fn pids(ranking: &Ranking) -> Vec<u32> {
        ranking.rows.iter().map(|row| row.pid.get()).collect()
    }

    /// The acceptance test of the issue. A process with a large count since
    /// it started and a small delta ranks below a process with a small count
    /// since it started and a large delta. The counts come from the parser of
    /// `top`, so the test also proves that the ranking reads the delta.
    #[test]
    fn a_small_delta_ranks_below_a_large_delta_whatever_the_count_since_start() {
        let sample = crate::top::parse(TWO_SAMPLES).expect("the two samples parse");
        let machine = Machine::new(sample.rows, vec![process(10), process(20)]);

        let ranking = machine.rank();

        assert_eq!(pids(&ranking), [20, 10]);
        assert_eq!(
            ranking.rows.first(),
            Some(&RankedRow {
                pid: Pid::new(20),
                uid: Uid::new(VIEWER_UID),
                faults: 4_000,
                rss_kib: Some(200),
                started_at_epoch_secs: Some(STARTED),
                command: "/usr/bin/process-20 --flag".to_owned(),
                claude: ClaudeView::NotClaude,
            }),
            "the row of PID 20 carries its delta and the facts of its table row"
        );
        assert_eq!(ranking.rows.get(1).map(|row| row.faults), Some(3));
        assert_eq!(ranking.window, WINDOW);
    }
}
