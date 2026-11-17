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
///
/// Each read uses a new `sysinfo::System`. `sysinfo` reads the status of a
/// process (`pbi_status` on macOS) only when it sees the pid for the first
/// time. A later refresh of a known pid keeps the old status. Thus a long-lived
/// `System` does not see a later `SIGSTOP`. A new `System` makes each read a
/// first read. It is still one read of the process table.
#[derive(Debug, Default)]
pub struct SystemClock;

impl SystemClock {
    /// A clock for the process table of the system.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ProcessClock for SystemClock {
    fn processes(&mut self, pids: &[Pid]) -> HashMap<Pid, ProcessFacts> {
        let wanted: Vec<sysinfo::Pid> = pids
            .iter()
            .map(|pid| sysinfo::Pid::from_u32(pid.as_u32()))
            .collect();
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&wanted),
            true,
            sysinfo::ProcessRefreshKind::nothing(),
        );
        pids.iter()
            .filter_map(|pid| {
                let process = system.process(sysinfo::Pid::from_u32(pid.as_u32()))?;
                let status = process.status();
                if status == sysinfo::ProcessStatus::Zombie {
                    return None;
                }
                let run = if status == sysinfo::ProcessStatus::Stop {
                    RunStatus::Stopped
                } else {
                    RunStatus::Running
                };
                Some((
                    *pid,
                    ProcessFacts {
                        start_secs: process.start_time(),
                        run,
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

    #[cfg(unix)]
    use std::os::unix::process::CommandExt;

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

    /// The longest time that a test waits for the process table to show a
    /// signal. The bound is large because it measures the machine, not the code.
    #[cfg(unix)]
    const SIGNAL_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

    /// The time between two reads of the process table while a test waits.
    #[cfg(unix)]
    const SIGNAL_POLL: std::time::Duration = std::time::Duration::from_millis(10);

    /// A `sleep 60` process and its watchdog. A stopped process runs no code,
    /// so its `sleep` does not end by itself. When cargo dies after the stop,
    /// the process stays stopped forever. The watchdog is a shell in its own
    /// process group, so a Ctrl-C to the group of the test does not reach it.
    /// It waits 60 seconds and then kills the process with `SIGKILL`, which
    /// ends a stopped process. `Drop` ends both children, also after a failed
    /// assertion.
    #[cfg(unix)]
    struct SleepGuard {
        target: std::process::Child,
        watchdog: std::process::Child,
    }

    #[cfg(unix)]
    impl SleepGuard {
        fn start() -> Self {
            let target = childenv::fixture_command("sleep")
                .arg("60")
                .spawn()
                .expect("sleep starts");
            let watchdog = childenv::fixture_command("/bin/sh")
                .args([
                    "-c",
                    "sleep 60; kill -KILL \"$1\" 2>/dev/null",
                    "cctop-watchdog",
                    &target.id().to_string(),
                ])
                .process_group(0)
                .spawn()
                .expect("watchdog starts");
            Self { target, watchdog }
        }

        fn pid(&self) -> Pid {
            Pid::new(self.target.id())
        }

        /// Send `signal` to the target with the `kill` program.
        fn signal(&self, signal: &str) {
            let status = childenv::fixture_command("/bin/kill")
                .args([signal, &self.target.id().to_string()])
                .status()
                .expect("kill starts");
            assert!(status.success(), "kill {signal} failed: {status}");
        }

        /// Read the process table until the target has the run status `want`.
        /// The result is the facts of the last read.
        fn wait_for(&self, clock: &mut SystemClock, want: RunStatus) -> Option<ProcessFacts> {
            let pid = self.pid();
            let deadline = std::time::Instant::now() + SIGNAL_WAIT;
            loop {
                let facts = clock.processes(&[pid]).get(&pid).copied();
                let reached = facts.is_some_and(|facts| facts.run == want);
                if reached || std::time::Instant::now() >= deadline {
                    return facts;
                }
                std::thread::sleep(SIGNAL_POLL);
            }
        }
    }

    #[cfg(unix)]
    impl Drop for SleepGuard {
        fn drop(&mut self) {
            let _ = self.target.kill();
            let _ = self.target.wait();
            let _ = childenv::fixture_command("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", self.watchdog.id())])
                .status();
            let _ = self.watchdog.wait();
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_system_clock_sees_a_stopped_process_and_then_a_continued_one() {
        let guard = SleepGuard::start();
        let pid = guard.pid();
        let mut clock = SystemClock::new();
        let before = guard
            .wait_for(&mut clock, RunStatus::Running)
            .expect("the child is live");
        assert_eq!(before.run, RunStatus::Running);

        guard.signal("-STOP");
        let stopped = guard.wait_for(&mut clock, RunStatus::Stopped);
        assert_eq!(stopped.map(|facts| facts.run), Some(RunStatus::Stopped));
        assert_eq!(
            stopped.map(|facts| facts.start_secs),
            Some(before.start_secs)
        );

        guard.signal("-CONT");
        let running = guard.wait_for(&mut clock, RunStatus::Running);
        assert_eq!(running.map(|facts| facts.run), Some(RunStatus::Running));
        assert_eq!(
            running.map(|facts| facts.start_secs),
            Some(before.start_secs)
        );
        assert!(clock.processes(&[pid]).contains_key(&pid));
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
