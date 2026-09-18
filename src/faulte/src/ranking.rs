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

use std::collections::{BTreeMap, HashMap};
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

/// The PID of the kernel.
const KERNEL_PID: Pid = Pid::new(0);

/// The account that owns the kernel: root.
const KERNEL_UID: Uid = Uid::new(0);

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

impl Ranking {
    /// Gives the rate of `faults` over the window, in faults per second.
    ///
    /// The rate is the signal of this tool. The counter of a process since it
    /// started is not: an old process ranks high on that counter whatever it
    /// does now.
    #[must_use]
    pub fn faults_per_second(&self, faults: u64) -> f64 {
        if self.window.is_zero() {
            return 0.0;
        }
        faults as f64 / self.window.as_secs_f64()
    }

    /// Gives the share of all faults that `faults` is, as a fraction from 0
    /// to 1.
    ///
    /// All faults are [`Ranking::total_faults`], so the faults of the
    /// processes that exited count. When the total is zero, every share is
    /// zero.
    #[must_use]
    pub fn share(&self, faults: u64) -> f64 {
        if self.total_faults == 0 {
            return 0.0;
        }
        faults as f64 / self.total_faults as f64
    }
}

/// Ranks every process of `input` by its faults over the window.
///
/// Each PID of the sample and of the table is one process: it has a row, or it
/// is in exactly one count of [`Skipped`]. Neither `top` nor `ps` prints a PID
/// twice, and the faults of a PID that the sample lists twice add up, so a
/// source that breaks that rule cannot make one process into two.
#[must_use]
pub fn rank(input: &Observation<'_>) -> Ranking {
    let mut sampled: BTreeMap<Pid, u64> = BTreeMap::new();
    for count in input.faults {
        let faults = sampled.entry(count.pid).or_insert(0);
        *faults = faults.saturating_add(count.faults);
    }
    let mut table: HashMap<Pid, &ProcessRow> = HashMap::with_capacity(input.table.len());
    for process in input.table {
        table.entry(process.pid).or_insert(process);
    }
    let mut rows: Vec<RankedRow> = sampled
        .iter()
        .filter_map(|(&pid, &faults)| match table.get(&pid) {
            Some(process) => Some(RankedRow {
                pid,
                uid: process.uid,
                faults,
                rss_kib: Some(process.rss_kib),
                started_at_epoch_secs: Some(process.started_at_epoch_secs),
                command: process.command.clone(),
                claude: ClaudeView::NotClaude,
            }),
            // `ps` does not list the kernel, and the kernel does the work of
            // a Mac that is short of memory. Thus its row says what `top`
            // gives, and no more.
            None if pid == KERNEL_PID => Some(RankedRow {
                pid: KERNEL_PID,
                uid: KERNEL_UID,
                faults,
                rss_kib: None,
                started_at_epoch_secs: None,
                command: KERNEL_TASK.to_owned(),
                claude: ClaudeView::NotClaude,
            }),
            None => None,
        })
        .collect();
    // The PID breaks a tie, so two runs over the same sources agree on the
    // order, whatever order `top` printed.
    rows.sort_by(|left, right| {
        right
            .faults
            .cmp(&left.faults)
            .then_with(|| left.pid.cmp(&right.pid))
    });
    // The faults of a process that exited were real, so the total holds
    // them. A share that leaves them out is too large.
    let total_faults = sampled
        .values()
        .fold(0_u64, |total, faults| total.saturating_add(*faults));
    // A process of the sample that the table lacks stopped between the two
    // reads. Its row would give no owner, no memory and no command, so it is
    // a count and not a row.
    let (exited, exited_faults) = sampled
        .iter()
        .filter(|(pid, _)| **pid != KERNEL_PID && !table.contains_key(pid))
        .fold((0, 0_u64), |(processes, total), (_, faults)| {
            (processes + 1, total.saturating_add(*faults))
        });
    // `top` does not list a zombie, so a zombie of the table is not a
    // process that `top` missed. A process of the table that `top` did not
    // list, and that is not a zombie, started after the second sample.
    let missing = table
        .values()
        .filter(|process| !sampled.contains_key(&process.pid));
    let (zombies, unsampled) = missing.fold((0, 0), |(zombies, unsampled), process| {
        if process.zombie {
            (zombies + 1, unsampled)
        } else {
            (zombies, unsampled + 1)
        }
    });
    Ranking {
        rows,
        window: input.window,
        total_faults,
        claude: ClaudeTotal::default(),
        skipped: Skipped {
            exited,
            exited_faults,
            zombies,
            unsampled,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;

    use occ::SessionStatus;

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

    /// Gives the fault count of `pid`.
    fn count(pid: u32, faults: u64) -> FaultCount {
        FaultCount {
            pid: Pid::new(pid),
            faults,
        }
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

    /// Gives the table row of a zombie: a process that stopped, and whose
    /// parent did not collect its exit status yet.
    fn zombie(pid: u32) -> ProcessRow {
        ProcessRow {
            zombie: true,
            ..process(pid)
        }
    }

    /// The session of a record in the tests.
    const SESSION: &str = "d3b0d921-f0a1-41fc-b309-c11aa30c1173";

    /// The working directory of a record in the tests.
    const DIRECTORY: &str = "/Volumes/SamsungSSDs/code/tools";

    /// Gives the record of a session that is idle for `idle_for`.
    fn idle_record(idle_for: Duration) -> SessionRecord {
        SessionRecord {
            session: session(),
            status: Some(SessionStatus::Idle),
            status_changed_at: Some(now() - idle_for),
            directory: Some(PathBuf::from(DIRECTORY)),
        }
    }

    /// Gives the session of [`SESSION`].
    fn session() -> SessionId {
        SessionId::parse(SESSION).expect("the test ID is a UUID")
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

        /// Gives this machine, with `pid` in the role `role` of Claude Code.
        fn with_claude(mut self, pid: u32, role: ClaudeRole) -> Self {
            self.claude.insert(Pid::new(pid), role);
            self
        }

        /// Gives this machine, with `record` as the registry record of `pid`.
        fn with_record(mut self, pid: u32, record: SessionRecord) -> Self {
            self.records.insert(Pid::new(pid), record);
            self
        }

        /// Ranks the processes of this machine over [`WINDOW`].
        fn rank(&self) -> Ranking {
            self.rank_over(WINDOW)
        }

        /// Ranks the processes of this machine over `window`.
        fn rank_over(&self, window: Duration) -> Ranking {
            rank(&Observation {
                window,
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

    /// Two processes with the same faults rank by PID, lowest first. Thus two
    /// runs over the same sources print the same order, whatever order `top`
    /// printed. The sample lists the tied PIDs from the highest down.
    #[test]
    fn equal_faults_rank_by_pid_lowest_first() {
        let machine = Machine::new(
            vec![count(30, 7), count(12, 7), count(20, 9), count(11, 7)],
            [30, 12, 20, 11].map(process).to_vec(),
        );

        assert_eq!(pids(&machine.rank()), [20, 11, 12, 30]);
    }

    /// PID 99 made 100 faults and exited before `ps` read the table. Those
    /// faults were real. The total holds them, and the share of each row is a
    /// share of that total.
    #[test]
    fn the_total_and_the_shares_include_the_faults_of_processes_that_exited() {
        let machine = Machine::new(vec![count(10, 300), count(99, 100)], vec![process(10)]);

        let ranking = machine.rank();

        assert_eq!(pids(&ranking), [10]);
        assert_eq!(ranking.total_faults, 400);
        assert_eq!(ranking.share(300), 0.75);
        assert_eq!(ranking.share(100), 0.25);
        assert_eq!(ranking.share(400), 1.0);
    }

    /// A sample in which no process made a fault gives a total of zero. Each
    /// share is then zero. A division of zero by zero gives no number, and
    /// the header prints that as `NaN%`.
    #[test]
    fn a_total_of_zero_gives_a_share_of_zero() {
        let machine = Machine::new(vec![count(10, 0), count(20, 0)], vec![process(10)]);

        let ranking = machine.rank();

        assert_eq!(ranking.total_faults, 0);
        assert_eq!(ranking.share(0), 0.0);
    }

    /// The rate is the faults over the window. The window of these sources is
    /// 4 seconds, and `top` asked for 2. On a loaded machine the two samples
    /// were 4 seconds apart, so the caller gives the longer time.
    #[test]
    fn the_rate_is_the_faults_over_the_window() {
        let machine = Machine::new(vec![count(10, 4_000), count(20, 3)], vec![process(10)]);

        let ranking = machine.rank();

        assert_eq!(ranking.faults_per_second(4_000), 1_000.0);
        assert_eq!(ranking.faults_per_second(3), 0.75);
        assert_eq!(ranking.faults_per_second(0), 0.0);
        assert_eq!(
            machine
                .rank_over(Duration::from_millis(2_500))
                .faults_per_second(5),
            2.0
        );
    }

    /// A window of no time measures nothing, so every rate over it is zero. A
    /// division by zero gives no number, and the ranking prints that as `inf`
    /// or `NaN`.
    #[test]
    fn a_window_of_no_time_gives_a_rate_of_zero() {
        let machine = Machine::new(vec![count(10, 4_000)], vec![process(10)]);

        let ranking = machine.rank_over(Duration::ZERO);

        assert_eq!(ranking.faults_per_second(4_000), 0.0);
        assert_eq!(ranking.faults_per_second(0), 0.0);
    }

    /// `top` lists PID 0, and `ps` does not. The kernel does the work of a
    /// Mac that is short of memory, so PID 0 is a row, and never a process
    /// that exited. The row names the kernel, and gives no memory and no
    /// start time, because `ps` gives neither.
    #[test]
    fn pid_zero_is_a_row_of_the_kernel() {
        let machine = Machine::new(vec![count(0, 1_329), count(10, 3)], vec![process(10)]);

        let ranking = machine.rank();

        assert_eq!(pids(&ranking), [0, 10]);
        assert_eq!(
            ranking.rows.first(),
            Some(&RankedRow {
                pid: Pid::new(0),
                uid: Uid::new(0),
                faults: 1_329,
                rss_kib: None,
                started_at_epoch_secs: None,
                command: KERNEL_TASK.to_owned(),
                claude: ClaudeView::NotClaude,
            })
        );
    }

    /// A process of the sample that the table does not hold stopped between
    /// the two reads. It has no row, because the table gives its owner, its
    /// memory and its command. The count says how many such processes there
    /// were, and their faults say what share of the total they made. PID 0 is
    /// not one of them.
    #[test]
    fn a_process_of_the_sample_that_the_table_lacks_exited() {
        let machine = Machine::new(
            vec![
                count(0, 1_000),
                count(10, 500),
                count(99, 40),
                count(98, 60),
            ],
            vec![process(10)],
        );

        let ranking = machine.rank();

        assert_eq!(pids(&ranking), [0, 10]);
        assert_eq!(ranking.skipped.exited, 2);
        assert_eq!(ranking.skipped.exited_faults, 100);
        assert_eq!(ranking.share(ranking.skipped.exited_faults), 0.0625);
    }

    /// `top` does not list a zombie, so a zombie of the table is a count and
    /// not a process that `top` missed. A zombie that `top` did list is
    /// unusual, and it is a row like every other row of the sample.
    #[test]
    fn a_zombie_that_the_sample_lacks_counts_as_a_zombie() {
        let machine = Machine::new(
            vec![count(10, 300), count(51, 7)],
            vec![process(10), zombie(50), zombie(51)],
        );

        let ranking = machine.rank();

        assert_eq!(pids(&ranking), [10, 51]);
        assert_eq!(ranking.skipped.zombies, 1);
        assert_eq!(ranking.skipped.exited, 0);
    }

    /// A process of the table that is not a zombie and that the sample lacks
    /// started after `top` took its second sample. It has no fault count, so
    /// it has no row.
    #[test]
    fn a_process_of_the_table_that_the_sample_lacks_counts_as_unsampled() {
        let machine = Machine::new(
            vec![count(10, 300)],
            vec![process(10), process(60), process(61), zombie(50)],
        );

        let ranking = machine.rank();

        assert_eq!(pids(&ranking), [10]);
        assert_eq!(ranking.skipped.unsampled, 2);
        assert_eq!(ranking.skipped.zombies, 1);
        assert_eq!(ranking.skipped.exited, 0);
    }

    /// The identity that the issue demands: no process is dropped. Over a
    /// mixed input, the rows and the three counts hold each PID of the sample
    /// and of the table exactly once.
    ///
    /// The input lists PID 20 twice in the sample and PID 60 twice in the
    /// table. Neither `top` nor `ps` prints a PID twice, and a ranking that
    /// counts one process twice breaks this identity as surely as a ranking
    /// that drops one. Thus each PID is one process here, and the faults of a
    /// PID that the sample lists twice add up.
    #[test]
    fn every_process_of_the_sample_or_the_table_is_a_row_or_one_skip() {
        let faults = vec![
            count(0, 1_000),
            count(10, 300),
            count(20, 7),
            count(20, 13),
            count(51, 7),
            count(99, 40),
            count(98, 60),
        ];
        let table = vec![
            process(10),
            process(20),
            zombie(50),
            zombie(51),
            process(60),
            process(60),
            process(61),
        ];
        let processes: BTreeSet<u32> = faults
            .iter()
            .map(|count| count.pid.get())
            .chain(table.iter().map(|process| process.pid.get()))
            .collect();

        let ranking = Machine::new(faults, table).rank();

        assert_eq!(processes.len(), 9);
        assert_eq!(pids(&ranking), [0, 10, 20, 51]);
        let skipped = ranking.skipped;
        assert_eq!(
            ranking.rows.len() + skipped.exited + skipped.zombies + skipped.unsampled,
            processes.len(),
            "each of {processes:?} is a row or one skip: {ranking:?}"
        );
        assert_eq!(
            ranking.rows.iter().find(|row| row.pid == Pid::new(20)),
            Some(&RankedRow {
                pid: Pid::new(20),
                uid: Uid::new(VIEWER_UID),
                faults: 20,
                rss_kib: Some(200),
                started_at_epoch_secs: Some(STARTED),
                command: "/usr/bin/process-20 --flag".to_owned(),
                claude: ClaudeView::NotClaude,
            }),
            "the faults of a PID that the sample lists twice add up"
        );
        assert_eq!(
            ranking.rows.iter().map(|row| row.faults).sum::<u64>() + skipped.exited_faults,
            ranking.total_faults,
            "the rows and the processes that exited hold every fault"
        );
    }

    /// A row of a Claude Code process that has a registry record gives the
    /// session, its state at the time now, and its working directory.
    #[test]
    fn a_claude_process_with_a_record_shows_its_session() {
        let idle_for = Duration::from_secs(3 * 3_600 + 12 * 60);
        let machine = Machine::new(vec![count(10, 300)], vec![process(10)])
            .with_claude(10, ClaudeRole::Session)
            .with_record(10, idle_record(idle_for));

        let ranking = machine.rank();

        assert_eq!(
            ranking.rows.first().map(|row| &row.claude),
            Some(&ClaudeView::Session {
                id: session(),
                state: SessionState::Idle {
                    for_: Some(idle_for)
                },
                directory: Some(PathBuf::from(DIRECTORY)),
            })
        );
    }
}
