//! Asks each host how busy it is, and chooses the least busy host.
//!
//! `drun` starts one small `busybox` container on each host, all at the same
//! time. The container prints the memory figures of the host, its load
//! average, and its count of CPUs. Docker removes the container when it stops.
//! On a Linux host the figures are those of the host. On a host that runs
//! Docker Desktop they are those of the virtual machine that runs the
//! containers, which is the memory that a container can use.
//!
//! A host is as busy as the larger of two fractions: the memory in use, and
//! the one-minute load average for each CPU. The least busy host has the
//! lowest value. When two hosts have the same value, the host with more free
//! memory wins, and after that the host that is first in the configuration.

use crate::config::Config;
use crate::host::{self, Answer, ContextName, HostFailure};
use crate::table;

/// The image of the probe container. It is small, and it has `sh`, `cat`,
/// and `nproc` for every CPU architecture that Docker supports.
pub const PROBE_IMAGE: &str = "busybox:1.37";

/// The label of the probe container, so a person who sees it knows what it is.
const PROBE_LABEL: &str = "drun.probe=true";

/// The line that divides the three parts of the output of the probe.
const SECTION_MARK: &str = "@@drun@@";

/// The kibibytes in one gibibyte.
const KIB_PER_GIB: f64 = 1_048_576.0;

/// How busy one host is.
#[derive(Debug, Clone, PartialEq)]
pub struct Load {
    memory_total_kib: u64,
    memory_available_kib: u64,
    load_one_minute: f64,
    cpus: u32,
}

/// The answers of all hosts to the probe.
#[derive(Debug)]
pub struct Survey {
    answers: Vec<Answer<Load>>,
}

/// Asks every host in `config` how busy it is, all at the same time.
pub fn survey(config: &Config) -> Survey {
    let script = probe_script();
    let args = [
        "run",
        "--rm",
        "--pull",
        "missing",
        "--network",
        "none",
        "--label",
        PROBE_LABEL,
        PROBE_IMAGE,
        "sh",
        "-c",
        &script,
    ];
    let answers = host::ask_all(config.contexts(), |context| {
        let text = host::ask(context, &args, config.host_timeout())?;
        Load::parse(&text).map_err(HostFailure::Unreadable)
    });
    Survey { answers }
}

/// The shell script that the probe container runs.
fn probe_script() -> String {
    format!("cat /proc/meminfo; echo {SECTION_MARK}; cat /proc/loadavg; echo {SECTION_MARK}; nproc")
}

impl Load {
    /// Reads the output of the probe container.
    ///
    /// # Errors
    ///
    /// Returns words that say what is missing or wrong in `text`.
    fn parse(text: &str) -> Result<Self, String> {
        todo!("{text}")
    }

    /// The fraction of the memory in use, from 0 to 1.
    fn memory_in_use(&self) -> f64 {
        todo!("{self:?}")
    }

    /// The one-minute load average for each CPU. It is more than 1 when more
    /// work waits than the CPUs can do.
    fn cpu_in_use(&self) -> f64 {
        todo!("{self:?}")
    }

    /// How busy the host is: the larger of [`Self::memory_in_use`] and
    /// [`Self::cpu_in_use`].
    pub fn busy(&self) -> f64 {
        todo!("{self:?}")
    }

    /// One line that says how busy the host is, for a person to read.
    pub fn summary(&self) -> String {
        todo!("{self:?}")
    }
}

impl Survey {
    /// The least busy host that answered, or `None` when no host answered.
    pub fn chosen(&self) -> Option<(&ContextName, &Load)> {
        todo!("{self:?}")
    }

    /// The hosts that gave no usable answer, and why.
    pub fn failures(&self) -> impl Iterator<Item = (&ContextName, &HostFailure)> {
        self.answers.iter().filter_map(|answer| match &answer.result {
            Ok(_) => None,
            Err(failure) => Some((&answer.context, failure)),
        })
    }

    /// A table of all hosts that shows how busy each is and which one is
    /// chosen.
    pub fn table(&self) -> String {
        todo!("{}", table::render(&[], &[]))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// The real `/proc/meminfo` of a computer with 16 GB of memory, cut short.
    const MEMINFO: &str = "MemTotal:       16318592 kB
MemFree:         1203364 kB
MemAvailable:   12237944 kB
Buffers:          412300 kB
Cached:         10123456 kB
SwapCached:            0 kB
SwapTotal:       2097148 kB
SwapFree:        2097148 kB";

    /// The output of the probe for `meminfo`, `loadavg`, and `nproc`.
    fn probe_output(meminfo: &str, loadavg: &str, nproc: &str) -> String {
        format!("{meminfo}\n{SECTION_MARK}\n{loadavg}\n{SECTION_MARK}\n{nproc}\n")
    }

    fn load(total_kib: u64, available_kib: u64, load_one_minute: f64, cpus: u32) -> Load {
        Load {
            memory_total_kib: total_kib,
            memory_available_kib: available_kib,
            load_one_minute,
            cpus,
        }
    }

    fn answer(name: &str, result: Result<Load, HostFailure>) -> Answer<Load> {
        Answer {
            context: ContextName::parse(name).unwrap(),
            result,
        }
    }

    fn close(left: f64, right: f64) -> bool {
        (left - right).abs() < 1e-9
    }

    #[test]
    fn the_probe_output_gives_memory_load_and_cpus() {
        let text = probe_output(MEMINFO, "0.52 0.58 0.59 1/389 12345", "4");
        assert_eq!(
            Load::parse(&text),
            Ok(load(16_318_592, 12_237_944, 0.52, 4))
        );
    }

    #[test]
    fn a_probe_output_without_mem_available_is_refused() {
        let meminfo = "MemTotal:       16318592 kB\nMemFree:         1203364 kB";
        let error = Load::parse(&probe_output(meminfo, "0.52 0.58 0.59 1/389 12345", "4"))
            .unwrap_err();
        assert!(error.contains("MemAvailable"), "{error}");
    }

    #[test]
    fn a_probe_output_with_a_wrong_number_is_refused() {
        for (loadavg, nproc) in [("high 0.5 0.5 1/2 3", "4"), ("0.5 0.5 0.5 1/2 3", "0"), ("", "4")] {
            assert!(
                Load::parse(&probe_output(MEMINFO, loadavg, nproc)).is_err(),
                "{loadavg:?} {nproc:?} must be refused"
            );
        }
    }

    #[test]
    fn a_probe_output_without_its_three_parts_is_refused() {
        assert!(Load::parse(MEMINFO).is_err());
        assert!(Load::parse("").is_err());
    }

    #[test]
    fn a_host_is_as_busy_as_its_fullest_resource() {
        let memory_full = load(1_000, 100, 0.4, 4);
        assert!(close(memory_full.busy(), 0.9), "{}", memory_full.busy());
        let cpus_full = load(1_000, 900, 6.0, 4);
        assert!(close(cpus_full.busy(), 1.5), "{}", cpus_full.busy());
    }

    #[test]
    fn more_free_memory_than_total_memory_is_not_less_than_idle() {
        assert!(close(load(1_000, 1_200, 0.0, 1).busy(), 0.0));
    }

    #[test]
    fn the_least_busy_host_is_chosen() {
        let survey = Survey {
            answers: vec![
                answer("box-a", Ok(load(1_000, 100, 0.1, 4))),
                answer("box-b", Ok(load(1_000, 800, 0.1, 4))),
            ],
        };
        assert_eq!(survey.chosen().unwrap().0.as_str(), "box-b");
    }

    #[test]
    fn a_host_with_free_memory_and_saturated_cpus_is_not_chosen() {
        let survey = Survey {
            answers: vec![
                answer("box-a", Ok(load(1_000, 700, 8.0, 4))),
                answer("box-b", Ok(load(1_000, 400, 0.5, 4))),
            ],
        };
        assert_eq!(survey.chosen().unwrap().0.as_str(), "box-b");
    }

    #[test]
    fn of_two_equally_busy_hosts_the_one_with_more_free_memory_is_chosen() {
        let survey = Survey {
            answers: vec![
                answer("box-a", Ok(load(1_000, 500, 0.0, 4))),
                answer("box-b", Ok(load(2_000, 1_000, 0.0, 4))),
            ],
        };
        assert_eq!(survey.chosen().unwrap().0.as_str(), "box-b");
    }

    #[test]
    fn of_two_identical_hosts_the_first_in_the_configuration_is_chosen() {
        let survey = Survey {
            answers: vec![
                answer("box-b", Ok(load(1_000, 500, 0.0, 4))),
                answer("box-a", Ok(load(1_000, 500, 0.0, 4))),
            ],
        };
        assert_eq!(survey.chosen().unwrap().0.as_str(), "box-b");
    }

    #[test]
    fn a_host_that_did_not_answer_is_never_chosen() {
        let survey = Survey {
            answers: vec![
                answer("box-a", Err(HostFailure::TimedOut(Duration::from_secs(10)))),
                answer("box-b", Ok(load(1_000, 10, 3.0, 1))),
            ],
        };
        assert_eq!(survey.chosen().unwrap().0.as_str(), "box-b");
        let only_failures = Survey {
            answers: vec![answer(
                "box-a",
                Err(HostFailure::TimedOut(Duration::from_secs(10))),
            )],
        };
        assert!(only_failures.chosen().is_none());
    }

    #[test]
    fn the_summary_gives_the_busy_value_the_memory_and_the_load() {
        let summary = load(16_318_592, 12_237_944, 0.52, 4).summary();
        assert_eq!(
            summary,
            "25% busy: 11.7 of 15.6 GiB memory free, load 0.52 on 4 CPUs"
        );
        assert!(load(1_048_576, 524_288, 0.0, 1).summary().ends_with("on 1 CPU"));
    }

    #[test]
    fn the_table_shows_every_host_and_marks_the_chosen_one() {
        let survey = Survey {
            answers: vec![
                answer("box-a", Ok(load(16_318_592, 1_631_859, 0.4, 4))),
                answer("box-b", Ok(load(16_318_592, 12_237_944, 0.52, 4))),
                answer("box-c", Err(HostFailure::TimedOut(Duration::from_secs(10)))),
            ],
        };
        let table = survey.table();
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 4, "{table}");
        assert!(lines[0].starts_with("HOST"), "{table}");
        assert!(lines[1].starts_with("box-a") && lines[1].contains("90%"), "{table}");
        assert!(!lines[1].contains("chosen"), "{table}");
        assert!(lines[2].starts_with("box-b") && lines[2].ends_with("chosen"), "{table}");
        assert!(
            lines[3].starts_with("box-c") && lines[3].ends_with("no answer in 10 s"),
            "{table}"
        );
    }
}
