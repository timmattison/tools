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

/// A source for the start time of processes.
pub trait ProcessClock {
    /// The start time of each live, non-zombie process in `pids`, in whole
    /// seconds since the epoch. A pid with no such process is not in the map.
    fn start_times(&mut self, pids: &[Pid]) -> HashMap<Pid, u64>;
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
    fn start_times(&mut self, pids: &[Pid]) -> HashMap<Pid, u64> {
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
                Some((*pid, process.start_time()))
            })
            .collect()
    }
}

/// The pids of the stale sessions, sorted. The function asks `clock` once for
/// all pids.
#[must_use]
pub fn stale_pids(sessions: &[(Pid, EpochMillis)], clock: &mut dyn ProcessClock) -> Vec<Pid> {
    let pids: Vec<Pid> = sessions.iter().map(|(pid, _)| *pid).collect();
    let times = clock.start_times(&pids);
    let mut stale: Vec<Pid> = sessions
        .iter()
        .filter(|(pid, started_at)| is_stale(*started_at, times.get(pid).copied()))
        .map(|(pid, _)| *pid)
        .collect();
    stale.sort_by_key(|pid| pid.as_u32());
    stale
}

#[cfg(test)]
mod tests {
    use super::*;

    const STARTED_AT: EpochMillis = EpochMillis::new(1_794_823_262_488);

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
        fn start_times(&mut self, pids: &[Pid]) -> HashMap<Pid, u64> {
            self.calls.push(pids.to_vec());
            self.times.clone()
        }
    }

    #[test]
    fn the_system_clock_knows_this_process_and_not_an_absent_one() {
        let own = Pid::current();
        let absent = Pid::new(99_999_999);
        let mut clock = SystemClock::new();
        let times = clock.start_times(&[own, absent]);
        let now_secs = EpochMillis::now().as_u64() / 1000;
        let start = times.get(&own).copied().expect("this process is live");
        assert!(start <= now_secs);
        assert!(!times.contains_key(&absent));
    }

    #[test]
    fn stale_pids_gives_the_gone_and_the_reused_pid_sorted() {
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
            stale_pids(&sessions, &mut clock),
            vec![Pid::new(20), Pid::new(30)]
        );
        assert_eq!(
            clock.calls,
            vec![vec![Pid::new(40), Pid::new(30), Pid::new(20), Pid::new(10)]]
        );
    }
}
