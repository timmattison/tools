//! The stop of `faulte kill`: the question that the person answers, the check
//! immediately before each signal, and the sequence of the two signals.
//!
//! A stop is not reversible, and the plan can be minutes old when the person
//! answers. Thus the question and the check are pure functions over plain
//! values, and the sequence reads this Mac through one trait. No unit test
//! signals a real process.

use std::time::Duration;

use occ::{SessionRecord, SessionStatus};

use crate::machine::{Machine, MachineError, Signal};
use crate::plan::{pids_with_a_live_descendant, Candidate};
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
    // A time that no record gives is no answer. Two absent times compare as
    // equal, so the candidate must carry a time for the comparison to say
    // anything.
    let still_idle = match (candidate.status_changed_at, fresh_record) {
        (Some(changed_at), Some(record)) => {
            matches!(record.status, Some(SessionStatus::Idle))
                && record.status_changed_at == Some(changed_at)
        }
        _ => false,
    };
    if !still_idle {
        return Recheck::StatusChanged;
    }
    // The walk of rule 3, over the fresh table. A session that started a tool
    // call or a shell since the plan is active now.
    if pids_with_a_live_descendant(fresh_table).contains(&candidate.row.pid) {
        return Recheck::DescendantStarted;
    }
    Recheck::Proceed
}

/// What one run of [`stop`] did.
///
/// Every candidate that the caller gave is in exactly one of these lists. A
/// stop is not reversible, and the person must read what happened to each
/// session that the plan named.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StopReport {
    /// The candidates that the check refused, each with its reason. None of
    /// them got a signal.
    pub skipped: Vec<(Candidate, Recheck)>,
    /// The sessions that were gone after `SIGTERM`.
    pub stopped: Vec<Candidate>,
    /// The sessions that were gone after `SIGKILL`.
    pub killed: Vec<Candidate>,
    /// The sessions that were still the same process after `SIGKILL`.
    pub survived: Vec<Candidate>,
    /// The candidates that `faulte` could not signal, or could not read
    /// again, each with what the operating system said.
    pub failed: Vec<(Candidate, MachineError)>,
}

/// Stops each session of `candidates`, and reports what happened to each one.
///
/// The sequence is one function, because each step reads what the step before
/// it learned and no caller can take one of them away:
///
/// 1. A fresh process table and a fresh record of each session. Every
///    candidate that [`recheck`] refuses goes to [`StopReport::skipped`], and
///    it gets no signal.
/// 2. `SIGTERM` to every target that proceeds. Claude Code closes its
///    transcript when it gets that signal.
/// 3. A wait of `poll`, then a read of the table, until every target is gone
///    or `grace` ends. A target is gone when its PID has no row, has a zombie
///    row, or has a row of another start time.
/// 4. `SIGKILL` to each target that is still the same process, then one more
///    wait and one more read, to learn what that signal did.
///
/// `grace` is 30 seconds in a real run, and `poll` is one second. The grace
/// period is long because of the Mac that this tool is for: a Mac that is
/// short of memory is slow to page a process in, and a process handles no
/// signal until it is in memory. A short grace period sends `SIGKILL` to a
/// session that was on its way to closing its transcript.
///
/// A read of the process table that fails ends the sequence. `faulte` cannot
/// tell a process that exited from a PID that another process took, so it
/// signals nothing more, and each target that is left goes to
/// [`StopReport::failed`] with the reason.
#[must_use]
pub fn stop(
    machine: &dyn Machine,
    candidates: &[Candidate],
    grace: Duration,
    poll: Duration,
) -> StopReport {
    let mut report = StopReport::default();
    let Ok(table) = machine.process_table() else {
        return report;
    };
    let mut targets: Vec<&Candidate> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let record = machine.record_for(
            candidate.row.pid,
            candidate.row.uid,
            candidate.started_at_epoch_secs,
        );
        match recheck(candidate, &table, record.as_ref()) {
            Recheck::Proceed => targets.push(candidate),
            refusal => report.skipped.push((candidate.clone(), refusal)),
        }
    }
    let mut targets = signal_each(machine, targets, Signal::Terminate, &mut report);
    for _ in 0..waits(grace, poll) {
        machine.sleep(poll);
        let Ok(table) = machine.process_table() else {
            return report;
        };
        targets.retain(|candidate| {
            let gone = is_gone(candidate, &table);
            if gone {
                report.stopped.push((*candidate).clone());
            }
            !gone
        });
    }
    if targets.is_empty() {
        return report;
    }
    let targets = signal_each(machine, targets, Signal::Kill, &mut report);
    if targets.is_empty() {
        return report;
    }
    machine.sleep(poll);
    let Ok(table) = machine.process_table() else {
        return report;
    };
    for candidate in targets {
        if is_gone(candidate, &table) {
            report.killed.push(candidate.clone());
        } else {
            report.survived.push(candidate.clone());
        }
    }
    report
}

/// Sends `signal` to each target of `targets`, and gives back the targets that
/// got it.
///
/// A signal that the operating system refuses says nothing about the other
/// targets of the same run, so the run goes on. Two accounts share this Mac,
/// and a refusal of one signal is a fact about one process.
///
/// The target that got no signal goes into [`StopReport::failed`] with the
/// reason, and it leaves the run. A process that refused `SIGTERM` refuses
/// `SIGKILL` for the same reason, and a report that named it a session which
/// survived would hide the reason that the person needs.
fn signal_each<'a>(
    machine: &dyn Machine,
    targets: Vec<&'a Candidate>,
    signal: Signal,
    report: &mut StopReport,
) -> Vec<&'a Candidate> {
    targets
        .into_iter()
        .filter(
            |candidate| match machine.signal(candidate.row.pid, signal) {
                Ok(()) => true,
                Err(error) => {
                    report.failed.push(((*candidate).clone(), error));
                    false
                }
            },
        )
        .collect()
}

/// Gives the count of the waits that the grace period holds.
///
/// The count is `grace` divided by `poll`, rounded up. The sequence has no
/// clock: it waits, it reads the table, and it counts. Thus a machine of a
/// test that waits for no time makes the same count of reads as this Mac, and
/// the sequence ends in every test.
///
/// A `poll` of no time gives one wait, because a division by no time gives no
/// number. The grace period then ends at the first read of the table.
fn waits(grace: Duration, poll: Duration) -> u128 {
    if poll.is_zero() {
        return u128::from(!grace.is_zero());
    }
    grace.as_nanos().div_ceil(poll.as_nanos())
}

/// Tells whether the process of `candidate` is gone from `table`.
///
/// A process is gone when its PID has no row, when its row is a zombie, or
/// when its row states another start time. The last one is a PID that the
/// operating system gave to a new process, and the session of the candidate is
/// gone in that case as well.
///
/// [`recheck`] asks the same question before the first signal, and it tells
/// the three answers apart to name the reason. Here one answer is enough: the
/// sequence signals a process that is still there, and nothing else.
fn is_gone(candidate: &Candidate, table: &[ProcessRow]) -> bool {
    !table.iter().any(|process| {
        process.pid == candidate.row.pid
            && !process.zombie
            && process.started_at_epoch_secs == candidate.started_at_epoch_secs
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::path::PathBuf;
    use std::time::SystemTime;

    use occ::{SessionId, SessionStatus};

    use crate::duration::Span;
    use crate::pid::{Pid, Uid};
    use crate::ranking::{ClaudeRole, ClaudeView, RankedRow, Viewer};
    use crate::state::SessionState;
    use crate::top::TopSample;
    use crate::vm::{SwapUsage, VmCounters};

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

    /// Gives the table row of a zombie process `pid`, whose parent is `ppid`.
    /// A zombie stopped already, so it is not a live descendant.
    fn zombie(pid: u32, ppid: u32) -> ProcessRow {
        ProcessRow {
            zombie: true,
            ..process(pid, ppid)
        }
    }

    /// A session that started a process is never signalled. Rule 3 of the plan
    /// refuses a session with a live descendant, and a session can start a
    /// tool call or a shell between the plan and the answer.
    ///
    /// The walk is the walk of the plan, so a descendant two levels down
    /// counts, and a zombie between the session and a live grandchild does not
    /// hide that grandchild. A zombie child on its own stopped already, so it
    /// holds nothing and the signal goes.
    #[test]
    fn a_session_that_started_a_process_is_not_signalled() {
        let candidate = candidate(30);
        let record = record(30, Some(changed_at()));
        let check = |table: &[ProcessRow]| recheck(&candidate, table, Some(&record));

        assert_eq!(
            check(&[process(30, LAUNCHD_PID), process(31, 30)]),
            Recheck::DescendantStarted,
            "the session started one process"
        );
        assert_eq!(
            check(&[process(30, LAUNCHD_PID), zombie(31, 30), process(32, 31)]),
            Recheck::DescendantStarted,
            "a zombie child does not hide a live grandchild"
        );
        assert_eq!(
            check(&[process(30, LAUNCHD_PID), zombie(31, 30)]),
            Recheck::Proceed,
            "a zombie child stopped already, and it is no descendant that is alive"
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

    /// The grace period of a test that wants one read of the table before
    /// `SIGKILL`, and the time between two reads of the table.
    const ONE_POLL: Duration = Duration::from_secs(1);

    /// The machine of the stop tests. It answers each read that the sequence
    /// makes, and it records every signal and every wait.
    ///
    /// A test states one process table for each read that it cares about. The
    /// last table of the list answers every read after it, so a sequence that
    /// reads the table more times than the test states gives an assertion and
    /// not a panic.
    struct FakeMachine {
        /// What each read of the process table gives, in order.
        tables: RefCell<VecDeque<Result<Vec<ProcessRow>, MachineError>>>,
        /// The registry record under each PID.
        records: HashMap<Pid, SessionRecord>,
        /// The reason why a signal to a PID fails.
        refusals: HashMap<Pid, MachineError>,
        /// Each signal that the sequence sent, in order.
        signals: RefCell<Vec<(Pid, Signal)>>,
        /// The count of the waits that the sequence made.
        sleeps: RefCell<usize>,
    }

    impl FakeMachine {
        /// Gives a machine whose reads of the process table give `tables`, in
        /// order, and whose reads never fail.
        fn reading(tables: Vec<Vec<ProcessRow>>) -> Self {
            Self {
                tables: RefCell::new(tables.into_iter().map(Ok).collect()),
                records: HashMap::new(),
                refusals: HashMap::new(),
                signals: RefCell::new(Vec::new()),
                sleeps: RefCell::new(0),
            }
        }

        /// Gives this machine, with `record` under `pid` in the registry.
        fn with_record(mut self, pid: u32, record: SessionRecord) -> Self {
            self.records.insert(Pid::new(pid), record);
            self
        }

        /// Gives this machine, whose read of the process table after the
        /// tables that it holds gives `error`.
        fn then_failing(self, error: MachineError) -> Self {
            self.tables.borrow_mut().push_back(Err(error));
            self
        }

        /// Gives this machine, with `error` as the answer to every signal to
        /// `pid`.
        fn refusing(mut self, pid: u32, error: MachineError) -> Self {
            self.refusals.insert(Pid::new(pid), error);
            self
        }

        /// Gives each signal that the sequence sent, in order.
        fn signals(&self) -> Vec<(Pid, Signal)> {
            self.signals.borrow().clone()
        }

        /// Gives the count of the waits that the sequence made.
        fn sleeps(&self) -> usize {
            *self.sleeps.borrow()
        }
    }

    impl Machine for FakeMachine {
        fn process_table(&self) -> Result<Vec<ProcessRow>, MachineError> {
            let mut tables = self.tables.borrow_mut();
            if tables.len() > 1 {
                tables.pop_front()
            } else {
                tables.front().cloned()
            }
            .expect("the test states a process table for each read")
        }

        fn record_for(
            &self,
            pid: Pid,
            _owner: Uid,
            _started_at_epoch_secs: u64,
        ) -> Option<SessionRecord> {
            self.records.get(&pid).cloned()
        }

        fn signal(&self, pid: Pid, signal: Signal) -> Result<(), MachineError> {
            self.signals.borrow_mut().push((pid, signal));
            match self.refusals.get(&pid) {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }

        fn sleep(&self, _how_long: Duration) {
            *self.sleeps.borrow_mut() += 1;
        }

        fn sample_faults(&self, _interval: Span) -> Result<(TopSample, Duration), MachineError> {
            unreachable!("the stop sequence samples no faults");
        }

        fn claude_roles(&self) -> HashMap<Pid, ClaudeRole> {
            unreachable!("the stop sequence reads no role of Claude Code");
        }

        fn vm_counters(&self) -> Result<VmCounters, MachineError> {
            unreachable!("the stop sequence reads no counter of the virtual memory");
        }

        fn swap_usage(&self) -> Result<SwapUsage, MachineError> {
            unreachable!("the stop sequence reads no swap file");
        }

        fn page_size(&self) -> u64 {
            unreachable!("the stop sequence reads no size of a page");
        }

        fn viewer(&self) -> Viewer {
            unreachable!("the stop sequence reads no account of a viewer");
        }

        fn account_name(&self, uid: Uid) -> Option<String> {
            unreachable!("the stop sequence asked for the name of the account {uid}");
        }

        fn now(&self) -> SystemTime {
            unreachable!("the stop sequence reads no clock");
        }
    }

    /// Gives the machine of a stop test: it reads `tables`, and its registry
    /// holds an idle record of each session of `sessions`.
    fn machine_of(sessions: &[u32], tables: Vec<Vec<ProcessRow>>) -> FakeMachine {
        sessions.iter().fold(
            FakeMachine::reading(tables),
            |machine: FakeMachine, pid: &u32| {
                machine.with_record(*pid, record(*pid, Some(changed_at())))
            },
        )
    }

    /// A session that is gone after `SIGTERM` is a session that `faulte`
    /// stopped, and it gets no `SIGKILL`.
    ///
    /// `SIGTERM` is the first signal, because Claude Code closes its
    /// transcript when it gets that signal. A process that is gone holds
    /// nothing, so a second signal has nothing to reach.
    #[test]
    fn a_session_that_goes_after_sigterm_is_stopped() {
        let candidate = candidate(30);
        let machine = machine_of(&[30], vec![vec![process(30, LAUNCHD_PID)], Vec::new()]);

        let report = stop(&machine, &[candidate.clone()], ONE_POLL, ONE_POLL);

        assert_eq!(
            report,
            StopReport {
                stopped: vec![candidate],
                ..StopReport::default()
            },
            "the session was gone at the first read after SIGTERM"
        );
        assert_eq!(
            machine.signals(),
            vec![(Pid::new(30), Signal::Terminate)],
            "SIGTERM goes once, and SIGKILL does not go at all"
        );
    }

    /// A candidate that the check refuses gets no signal at all, and the
    /// report names it with the reason that the check gave.
    ///
    /// The plan can be minutes old when the person answers the question. A
    /// session that the person started to use again in that time must survive,
    /// and the person must read which sessions `faulte` left alone and why.
    ///
    /// Each refused candidate carries its own reason, and a candidate that
    /// proceeds beside them is still signalled.
    #[test]
    fn a_candidate_that_the_check_refuses_is_skipped_and_never_signalled() {
        let exited = candidate(30);
        let target = candidate(40);
        let busy = candidate(50);
        let machine = FakeMachine::reading(vec![
            vec![process(40, LAUNCHD_PID), process(50, LAUNCHD_PID)],
            Vec::new(),
        ])
        .with_record(40, record(40, Some(changed_at())))
        .with_record(
            50,
            SessionRecord {
                status: Some(SessionStatus::Busy),
                ..record(50, Some(changed_at()))
            },
        );

        let report = stop(
            &machine,
            &[exited.clone(), target.clone(), busy.clone()],
            ONE_POLL,
            ONE_POLL,
        );

        assert_eq!(
            report,
            StopReport {
                skipped: vec![(exited, Recheck::Exited), (busy, Recheck::StatusChanged)],
                stopped: vec![target],
                ..StopReport::default()
            },
            "each refused candidate carries the reason of the check"
        );
        assert_eq!(
            machine.signals(),
            vec![(Pid::new(40), Signal::Terminate)],
            "no signal reaches a candidate that the check refused"
        );
    }

    /// The grace period of a test that wants three reads of the table before
    /// `SIGKILL`.
    const THREE_POLLS: Duration = Duration::from_secs(3);

    /// A target that is still the same process at the end of the grace period
    /// gets `SIGKILL`, and it is a session that `faulte` killed when it then
    /// goes.
    ///
    /// The order of the two signals is the whole point of the grace period.
    /// Claude Code closes its transcript when it gets `SIGTERM`, and the Mac
    /// that this tool is for is slow to page a process in. A process handles no
    /// signal until it is in memory, so `SIGKILL` goes last and it goes late.
    #[test]
    fn a_target_that_is_alive_after_the_grace_period_gets_sigkill() {
        let candidate = candidate(30);
        let alive = vec![process(30, LAUNCHD_PID)];
        let machine = machine_of(
            &[30],
            vec![
                alive.clone(),
                alive.clone(),
                alive.clone(),
                alive,
                Vec::new(),
            ],
        );

        let report = stop(&machine, &[candidate.clone()], THREE_POLLS, ONE_POLL);

        assert_eq!(
            report,
            StopReport {
                killed: vec![candidate],
                ..StopReport::default()
            },
            "the session went after SIGKILL and not after SIGTERM"
        );
        assert_eq!(
            machine.signals(),
            vec![
                (Pid::new(30), Signal::Terminate),
                (Pid::new(30), Signal::Kill)
            ],
            "SIGTERM goes first, and SIGKILL goes after it"
        );
        assert_eq!(
            machine.sleeps(),
            4,
            "three waits of the grace period, and one after SIGKILL"
        );
    }

    /// A target that is gone during the grace period gets no `SIGKILL`, and it
    /// counts as a session that `faulte` stopped.
    ///
    /// A process is gone in three spellings: its PID has no row, its row is a
    /// zombie, or its row states another start time. The last one is the one
    /// that the issue demands by name. The session of the plan exited, the
    /// operating system gave its number to a new process, and a signal to that
    /// number stops a process that nobody asked to stop.
    #[test]
    fn a_target_that_is_gone_in_the_grace_period_gets_no_sigkill() {
        let candidate = candidate(30);
        let reused = ProcessRow {
            started_at_epoch_secs: NOW - 60,
            command: "/usr/bin/vim notes.txt".to_owned(),
            ..process(30, LAUNCHD_PID)
        };
        let cases = [
            ("no row of the PID", Vec::new()),
            ("a zombie row of the PID", vec![zombie(30, LAUNCHD_PID)]),
            ("a row of another process", vec![reused]),
        ];

        for (table_holds, gone) in cases {
            let machine = machine_of(&[30], vec![vec![process(30, LAUNCHD_PID)], gone]);

            let report = stop(&machine, &[candidate.clone()], THREE_POLLS, ONE_POLL);

            assert_eq!(
                report,
                StopReport {
                    stopped: vec![candidate.clone()],
                    ..StopReport::default()
                },
                "the table holds {table_holds}"
            );
            assert_eq!(
                machine.signals(),
                vec![(Pid::new(30), Signal::Terminate)],
                "no SIGKILL goes when the table holds {table_holds}"
            );
        }
    }

    /// A target that is still the same process after `SIGKILL` is a session
    /// that survived, and the report says so.
    ///
    /// The kernel stops a process that gets `SIGKILL`, so this answer is rare.
    /// A process that is in an uninterruptible call of the kernel is one, and
    /// a Mac that pages a process in from swap is slow at every step. The
    /// person asked `faulte` to stop that session, so a report that says
    /// nothing about it is a report that lies.
    #[test]
    fn a_target_that_is_the_same_process_after_sigkill_survived() {
        let candidate = candidate(30);
        let machine = machine_of(&[30], vec![vec![process(30, LAUNCHD_PID)]]);

        let report = stop(&machine, &[candidate.clone()], ONE_POLL, ONE_POLL);

        assert_eq!(
            report,
            StopReport {
                survived: vec![candidate],
                ..StopReport::default()
            },
            "the process holds the same PID and the same start time after SIGKILL"
        );
        assert_eq!(
            machine.signals(),
            vec![
                (Pid::new(30), Signal::Terminate),
                (Pid::new(30), Signal::Kill)
            ],
            "both signals went, and neither one stopped the process"
        );
    }

    /// What the operating system says when the account of the viewer does not
    /// own the process.
    const NOT_PERMITTED: &str = "Operation not permitted (os error 1)";

    /// Gives the error of a signal to `pid` that the operating system refused.
    fn refused(pid: u32) -> MachineError {
        MachineError::KernelRead {
            call: format!("kill({pid}, SIGTERM)"),
            reason: NOT_PERMITTED.to_owned(),
        }
    }

    /// A signal that fails puts its target in the report with the reason, and
    /// the other targets still get their signal.
    ///
    /// Two accounts share this Mac, and `faulte` under `sudo` stops the
    /// sessions of every account. A signal that the operating system refuses
    /// says nothing about the other targets of the same run, so the run goes
    /// on. The target that got no signal gets no second one either: the
    /// sequence never sends `SIGKILL` to a process that refused `SIGTERM`.
    #[test]
    fn a_signal_that_fails_does_not_stop_the_rest() {
        let refused_target = candidate(30);
        let target = candidate(40);
        let machine = machine_of(
            &[30, 40],
            vec![
                vec![process(30, LAUNCHD_PID), process(40, LAUNCHD_PID)],
                vec![process(30, LAUNCHD_PID)],
            ],
        )
        .refusing(30, refused(30));

        let report = stop(
            &machine,
            &[refused_target.clone(), target.clone()],
            ONE_POLL,
            ONE_POLL,
        );

        assert_eq!(
            report,
            StopReport {
                stopped: vec![target],
                failed: vec![(refused_target, refused(30))],
                ..StopReport::default()
            },
            "the run names the target that got no signal, and it stops the other one"
        );
        assert_eq!(
            machine.signals(),
            vec![
                (Pid::new(30), Signal::Terminate),
                (Pid::new(40), Signal::Terminate)
            ],
            "the target that refused SIGTERM gets no SIGKILL"
        );
    }

    /// Gives the error of a read of the process table that failed.
    fn unreadable_table() -> MachineError {
        MachineError::CommandDidNotStart {
            program: crate::table::PROGRAM.to_owned(),
            reason: "No such file or directory (os error 2)".to_owned(),
        }
    }

    /// A read of the process table that fails ends the sequence, and every
    /// target that is left goes into the report with the reason.
    ///
    /// The table is the only source that tells an exit from a PID that another
    /// process took. Without it `faulte` knows nothing about its targets, and a
    /// signal to a PID that it cannot read stops a process that nobody asked to
    /// stop. So the sequence signals nothing more.
    ///
    /// The read fails at three places, and each one ends the sequence: before
    /// the first signal, inside the grace period, and after `SIGKILL`.
    #[test]
    fn a_table_that_faulte_cannot_read_ends_the_sequence() {
        let first = candidate(30);
        let second = candidate(40);
        let both = [first.clone(), second.clone()];
        let alive = vec![process(30, LAUNCHD_PID), process(40, LAUNCHD_PID)];
        let terminate = vec![
            (Pid::new(30), Signal::Terminate),
            (Pid::new(40), Signal::Terminate),
        ];

        let before = machine_of(&[30, 40], Vec::new()).then_failing(unreadable_table());
        let report = stop(&before, &both, ONE_POLL, ONE_POLL);
        assert_eq!(
            report,
            StopReport {
                failed: vec![
                    (first.clone(), unreadable_table()),
                    (second.clone(), unreadable_table())
                ],
                ..StopReport::default()
            },
            "a table that faulte cannot read before the first signal"
        );
        assert_eq!(before.signals(), Vec::new(), "no signal goes at all");
        assert_eq!(before.sleeps(), 0, "the sequence waits for nothing");

        let inside = machine_of(&[30, 40], vec![alive.clone()]).then_failing(unreadable_table());
        let report = stop(&inside, &both, THREE_POLLS, ONE_POLL);
        assert_eq!(
            report,
            StopReport {
                failed: vec![
                    (first.clone(), unreadable_table()),
                    (second, unreadable_table())
                ],
                ..StopReport::default()
            },
            "a table that faulte cannot read inside the grace period"
        );
        assert_eq!(inside.signals(), terminate, "SIGTERM went to both targets");

        let after = machine_of(&[30], vec![vec![process(30, LAUNCHD_PID)]; 2])
            .then_failing(unreadable_table());
        let report = stop(&after, &both[..1], ONE_POLL, ONE_POLL);
        assert_eq!(
            report,
            StopReport {
                failed: vec![(first, unreadable_table())],
                ..StopReport::default()
            },
            "a table that faulte cannot read after SIGKILL"
        );
        assert_eq!(
            after.signals(),
            vec![
                (Pid::new(30), Signal::Terminate),
                (Pid::new(30), Signal::Kill)
            ],
            "both signals went before the read that failed"
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
