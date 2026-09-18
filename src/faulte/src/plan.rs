//! The plan of `faulte kill`: which Claude Code sessions it selects, in which
//! order, and which sessions it refuses.
//!
//! A stop is not reversible, so every rule of the plan is a pure function over
//! plain values. A test gives the ranking, the process table, the limits and
//! the time, and reads the plan back. No test reads the real process table.
//!
//! The plan never signals anything. It names the targets, and the caller shows
//! them to the person before it asks the question.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use occ::SessionId;

use crate::duration::Span;
use crate::pid::Pid;
use crate::ranking::{ClaudeView, RankedRow, Ranking};
use crate::state::SessionState;
use crate::table::ProcessRow;

/// The limits of `faulte kill`, as the person gave them on the command line.
///
/// The limits are [`Span`] values and not [`Duration`] values, because the
/// plan gives them back to the person. The other-account block carries a
/// `sudo faulte kill` command line, and only a [`Span`] prints itself as `7d`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// A session is a candidate only when it is older than this span.
    pub older_than: Span,
    /// A session is a candidate only when it became idle more than this span
    /// ago.
    pub idle_for: Span,
    /// The greatest number of sessions to select. `None` selects every
    /// candidate.
    pub max: Option<usize>,
}

/// Everything that [`plan`] reads.
#[derive(Debug, Clone, Copy)]
pub struct PlanInput<'a> {
    /// The ranking of this Mac. Its rows carry the Claude Code view of each
    /// process, and the start time of each process.
    pub ranking: &'a Ranking,
    /// The process table of `ps`. The plan reads the parent links and the
    /// zombie flags from it, to find a live descendant of a session.
    pub table: &'a [ProcessRow],
    /// The limits that the person gave.
    pub rules: Rules,
    /// The PID of this `faulte` process. A session that runs `faulte kill` is
    /// never a candidate.
    pub faulte: Pid,
    /// The time now. The age of a process and the idle time of a session are
    /// both measured against it.
    pub now: SystemTime,
}

/// One session that the plan selects.
///
/// The candidate carries the identity of the process and the exact time of the
/// last status change. The person can take minutes to answer the question, so
/// the check before the signal reads the table and the registry again and
/// compares both values. A PID alone is not an identity: the number comes back
/// for another process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The row of the ranking that the plan selected.
    pub row: RankedRow,
    /// The session that the process belongs to.
    pub session: SessionId,
    /// The time when the process started, in seconds since the Unix epoch.
    /// Rule 1 proves that the row has one.
    pub started_at_epoch_secs: u64,
    /// The time when the status of the session last changed. `None` only when
    /// that time is before the Unix epoch, which no clock of a Mac gives.
    pub status_changed_at: Option<SystemTime>,
}

/// The count of the sessions that the plan refused, under the first rule that
/// each one fails.
///
/// A session can fail more than one rule. Each session is in exactly one count
/// here, under the first rule that [`plan`] tests and that the session fails.
/// The order is the order of the fields: it runs `faulte`, it is too young, it
/// is not idle, it is idle for too short a time, it has a live descendant.
///
/// `faulte` comes first because a session that runs `faulte kill` holds
/// `faulte` as a descendant. Under another order, every such session would
/// report the wrong reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NotSelected {
    /// The sessions that are `faulte` or an ancestor of `faulte` (rule 4).
    pub runs_faulte: usize,
    /// The sessions that are not older than [`Rules::older_than`] (rule 1).
    pub too_young: usize,
    /// The sessions whose status is not `idle` (rule 2).
    pub not_idle: usize,
    /// The sessions that are idle, and became idle inside
    /// [`Rules::idle_for`], or at a time that the record does not give
    /// (rule 2).
    pub idle_too_short: usize,
    /// The sessions that have a live descendant (rule 3).
    pub live_descendant: usize,
}

/// What `faulte kill` does, before it asks the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The sessions to stop, oldest first. [`Rules::max`] keeps the first
    /// ones.
    pub candidates: Vec<Candidate>,
    /// The count of the candidates that [`Rules::max`] held back.
    pub held_back_by_max: usize,
    /// The count of the sessions that the plan refused, by rule.
    pub not_selected: NotSelected,
    /// The rows of another account that pass every rule which `faulte` can
    /// read without root, oldest first. `faulte` never signals them.
    pub other_account: Vec<RankedRow>,
    /// The limits that the person gave.
    pub rules: Rules,
}

/// Gives the time `now` in seconds since the Unix epoch.
///
/// A time before the epoch gives zero. Every process of a Mac started after
/// the epoch, so a clock that far back makes every process look young, and the
/// plan then selects nothing.
fn epoch_seconds(now: SystemTime) -> u64 {
    now.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Gives the age of a process that started at `started_at_epoch_secs`, at the
/// time `now_secs`.
///
/// A process with no start time has no age, and rule 1 refuses it. `ps` gives
/// the start time of every process that it lists, so only the row of the
/// kernel has none. A process that started after `now_secs` has no age either:
/// the clock of this Mac can move back between the read of the table and the
/// read of the time.
fn age_of(started_at_epoch_secs: Option<u64>, now_secs: u64) -> Option<Duration> {
    started_at_epoch_secs
        .filter(|started| *started <= now_secs)
        .map(|started| Duration::from_secs(now_secs - started))
}

/// Gives the parent of each process of `table`.
///
/// The first row of a PID wins. `ps` never prints a PID twice, and a source
/// that breaks that rule cannot give one process two parents here.
fn parents_of(table: &[ProcessRow]) -> HashMap<Pid, Pid> {
    let mut parents = HashMap::with_capacity(table.len());
    for process in table {
        parents.entry(process.pid).or_insert(process.ppid);
    }
    parents
}

/// Puts `pid` and each ancestor of `pid` into `seen`.
///
/// The walk stops at a PID that `seen` holds already. That one rule ends three
/// walks. A cycle in the parent links comes back to a PID of the same walk. A
/// process that is its own parent is such a cycle of one step. And a walk that
/// reaches a chain of an earlier walk stops, because `seen` holds every
/// ancestor above that PID already.
fn walk_up(pid: Pid, parents: &HashMap<Pid, Pid>, seen: &mut HashSet<Pid>) {
    let mut pid = pid;
    while seen.insert(pid) {
        let Some(&parent) = parents.get(&pid) else {
            break;
        };
        pid = parent;
    }
}

/// Gives the PID of every process of `table` that has a live descendant.
///
/// The walk starts at the parent of each process that is not a zombie, so a
/// process is never a descendant of itself. A zombie stopped already, and its
/// parent did not collect its exit status yet, so it does nothing and it is
/// not a live descendant. The walk still climbs through a zombie, because the
/// table can hold a live process under one.
fn pids_with_a_live_descendant(table: &[ProcessRow]) -> HashSet<Pid> {
    let parents = parents_of(table);
    let mut ancestors = HashSet::with_capacity(table.len());
    for process in table.iter().filter(|process| !process.zombie) {
        walk_up(process.ppid, &parents, &mut ancestors);
    }
    ancestors
}

/// Gives the PID of `faulte` and the PID of each of its ancestors.
///
/// A session that runs `faulte kill` is never a candidate. Rule 3 refuses
/// every ancestor of `faulte` as well, because `faulte` is a live descendant
/// of each one. This rule states the reason, and it holds when rule 3 changes.
fn faulte_and_its_ancestors(table: &[ProcessRow], faulte: Pid) -> HashSet<Pid> {
    let mut chain = HashSet::new();
    walk_up(faulte, &parents_of(table), &mut chain);
    chain
}

/// Gives the time when the status of `state` last changed.
///
/// The state carries the time since the change, and the ranking measured it
/// against the same `now`. Thus this subtraction gives the time of the change
/// back exactly, and the check before the signal compares it with the time
/// that a fresh record gives.
fn status_changed_at(state: &SessionState, now: SystemTime) -> Option<SystemTime> {
    match state {
        SessionState::Idle { for_: Some(for_) } => now.checked_sub(*for_),
        _ => None,
    }
}

/// Gives the plan of `faulte kill` over `input`.
#[must_use]
pub fn plan(input: &PlanInput<'_>) -> Plan {
    let older_than = Duration::from(input.rules.older_than);
    let idle_for = Duration::from(input.rules.idle_for);
    let now_secs = epoch_seconds(input.now);
    let live_descendants = pids_with_a_live_descendant(input.table);
    let runs_faulte = faulte_and_its_ancestors(input.table, input.faulte);
    let mut candidates = Vec::new();
    for row in &input.ranking.rows {
        // Only a session that `faulte` read a registry record for can be a
        // candidate. A row with no record shows no session, so a stop of that
        // row names a session that nothing proved.
        let ClaudeView::Session { id, state, .. } = &row.claude else {
            continue;
        };
        if runs_faulte.contains(&row.pid) {
            continue;
        }
        let (Some(age), Some(started_at_epoch_secs)) = (
            age_of(row.started_at_epoch_secs, now_secs),
            row.started_at_epoch_secs,
        ) else {
            continue;
        };
        if age <= older_than {
            continue;
        }
        if !state.is_idle_for_more_than(idle_for) {
            continue;
        }
        if live_descendants.contains(&row.pid) {
            continue;
        }
        candidates.push(Candidate {
            row: row.clone(),
            session: id.clone(),
            started_at_epoch_secs,
            status_changed_at: status_changed_at(state, input.now),
        });
    }
    // The person reads this order to decide. The PID breaks a tie, so two
    // runs over the same sources agree on the order.
    candidates.sort_by(|left, right| {
        left.started_at_epoch_secs
            .cmp(&right.started_at_epoch_secs)
            .then_with(|| left.row.pid.cmp(&right.row.pid))
    });
    let held_back_by_max = match input.rules.max {
        Some(max) if max < candidates.len() => {
            let held_back = candidates.len() - max;
            candidates.truncate(max);
            held_back
        }
        _ => 0,
    };
    Plan {
        candidates,
        held_back_by_max,
        not_selected: NotSelected::default(),
        other_account: Vec::new(),
        rules: input.rules,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use crate::pid::Uid;
    use crate::ranking::{ClaudeTotal, Skipped};

    /// The time now in the tests, in seconds since the Unix epoch.
    const NOW: u64 = 1_780_000_000;

    /// The age of a process that passes rule 1, in seconds: eight days.
    const OLD: u64 = 8 * 86_400;

    /// The age of a process that fails rule 1, in seconds: one day.
    const YOUNG: u64 = 86_400;

    /// The UID of the account that runs the tool in the tests.
    const VIEWER_UID: u32 = 501;

    /// The PID of the `faulte` process in the tests.
    const FAULTE_PID: u32 = 99;

    /// The PID of the parent of every process of the tests: `launchd`.
    const LAUNCHD_PID: u32 = 1;

    /// The working directory of a session in the tests.
    const DIRECTORY: &str = "/Volumes/SamsungSSDs/code/tools";

    /// Gives the time now in the tests.
    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(NOW)
    }

    /// Gives the session of the process `pid`. Each PID gives a different
    /// session, so an assertion names the session that it expects.
    fn session_id(pid: u32) -> SessionId {
        SessionId::parse(&format!("d3b0d921-f0a1-41fc-b309-{pid:012}"))
            .expect("the text of the test session is a UUID")
    }

    /// Gives the state of a session that is idle for `seconds`.
    fn idle(seconds: u64) -> SessionState {
        SessionState::Idle {
            for_: Some(Duration::from_secs(seconds)),
        }
    }

    /// Gives the ranked row of the process `pid`, which started `age` seconds
    /// ago, and which `claude` describes.
    fn row(pid: u32, age: u64, claude: ClaudeView) -> RankedRow {
        RankedRow {
            pid: Pid::new(pid),
            uid: Uid::new(VIEWER_UID),
            faults: 1_000,
            rss_kib: Some(831_488),
            started_at_epoch_secs: Some(NOW - age),
            command: format!("claude --process {pid}"),
            claude,
        }
    }

    /// Gives the ranked row of a Claude Code session in `state`, which started
    /// `age` seconds ago.
    fn session(pid: u32, age: u64, state: SessionState) -> RankedRow {
        row(
            pid,
            age,
            ClaudeView::Session {
                id: session_id(pid),
                state,
                directory: Some(PathBuf::from(DIRECTORY)),
            },
        )
    }

    /// Gives a ranking of `rows`.
    fn ranking(rows: Vec<RankedRow>) -> Ranking {
        let total_faults = rows.iter().map(|row| row.faults).sum();
        Ranking {
            rows,
            window: Duration::from_secs(4),
            total_faults,
            claude: ClaudeTotal::default(),
            skipped: Skipped::default(),
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
            started_at_epoch_secs: NOW - OLD,
            command: format!("claude --process {pid}"),
        }
    }

    /// Gives the table row of a zombie process `pid`, whose parent is `ppid`.
    /// A zombie stopped already, so it is not a live descendant.
    fn zombie(pid: u32, ppid: u32) -> ProcessRow {
        ProcessRow {
            zombie: true,
            ..process(pid, ppid)
        }
    }

    /// Gives the default limits of `faulte kill`: seven days and ten minutes.
    fn rules() -> Rules {
        Rules {
            older_than: "7d".parse().expect("7d is a span"),
            idle_for: "10m".parse().expect("10m is a span"),
            max: None,
        }
    }

    /// Gives the input of the plan over `ranking` and `table`, under `rules`.
    fn input<'a>(ranking: &'a Ranking, table: &'a [ProcessRow], rules: Rules) -> PlanInput<'a> {
        PlanInput {
            ranking,
            table,
            rules,
            faulte: Pid::new(FAULTE_PID),
            now: now(),
        }
    }

    /// Gives the PID of each candidate of `plan`, in the order of the plan.
    fn selected(plan: &Plan) -> Vec<u32> {
        plan.candidates
            .iter()
            .map(|candidate| candidate.row.pid.get())
            .collect()
    }

    /// Rule 1: a session is a candidate only when it is older than the limit.
    /// A process that is not Claude Code, and a Claude Code process with no
    /// registry record, are never candidates, whatever their age.
    #[test]
    fn only_a_claude_session_older_than_the_limit_is_a_candidate() {
        let ranking = ranking(vec![
            row(10, OLD, ClaudeView::NotClaude),
            row(20, OLD, ClaudeView::NoRecord),
            session(30, OLD, idle(3_600)),
            session(40, YOUNG, idle(3_600)),
        ]);
        let table = [
            process(10, LAUNCHD_PID),
            process(20, LAUNCHD_PID),
            process(30, LAUNCHD_PID),
            process(40, LAUNCHD_PID),
            process(FAULTE_PID, LAUNCHD_PID),
        ];

        let plan = plan(&input(&ranking, &table, rules()));

        assert_eq!(selected(&plan), vec![30]);
        assert_eq!(
            plan.candidates.first().map(|candidate| &candidate.session),
            Some(&session_id(30))
        );
    }

    /// Gives the table of the tests: one row for each PID of `pids`, and one
    /// row for `faulte`. Every process is a child of `launchd`.
    fn table_of(pids: &[u32]) -> Vec<ProcessRow> {
        pids.iter()
            .chain([FAULTE_PID].iter())
            .map(|pid| process(*pid, LAUNCHD_PID))
            .collect()
    }

    /// Rule 2: a session is a candidate only when its status is `idle`, and it
    /// became idle more than the limit ago. An idle time equal to the limit is
    /// not more than it, and an idle time that the record does not give is not
    /// long enough. Every status other than `idle` is active.
    ///
    /// The candidate carries the time of the last status change, which the
    /// check before the signal compares against a fresh record.
    #[test]
    fn only_a_session_that_is_idle_for_longer_than_the_limit_is_a_candidate() {
        let ranking = ranking(vec![
            session(30, OLD, idle(3_600)),
            session(40, OLD, idle(60)),
            session(50, OLD, idle(600)),
            session(60, OLD, SessionState::Idle { for_: None }),
            session(70, OLD, SessionState::Busy),
            session(80, OLD, SessionState::Waiting),
            session(90, OLD, SessionState::Other("shell".to_owned())),
            session(100, OLD, SessionState::Unknown),
        ]);
        let table = table_of(&[30, 40, 50, 60, 70, 80, 90, 100]);

        let plan = plan(&input(&ranking, &table, rules()));

        assert_eq!(selected(&plan), vec![30]);
        assert_eq!(
            plan.candidates
                .first()
                .map(|candidate| candidate.status_changed_at),
            Some(Some(now() - Duration::from_secs(3_600)))
        );
    }

    /// The plan gives the candidates oldest first, whatever order the ranking
    /// gives its rows in. The ranking sorts by faults, and the person reads
    /// the plan to decide which sessions to stop. Two sessions that started in
    /// the same second keep the order of their PIDs, so two runs over the same
    /// sources agree.
    ///
    /// `--max N` keeps the N oldest, and the plan counts the rest. A limit
    /// that is not below the count of the candidates holds nothing back.
    #[test]
    fn the_plan_is_oldest_first_and_max_keeps_the_oldest_of_them() {
        let ten_days = 10 * 86_400;
        let ranking = ranking(vec![
            session(30, OLD, idle(3_600)),
            session(40, ten_days, idle(3_600)),
            session(50, ten_days, idle(3_600)),
            session(60, 9 * 86_400, idle(3_600)),
        ]);
        let table = table_of(&[30, 40, 50, 60]);
        let under = |max| plan(&input(&ranking, &table, Rules { max, ..rules() }));

        let every = under(None);
        let two = under(Some(2));
        let plenty = under(Some(10));

        assert_eq!(selected(&every), vec![40, 50, 60, 30]);
        assert_eq!(every.held_back_by_max, 0);
        assert_eq!(selected(&two), vec![40, 50]);
        assert_eq!(two.held_back_by_max, 2);
        assert_eq!(selected(&plenty), vec![40, 50, 60, 30]);
        assert_eq!(plenty.held_back_by_max, 0);
    }

    /// Rule 3: a session with a live descendant is never a candidate. A
    /// background shell and a running tool call make a session active,
    /// whatever its status says.
    ///
    /// The walk reads the parent links of the whole table, so a descendant two
    /// levels down counts. A zombie is not alive, and a zombie between a
    /// session and a live grandchild does not hide that grandchild.
    ///
    /// A cycle in the parent links, and a process that is its own parent, end
    /// the walk. Neither one can hold the tool.
    #[test]
    fn a_session_with_a_live_descendant_is_never_a_candidate() {
        let ranking = ranking(vec![
            session(30, OLD, idle(3_600)),
            session(40, OLD, idle(3_600)),
            session(50, OLD, idle(3_600)),
            session(60, OLD, idle(3_600)),
            session(70, OLD, idle(3_600)),
        ]);
        let table = [
            process(30, LAUNCHD_PID),
            process(40, LAUNCHD_PID),
            process(41, 40),
            process(50, LAUNCHD_PID),
            zombie(51, 50),
            process(52, 51),
            process(60, LAUNCHD_PID),
            zombie(61, 60),
            process(70, LAUNCHD_PID),
            process(71, 72),
            process(72, 71),
            process(73, 73),
            process(FAULTE_PID, LAUNCHD_PID),
        ];

        let plan = plan(&input(&ranking, &table, rules()));

        assert_eq!(selected(&plan), vec![30, 60, 70]);
    }

    /// Each session that the plan refused is in exactly one count, under the
    /// first rule that it fails. A session can fail more than one rule, so the
    /// order of the rules decides the reason: it runs `faulte`, it is too
    /// young, it is not idle, it is idle for too short a time, it has a live
    /// descendant.
    ///
    /// `faulte` comes first, and that is the half of rule 4 which rule 3
    /// cannot show. `faulte` is a live descendant of every ancestor of it, so
    /// every such session would report the wrong reason under another order.
    ///
    /// A process that is not Claude Code, and a Claude Code process with no
    /// registry record, are in no count. The plan never refused them, because
    /// they were never sessions that it can stop.
    #[test]
    fn the_count_of_each_reason_is_the_first_rule_that_the_session_fails() {
        let ranking = ranking(vec![
            session(30, OLD, idle(3_600)),
            session(40, YOUNG, idle(3_600)),
            session(50, OLD, SessionState::Busy),
            session(60, OLD, idle(60)),
            session(70, OLD, idle(3_600)),
            session(80, OLD, idle(3_600)),
            session(90, YOUNG, SessionState::Busy),
            session(100, OLD, SessionState::Busy),
            session(110, OLD, idle(60)),
            session(120, YOUNG, idle(3_600)),
            row(200, YOUNG, ClaudeView::NotClaude),
            row(210, YOUNG, ClaudeView::NoRecord),
        ]);
        let mut table = table_of(&[30, 40, 50, 60, 70, 90, 100, 110, 200, 210]);
        table.extend([
            process(71, 70),
            process(91, 90),
            process(101, 100),
            process(111, 110),
            // The session 120 started the session 80, and the session 80 ran
            // `faulte kill`. Both of them are ancestors of `faulte`.
            process(120, LAUNCHD_PID),
            process(80, 120),
            process(FAULTE_PID, 80),
        ]);

        let plan = plan(&input(&ranking, &table, rules()));

        assert_eq!(selected(&plan), vec![30]);
        assert_eq!(
            plan.not_selected,
            NotSelected {
                runs_faulte: 2,
                too_young: 2,
                not_idle: 2,
                idle_too_short: 2,
                live_descendant: 1,
            }
        );
    }

    /// Rule 4: the `faulte` process is never a candidate. The session of this
    /// test passes rules 1, 2 and 3: it is old, it is idle, and it has no
    /// descendant. Rule 4 alone refuses it.
    ///
    /// The other half of rule 4 is the ancestors of `faulte`. Rule 3 refuses
    /// each one of them as well, because `faulte` is a live descendant of
    /// every one of its ancestors. Thus the count of each reason pins that
    /// half, and `the_count_of_each_reason_is_the_first_rule_that_the_session_fails`
    /// holds it.
    #[test]
    fn a_session_that_is_the_faulte_process_is_never_a_candidate() {
        let ranking = ranking(vec![
            session(FAULTE_PID, OLD, idle(3_600)),
            session(30, OLD, idle(3_600)),
        ]);
        let table = [process(FAULTE_PID, LAUNCHD_PID), process(30, LAUNCHD_PID)];

        let plan = plan(&input(&ranking, &table, rules()));

        assert_eq!(selected(&plan), vec![30]);
    }
}
