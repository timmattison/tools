//! The check that tells if the process of a session still runs.

use std::collections::HashMap;

use proctree::Pid;

use crate::state::EpochMillis;

/// Tell if a session is stale: its process is gone, or a new process reused
/// its pid.
///
/// `process_start_secs` is the start time of the process with that pid, in
/// whole seconds since the epoch. It is `None` when no such process runs.
#[must_use]
pub fn is_stale(started_at: EpochMillis, process_start_secs: Option<u64>) -> bool {
    match process_start_secs {
        None => true,
        Some(secs) => secs.saturating_mul(1000) > started_at.as_u64(),
    }
}

/// Tell if a live process runs or is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunStatus {
    /// The process runs, or it can run.
    Running,
    /// The process is stopped, for example by `SIGSTOP`. It runs no code until
    /// it gets `SIGCONT`.
    Stopped,
}

/// What one read of the process table tells about one live process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessFacts {
    /// The start time of the process, in whole seconds since the epoch.
    pub start_secs: u64,
    /// Tell if the process runs or is stopped.
    pub run: RunStatus,
}

/// A source for the facts about processes.
pub trait ProcessClock {
    /// The facts about each live, non-zombie process in `pids`, from one read
    /// of the process table. A pid with no such process is not in the map.
    fn processes(&mut self, pids: &[Pid]) -> HashMap<Pid, ProcessFacts>;
}

/// The process table of the system, read with `sysinfo`.
#[derive(Debug, Default)]
pub struct SystemClock {
    system: sysinfo::System,
}

impl SystemClock {
    /// A clock that has read no process yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ProcessClock for SystemClock {
    fn processes(&mut self, pids: &[Pid]) -> HashMap<Pid, ProcessFacts> {
        let wanted: Vec<sysinfo::Pid> = pids
            .iter()
            .map(|pid| sysinfo::Pid::from_u32(pid.as_u32()))
            .collect();
        self.system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&wanted),
            true,
            sysinfo::ProcessRefreshKind::nothing(),
        );
        pids.iter()
            .filter_map(|pid| {
                let process = self.system.process(sysinfo::Pid::from_u32(pid.as_u32()))?;
                if process.status() == sysinfo::ProcessStatus::Zombie {
                    return None;
                }
                Some((
                    *pid,
                    ProcessFacts {
                        start_secs: process.start_time(),
                        run: RunStatus::Running,
                    },
                ))
            })
            .collect()
    }
}

/// What one read of the process table tells about a set of sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Survey {
    /// The pids of the stale sessions, sorted.
    pub stale: Vec<Pid>,
    /// The pids of the live sessions whose process is stopped, sorted. A stale
    /// pid is never in this list.
    pub stopped: Vec<Pid>,
}

/// Read the process table once for `sessions`. The function asks `clock` once
/// for all pids, and sorts the pids of the result.
#[must_use]
pub fn survey(sessions: &[(Pid, EpochMillis)], clock: &mut dyn ProcessClock) -> Survey {
    let pids: Vec<Pid> = sessions.iter().map(|(pid, _)| *pid).collect();
    let facts = clock.processes(&pids);
    let mut stale: Vec<Pid> = sessions
        .iter()
        .filter(|(pid, started_at)| {
            is_stale(*started_at, facts.get(pid).map(|facts| facts.start_secs))
        })
        .map(|(pid, _)| *pid)
        .collect();
    stale.sort_by_key(|pid| pid.as_u32());
    Survey {
        stale,
        stopped: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STARTED_AT: EpochMillis = EpochMillis::new(1_794_823_262_488);

    /// 2020-01-01T00:00:00Z in seconds. A real start time is later than this.
    const YEAR_2020_SECS: u64 = 1_577_836_800;

    /// The seconds in one day. A test binary never runs for this long.
    const ONE_DAY_SECS: u64 = 86_400;

    #[test]
    fn no_process_is_stale() {
        assert!(is_stale(STARTED_AT, None));
    }

    #[test]
    fn a_process_that_started_before_the_session_is_not_stale() {
        assert!(!is_stale(STARTED_AT, Some(1_794_823_261)));
    }

    #[test]
    fn a_process_in_the_same_second_is_not_stale() {
        assert!(!is_stale(STARTED_AT, Some(1_794_823_262)));
    }

    #[test]
    fn a_process_that_started_one_second_later_is_stale() {
        assert!(is_stale(STARTED_AT, Some(1_794_823_263)));
    }

    #[test]
    fn the_comparison_does_not_overflow() {
        assert!(is_stale(STARTED_AT, Some(u64::MAX)));
    }

    /// A clock that answers from a map and counts its calls.
    struct FakeClock {
        times: HashMap<Pid, u64>,
        calls: Vec<Vec<Pid>>,
    }

    impl ProcessClock for FakeClock {
        fn processes(&mut self, pids: &[Pid]) -> HashMap<Pid, ProcessFacts> {
            self.calls.push(pids.to_vec());
            self.times
                .iter()
                .map(|(pid, secs)| {
                    (
                        *pid,
                        ProcessFacts {
                            start_secs: *secs,
                            run: RunStatus::Running,
                        },
                    )
                })
                .collect()
        }
    }

    #[test]
    fn the_system_clock_gives_the_real_start_time_of_this_process_and_none_for_an_absent_one() {
        let own = Pid::current();
        let absent = Pid::new(99_999_999);
        let mut clock = SystemClock::new();
        let times = clock.processes(&[own, absent]);
        let now_secs = EpochMillis::now().as_u64() / 1000;
        let own_facts = times.get(&own).copied().expect("this process is live");
        let start = own_facts.start_secs;
        assert_eq!(own_facts.run, RunStatus::Running);
        assert!(start <= now_secs, "start {start} is after now {now_secs}");
        assert!(
            start > YEAR_2020_SECS,
            "start {start} is not after 2020 (now {now_secs})"
        );
        assert!(
            start + ONE_DAY_SECS >= now_secs,
            "start {start} is more than one day before now {now_secs}"
        );
        assert!(!times.contains_key(&absent));
    }

    #[test]
    fn survey_gives_the_stale_pids_sorted_from_one_read() {
        let sessions = [
            (Pid::new(40), STARTED_AT),
            (Pid::new(30), STARTED_AT),
            (Pid::new(20), STARTED_AT),
            (Pid::new(10), STARTED_AT),
        ];
        let mut clock = FakeClock {
            times: HashMap::from([
                (Pid::new(10), 1_794_823_262),
                (Pid::new(30), 1_794_823_999),
                (Pid::new(40), 1_794_823_000),
            ]),
            calls: Vec::new(),
        };
        assert_eq!(
            survey(&sessions, &mut clock).stale,
            vec![Pid::new(20), Pid::new(30)]
        );
        assert_eq!(
            clock.calls,
            vec![vec![Pid::new(40), Pid::new(30), Pid::new(20), Pid::new(10)]]
        );
    }
}
