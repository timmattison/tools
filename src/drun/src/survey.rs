//! Asks each host how busy it is, and chooses the least busy host.
//!
//! `drun` starts one small `busybox` container on each host, all at the same
//! time. The container prints the memory figures of the host, its load
//! average, and its count of CPUs. Docker removes the container when it stops.
//! The probe never pulls its image. A host that does not have the image gets
//! it in a separate pull, which has its own limit, and then the probe again.
//! On a Linux host the figures are those of the host. On a host that runs
//! Docker Desktop they are those of the virtual machine that runs the
//! containers, which is the memory that a container can use.
//!
//! A host is as busy as the larger of two fractions: the memory in use, and
//! the one-minute load average for each CPU. The least busy host has the
//! lowest value. When two hosts have the same value, the host with more free
//! memory wins, and after that the host that is first in the configuration.

use std::io::Write;
use std::time::Duration;

use crate::config::Config;
use crate::host::{self, Answer, ContextName, HostFailure};
use crate::table;

/// The image of the probe container. It is small, and it has `sh`, `cat`,
/// and `nproc` for every CPU architecture that Docker supports.
pub const PROBE_IMAGE: &str = "busybox:1.37";

/// How long `drun` waits for a host to pull [`PROBE_IMAGE`]. A pull goes to a
/// registry, so it gets more time than the probe. Each run of `drun` waits up
/// to this limit for a host that does not have the image and whose registry
/// does not answer.
pub const PULL_TIMEOUT: Duration = Duration::from_secs(120);

/// The words of the Docker engine when a host does not have an image, as in
/// `Error response from daemon: No such image: busybox:1.37`.
const NO_SUCH_IMAGE: &str = "No such image";

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
    one_minute_average: f64,
    cpus: u32,
}

/// The answers of all hosts to the probe.
#[derive(Debug)]
pub struct Survey {
    answers: Vec<Answer<Load>>,
}

/// Asks every host in `config` how busy it is, all at the same time.
pub fn survey(config: &Config) -> Survey {
    let answers = host::ask_all(config.contexts(), |context| {
        survey_host(context, config.host_timeout(), host::ask)
    });
    Survey { answers }
}

/// Asks the host `context` how busy it is.
///
/// The probe never pulls its image, so `host_timeout` limits only the probe.
/// When the probe fails, `drun` asks the host if it has [`PROBE_IMAGE`]. When
/// the host does not have it, `drun` pulls it with the limit [`PULL_TIMEOUT`],
/// and then runs the probe again. When a probe or an image inspect gets no
/// answer in `host_timeout`, `drun` does not pull: a host that does not answer
/// a small question does not answer a pull either.
///
/// `ask` runs one docker command on a host, as [`host::ask`] does, and the
/// tests give their own.
///
/// # Errors
///
/// Returns the [`HostFailure`] of the probe, [`HostFailure::PullFailed`] when
/// the host does not have the image and the pull fails, or
/// [`HostFailure::Unreadable`] when `drun` cannot read the answer of the probe.
fn survey_host<A>(
    context: &ContextName,
    host_timeout: Duration,
    ask: A,
) -> Result<Load, HostFailure>
where
    A: Fn(&ContextName, &[&str], Duration) -> Result<String, HostFailure>,
{
    let script = probe_script();
    let probe = [
        "run",
        "--rm",
        "--pull",
        "never",
        "--network",
        "none",
        "--label",
        PROBE_LABEL,
        PROBE_IMAGE,
        "sh",
        "-c",
        &script,
    ];
    let text = match ask(context, &probe, host_timeout) {
        Ok(text) => text,
        Err(failure @ HostFailure::Failed { .. }) => {
            if !lacks_probe_image(context, host_timeout, &ask) {
                return Err(failure);
            }
            note_pull(context);
            ask(context, &["pull", PROBE_IMAGE], PULL_TIMEOUT).map_err(|cause| {
                HostFailure::PullFailed {
                    context: context.clone(),
                    image: PROBE_IMAGE.to_owned(),
                    cause: Box::new(cause),
                }
            })?;
            ask(context, &probe, host_timeout)?
        }
        Err(failure) => return Err(failure),
    };
    Load::parse(&text).map_err(HostFailure::Unreadable)
}

/// Tells the person on stderr that `drun` pulls [`PROBE_IMAGE`] on the host
/// `context`, because the pull can hold `drun` for up to [`PULL_TIMEOUT`].
/// The survey runs on a thread for each host, so a stderr that is closed is
/// not a reason to panic. The note is then lost.
fn note_pull(context: &ContextName) {
    let _ = writeln!(
        std::io::stderr(),
        "drun: {context} does not have {PROBE_IMAGE}. drun pulls it now, with a limit of {} s",
        PULL_TIMEOUT.as_secs()
    );
}

/// `true` when the host `context` answers that it does not have
/// [`PROBE_IMAGE`].
///
/// `docker image inspect` gives exit status 1 and the stdout `[]` for a host
/// without the image, and also for a host whose engine does not answer. Only
/// the error text of the engine tells the two apart: [`NO_SUCH_IMAGE`].
/// Measured with the docker 29.9.2 CLI and the docker 27.5.1 engine. A host
/// that gives a different answer, or no answer in `limit`, is not a host
/// without the image, so `drun` does not pull there.
fn lacks_probe_image<A>(context: &ContextName, limit: Duration, ask: &A) -> bool
where
    A: Fn(&ContextName, &[&str], Duration) -> Result<String, HostFailure>,
{
    matches!(
        ask(context, &["image", "inspect", PROBE_IMAGE], limit),
        Err(HostFailure::Failed { detail, .. }) if detail.contains(NO_SUCH_IMAGE)
    )
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
        let mut sections: Vec<Vec<&str>> = vec![Vec::new()];
        for line in text.lines() {
            if line.trim() == SECTION_MARK {
                sections.push(Vec::new());
            } else if let Some(section) = sections.last_mut() {
                section.push(line);
            }
        }
        let [meminfo, loadavg, nproc] = sections.as_slice() else {
            return Err(format!("the probe printed {} parts, not 3", sections.len()));
        };
        let memory_total_kib = meminfo_kib(meminfo, "MemTotal")?;
        if memory_total_kib == 0 {
            return Err("MemTotal is 0".to_owned());
        }
        let memory_available_kib = meminfo_kib(meminfo, "MemAvailable")?;
        let load_text = first_word(loadavg).ok_or("the load average is missing")?;
        let one_minute_average: f64 = load_text
            .parse()
            .ok()
            .filter(|load: &f64| load.is_finite() && *load >= 0.0)
            .ok_or_else(|| format!("the load average \"{load_text}\" is not a number"))?;
        let cpus_text = first_word(nproc).ok_or("the count of CPUs is missing")?;
        let cpus: u32 = cpus_text
            .parse()
            .ok()
            .filter(|cpus| *cpus > 0)
            .ok_or_else(|| format!("the count of CPUs \"{cpus_text}\" is not a number above 0"))?;
        Ok(Self {
            memory_total_kib,
            memory_available_kib,
            one_minute_average,
            cpus,
        })
    }

    /// The fraction of the memory in use, from 0 to 1.
    fn memory_in_use(&self) -> f64 {
        (1.0 - float(self.memory_available_kib) / float(self.memory_total_kib)).clamp(0.0, 1.0)
    }

    /// The one-minute load average for each CPU. It is more than 1 when more
    /// work waits than the CPUs can do.
    fn cpu_in_use(&self) -> f64 {
        self.one_minute_average / f64::from(self.cpus)
    }

    /// How busy the host is: the larger of [`Self::memory_in_use`] and
    /// [`Self::cpu_in_use`].
    pub fn busy(&self) -> f64 {
        self.memory_in_use().max(self.cpu_in_use())
    }

    /// One line that says how busy the host is, for a person to read.
    pub fn summary(&self) -> String {
        format!(
            "{} busy: {} memory free, load {:.2} on {}",
            self.busy_text(),
            self.memory_text(),
            self.one_minute_average,
            self.cpus_text()
        )
    }

    /// The busy value as a percentage, as in `25%`.
    fn busy_text(&self) -> String {
        format!("{:.0}%", self.busy() * 100.0)
    }

    /// The free memory and the total memory, as in `11.7 of 15.6 GiB`.
    fn memory_text(&self) -> String {
        format!(
            "{:.1} of {:.1} GiB",
            float(self.memory_available_kib) / KIB_PER_GIB,
            float(self.memory_total_kib) / KIB_PER_GIB
        )
    }

    /// The count of CPUs with its noun, as in `4 CPUs`.
    fn cpus_text(&self) -> String {
        let noun = if self.cpus == 1 { "CPU" } else { "CPUs" };
        format!("{} {noun}", self.cpus)
    }
}

/// `value` as a floating-point number.
#[expect(
    clippy::cast_precision_loss,
    reason = "memory in KiB stays far below 2^52, where f64 starts to lose whole numbers"
)]
fn float(value: u64) -> f64 {
    value as f64
}

/// The value in KiB of the `key` line of `/proc/meminfo`, as in
/// `MemTotal:       16318592 kB`.
fn meminfo_kib(lines: &[&str], key: &str) -> Result<u64, String> {
    let rest = lines
        .iter()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
        .ok_or_else(|| format!("{key} is missing"))?;
    let value = rest
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("{key} has no value"))?;
    value
        .parse()
        .map_err(|_| format!("{key} \"{value}\" is not a number"))
}

/// The first word of the first line in `lines` that is not empty.
fn first_word<'a>(lines: &[&'a str]) -> Option<&'a str> {
    lines.iter().find_map(|line| line.split_whitespace().next())
}

impl Survey {
    /// The least busy host that answered, or `None` when no host answered.
    pub fn chosen(&self) -> Option<(&ContextName, &Load)> {
        self.answers
            .iter()
            .filter_map(|answer| {
                answer
                    .result
                    .as_ref()
                    .ok()
                    .map(|load| (&answer.context, load))
            })
            .min_by(|(_, left), (_, right)| {
                left.busy()
                    .total_cmp(&right.busy())
                    .then_with(|| right.memory_available_kib.cmp(&left.memory_available_kib))
            })
    }

    /// The hosts that gave no usable answer, and why.
    pub fn failures(&self) -> impl Iterator<Item = (&ContextName, &HostFailure)> {
        self.answers
            .iter()
            .filter_map(|answer| match &answer.result {
                Ok(_) => None,
                Err(failure) => Some((&answer.context, failure)),
            })
    }

    /// A table of all hosts that shows how busy each is and which one is
    /// chosen.
    pub fn table(&self) -> String {
        let chosen = self.chosen().map(|(context, _)| context);
        let rows: Vec<Vec<String>> = self
            .answers
            .iter()
            .map(|answer| match &answer.result {
                Ok(load) => {
                    let mut row = vec![
                        answer.context.to_string(),
                        load.memory_text(),
                        format!("{:.2} / {}", load.one_minute_average, load.cpus_text()),
                        load.busy_text(),
                    ];
                    if chosen == Some(&answer.context) {
                        row.push("chosen".to_owned());
                    }
                    row
                }
                Err(failure) => vec![
                    answer.context.to_string(),
                    "-".to_owned(),
                    "-".to_owned(),
                    "-".to_owned(),
                    failure.to_string(),
                ],
            })
            .collect();
        table::render(&["HOST", "MEMORY FREE", "LOAD", "BUSY"], &rows)
    }
}

#[cfg(test)]
mod tests {
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

    fn load(total_kib: u64, available_kib: u64, one_minute_average: f64, cpus: u32) -> Load {
        Load {
            memory_total_kib: total_kib,
            memory_available_kib: available_kib,
            one_minute_average,
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
        let error =
            Load::parse(&probe_output(meminfo, "0.52 0.58 0.59 1/389 12345", "4")).unwrap_err();
        assert!(error.contains("MemAvailable"), "{error}");
    }

    #[test]
    fn a_probe_output_with_a_wrong_number_is_refused() {
        for (loadavg, nproc) in [
            ("high 0.5 0.5 1/2 3", "4"),
            ("nan 0.5 0.5 1/2 3", "4"),
            ("inf 0.5 0.5 1/2 3", "4"),
            ("-1 0.5 0.5 1/2 3", "4"),
            ("0.5 0.5 0.5 1/2 3", "0"),
            ("0.5 0.5 0.5 1/2 3", ""),
            ("", "4"),
        ] {
            assert!(
                Load::parse(&probe_output(MEMINFO, loadavg, nproc)).is_err(),
                "{loadavg:?} {nproc:?} must be refused"
            );
        }
    }

    #[test]
    fn a_probe_output_with_a_wrong_memory_number_is_refused() {
        for (meminfo, key) in [
            ("MemTotal: 0 kB\nMemAvailable: 0 kB", "MemTotal"),
            ("MemTotal: x kB\nMemAvailable: 100 kB", "MemTotal"),
            ("MemTotal: 1000 kB\nMemAvailable: x kB", "MemAvailable"),
        ] {
            let result = Load::parse(&probe_output(meminfo, "0.52 0.58 0.59 1/389 12345", "4"));
            assert!(
                result.as_ref().is_err_and(|error| error.contains(key)),
                "{meminfo:?} must be refused for {key}: {result:?}"
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
        assert!(load(1_048_576, 524_288, 0.0, 1)
            .summary()
            .ends_with("on 1 CPU"));
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
        assert!(
            lines[1].starts_with("box-a") && lines[1].contains("90%"),
            "{table}"
        );
        assert!(!lines[1].contains("chosen"), "{table}");
        assert!(
            lines[2].starts_with("box-b") && lines[2].ends_with("chosen"),
            "{table}"
        );
        assert!(
            lines[3].starts_with("box-c") && lines[3].ends_with("no answer in 10 s"),
            "{table}"
        );
    }

    /// The limit of the probe in the tests of [`survey_host`].
    const HOST_TIMEOUT: Duration = Duration::from_secs(10);

    /// The limit of the pull that the issue states: 120 s.
    const STATED_PULL_LIMIT: Duration = Duration::from_secs(120);

    /// One docker command that a test host got: its arguments and its limit.
    type Call = (Vec<String>, Duration);

    /// The failure of a docker command on a host that does not have the image
    /// of the probe, with the exit status `code`, as docker 29 gives it.
    fn no_such_image(code: i32) -> HostFailure {
        HostFailure::Failed {
            code: Some(code),
            detail: format!("Error response from daemon: No such image: {PROBE_IMAGE}"),
        }
    }

    /// The step of a call: `probe` for the probe container, else the first
    /// word of the docker command.
    fn step(args: &[String]) -> &str {
        if args.iter().any(|argument| argument == PROBE_LABEL) {
            "probe"
        } else {
            args.first().map_or("", String::as_str)
        }
    }

    /// Asks the host `box-a` how busy it is, with a probe limit of
    /// [`HOST_TIMEOUT`]. `answer` gives the answer to each docker command.
    /// Gives back the result, and each call in order.
    fn survey_with<F>(answer: F) -> (Result<Load, HostFailure>, Vec<Call>)
    where
        F: Fn(&str) -> Result<String, HostFailure>,
    {
        let calls = std::cell::RefCell::new(Vec::new());
        let context = ContextName::parse("box-a").unwrap();
        let result = survey_host(&context, HOST_TIMEOUT, |asked, args, limit| {
            assert_eq!(asked, &context);
            let args: Vec<String> = args.iter().map(|argument| (*argument).to_owned()).collect();
            let reply = answer(step(&args));
            calls.borrow_mut().push((args, limit));
            reply
        });
        (result, calls.into_inner())
    }

    #[test]
    fn a_probe_that_answers_gets_no_image_inspect_and_no_pull() {
        let (result, calls) =
            survey_with(|_| Ok(probe_output(MEMINFO, "0.52 0.58 0.59 1/389 12345", "4")));
        assert_eq!(result, Ok(load(16_318_592, 12_237_944, 0.52, 4)));
        assert_eq!(calls.len(), 1, "{calls:?}");
        let (args, limit) = &calls[0];
        assert_eq!(*limit, HOST_TIMEOUT);
        let pull = args.iter().position(|argument| argument == "--pull");
        assert_eq!(
            pull.and_then(|index| args.get(index + 1))
                .map(String::as_str),
            Some("never"),
            "{args:?}"
        );
    }

    #[test]
    fn a_host_without_the_image_pulls_it_with_its_own_limit_and_probes_again() {
        let pulled = std::cell::Cell::new(false);
        let (result, calls) = survey_with(|step| match step {
            "probe" if pulled.get() => Ok(probe_output(MEMINFO, "0.52 0.58 0.59 1/389 12345", "4")),
            "probe" => Err(no_such_image(125)),
            "image" => Err(no_such_image(1)),
            "pull" => {
                pulled.set(true);
                Ok(String::new())
            }
            other => panic!("unexpected docker command {other}"),
        });
        assert_eq!(result, Ok(load(16_318_592, 12_237_944, 0.52, 4)));
        let steps: Vec<(&str, Duration)> = calls
            .iter()
            .map(|(args, limit)| (step(args), *limit))
            .collect();
        assert_eq!(
            steps,
            [
                ("probe", HOST_TIMEOUT),
                ("image", HOST_TIMEOUT),
                ("pull", STATED_PULL_LIMIT),
                ("probe", HOST_TIMEOUT),
            ]
        );
        assert_eq!(calls[1].0, ["image", "inspect", PROBE_IMAGE]);
        assert_eq!(calls[2].0, ["pull", PROBE_IMAGE]);
    }

    #[test]
    fn a_pull_that_times_out_names_the_pull_command() {
        let (result, calls) = survey_with(|step| match step {
            "probe" => Err(no_such_image(125)),
            "image" => Err(no_such_image(1)),
            "pull" => Err(HostFailure::TimedOut(STATED_PULL_LIMIT)),
            other => panic!("unexpected docker command {other}"),
        });
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains(&format!("docker --context box-a pull {PROBE_IMAGE}")),
            "{message}"
        );
        assert!(message.contains("no answer in 120 s"), "{message}");
        let pulls: Vec<&Call> = calls
            .iter()
            .filter(|(args, _)| step(args) == "pull")
            .collect();
        assert_eq!(pulls.len(), 1, "{calls:?}");
        assert_eq!(pulls[0].1, STATED_PULL_LIMIT);
    }

    #[test]
    fn a_pull_that_fails_names_the_pull_command_and_its_error() {
        let (result, _) = survey_with(|step| match step {
            "probe" => Err(no_such_image(125)),
            "image" => Err(no_such_image(1)),
            "pull" => Err(HostFailure::Failed {
                code: Some(1),
                detail: "Error response from daemon: toomanyrequests: You have reached your \
                         pull rate limit"
                    .to_owned(),
            }),
            other => panic!("unexpected docker command {other}"),
        });
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains(&format!("docker --context box-a pull {PROBE_IMAGE}")),
            "{message}"
        );
        assert!(message.contains("toomanyrequests"), "{message}");
    }

    #[test]
    fn a_probe_failure_with_the_image_present_or_unknown_pulls_nothing() {
        let probe_failure = HostFailure::Failed {
            code: Some(125),
            detail: "docker: Error response from daemon: failed to create task".to_owned(),
        };
        let cannot_connect = HostFailure::Failed {
            code: Some(1),
            detail: "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. \
                     Is the docker daemon running?"
                .to_owned(),
        };
        for (name, inspect) in [
            ("present", Ok("sha256:0b8c1d5e".to_owned())),
            ("unreachable", Err(cannot_connect)),
            ("slow", Err(HostFailure::TimedOut(HOST_TIMEOUT))),
        ] {
            let (result, calls) = survey_with(|step| match step {
                "probe" => Err(probe_failure.clone()),
                "image" => inspect.clone(),
                _ => Ok(String::new()),
            });
            assert_eq!(result, Err(probe_failure.clone()), "{name}");
            assert!(
                calls.iter().all(|(args, _)| step(args) != "pull"),
                "{name}: {calls:?}"
            );
        }
    }

    #[test]
    fn a_probe_that_times_out_asks_the_host_nothing_more() {
        let (result, calls) = survey_with(|_| Err(HostFailure::TimedOut(HOST_TIMEOUT)));
        assert_eq!(result, Err(HostFailure::TimedOut(HOST_TIMEOUT)));
        assert_eq!(calls.len(), 1, "{calls:?}");
    }
}
