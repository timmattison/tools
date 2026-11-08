//! `faulte check`: one fast answer to the question "why is the system time of
//! this Mac high now?"
//!
//! The ranking of `faulte` runs `top` and `ps`, and one `ps` took up to 13
//! seconds on this Mac at its slowest. The check starts no `top` and no `ps`.
//! It reads the CPU counters and the compressor counters of the kernel, and it
//! starts one short process to learn the newest process number. Thus it
//! answers when the Mac is at its slowest.
//!
//! This Mac showed two states on 2026-11-08, both at 0% idle:
//!
//! - With 32 GB in the compressor, system time was 52% to 66% over 12 samples
//!   of 5 seconds. It followed the rate of compressions (correlation 0.88) and
//!   of decompressions (0.73), and it did not follow the load average (0.32).
//!   The two compressor rates were almost equal, thus the same pages left
//!   memory and came back. Both rates must be high, because one rate alone is
//!   a one-time movement of pages and not churn.
//! - With 16 GB in the compressor and 14,009 to 51,323 compressions each
//!   second, system time was still 42% to 56%, with 257 to 490 new processes
//!   each second and 7 test runs.
//!
//! The compressor thus explains only part of the system time. The check
//! reports both rates. No measurement proves that new processes cause the
//! rest, because that needs a kernel profile, and a profile needs root.
//!
//! The rules are pure functions over plain values. [`CheckSource`] is the one
//! boundary to the kernel, thus a test gives each counter as a plain value.

use std::time::{Duration, Instant};

use crate::machine::MachineError;
use crate::render;

/// The share of all CPU time in the kernel, in percent, at which this Mac has
/// a problem.
///
/// At 30%, more than 4 of the 14 cores of this Mac run kernel code. The bad
/// samples of 2026-11-08 were between 44% and 66%. No sample of a quiet period
/// set this limit, thus a future measurement can change it.
pub const SYSTEM_PERCENT_LIMIT: f64 = 30.0;

/// The rate of compressions, and of decompressions, at which the compressor
/// churns, in pages each second.
///
/// On 2026-11-08, the bad samples of this Mac were between 151,721 and 290,697
/// compressions each second, and the mean over 10.9 days was 13,145. This limit
/// is about 4 times that mean and one third of the lowest bad sample. No sample
/// of a quiet period set this limit, thus a future measurement can change it.
pub const CHURN_LIMIT_PER_SECOND: f64 = 50_000.0;

/// The rate of new processes at which process creation is a cause of system
/// time, in processes each second.
///
/// On 2026-11-08, the bad samples of this Mac were between 193 and 490 new
/// processes each second. This limit is about three quarters of the lowest bad
/// sample. No sample of a quiet period set this limit, thus a future
/// measurement can change it.
pub const PROCESS_LIMIT_PER_SECOND: f64 = 150.0;

/// The largest process number of a Mac.
pub const PID_SPACE: u64 = 99_999;

/// The name of [`Verdict::ProcessChurn`].
const PROCESS_NAME: &str = "PROCESS CHURN";

/// What a person reads after [`Verdict::ProcessChurn`].
const PROCESS_ADVICE: &str = "New processes use kernel time. On this Mac, test runs and hooks start most of them. Stop the test runs that you do not need.";

/// The name of [`Verdict::Healthy`].
const HEALTHY_NAME: &str = "OK";

/// The name of [`Verdict::MemoryChurn`].
const CHURN_NAME: &str = "MEMORY CHURN";

/// The name of [`Verdict::OtherSystemTime`].
const OTHER_NAME: &str = "OTHER SYSTEM TIME";

/// The text before the verdict on the first line of a report.
const REPORT_PREFIX: &str = "faulte check: ";

/// The width of the verdict column of a watch: the longest name.
const NAME_WIDTH: usize = OTHER_NAME.len();

/// What a person reads after [`Verdict::Healthy`].
const HEALTHY_ADVICE: &str = "The system time is below its limit.";

/// What a person reads after [`Verdict::MemoryChurn`].
const CHURN_ADVICE: &str = "The kernel moves pages in and out of compressed memory. Run `faulte` to rank the processes by page faults, and `faulte kill` to stop the old idle Claude Code sessions.";

/// What a person reads after [`Verdict::OtherSystemTime`].
const OTHER_ADVICE: &str =
    "The compressor and new processes are below their limits. `faulte check` does not measure the other causes of system time.";

/// The counters that one read of the kernel gives.
///
/// Each one counts from the time that the Mac started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CheckCounters {
    /// The ticks of all cores in user code.
    pub user_ticks: u32,
    /// The ticks of all cores in user code at a low priority.
    pub nice_ticks: u32,
    /// The ticks of all cores in kernel code.
    pub system_ticks: u32,
    /// The ticks of all cores with no work.
    pub idle_ticks: u32,
    /// The pages that the compressor compressed.
    pub compressions: u64,
    /// The pages that the compressor decompressed.
    pub decompressions: u64,
    /// The newest process number that the kernel gave.
    pub newest_pid: u32,
}

/// What the counters say over one interval.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Reading {
    /// The share of CPU time in user code, in percent. It includes low
    /// priority code.
    pub user_percent: f64,
    /// The share of CPU time in kernel code, in percent.
    pub system_percent: f64,
    /// The share of CPU time with no work, in percent.
    pub idle_percent: f64,
    /// The pages that the compressor compressed each second.
    pub compressions_per_second: f64,
    /// The pages that the compressor decompressed each second.
    pub decompressions_per_second: f64,
    /// The processes that started each second.
    pub new_processes_per_second: f64,
}

impl Reading {
    /// Gives the reading from the counters `before` an interval to the
    /// counters `after` it, over `elapsed`.
    ///
    /// The kernel counts ticks in 32 bits, thus a tick counter can wrap, and
    /// the difference wraps with it. A compressor counter has 64 bits, thus a
    /// lower value means that the Mac started again, and it gives no rate.
    /// Process numbers start again after [`PID_SPACE`]. An interval long
    /// enough for two such starts counts too few processes.
    /// An interval of no time gives zeros, not a division by zero.
    #[must_use]
    pub fn between(before: CheckCounters, after: CheckCounters, elapsed: Duration) -> Self {
        let user = f64::from(after.user_ticks.wrapping_sub(before.user_ticks));
        let nice = f64::from(after.nice_ticks.wrapping_sub(before.nice_ticks));
        let system = f64::from(after.system_ticks.wrapping_sub(before.system_ticks));
        let idle = f64::from(after.idle_ticks.wrapping_sub(before.idle_ticks));
        let total = user + nice + system + idle;
        let percent = |ticks: f64| {
            if total > 0.0 {
                ticks / total * 100.0
            } else {
                0.0
            }
        };
        let started_again = if after.newest_pid < before.newest_pid {
            PID_SPACE
        } else {
            0
        };
        let seconds = elapsed.as_secs_f64();
        let rate = |first: u64, last: u64| {
            if seconds > 0.0 {
                last.saturating_sub(first) as f64 / seconds
            } else {
                0.0
            }
        };
        Self {
            user_percent: percent(user + nice),
            system_percent: percent(system),
            idle_percent: percent(idle),
            compressions_per_second: rate(before.compressions, after.compressions),
            decompressions_per_second: rate(before.decompressions, after.decompressions),
            new_processes_per_second: rate(
                u64::from(before.newest_pid),
                u64::from(after.newest_pid) + started_again,
            ),
        }
    }
}

/// What one check decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// System time is below its limit.
    Healthy,
    /// System time is high, and pages leave and return to compressed memory.
    MemoryChurn,
    /// System time is high, the compressor is quiet, and many processes start.
    ProcessChurn,
    /// System time is high, and neither the compressor nor new processes are
    /// the cause.
    OtherSystemTime,
}

impl Verdict {
    /// Tells if this verdict is a problem, which makes the exit code 1.
    #[must_use]
    pub fn is_problem(self) -> bool {
        self != Self::Healthy
    }

    /// Gives the name of this verdict.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Healthy => HEALTHY_NAME,
            Self::MemoryChurn => CHURN_NAME,
            Self::ProcessChurn => PROCESS_NAME,
            Self::OtherSystemTime => OTHER_NAME,
        }
    }

    /// Gives the sentences that a person reads after this verdict.
    #[must_use]
    pub fn advice(self) -> &'static str {
        match self {
            Self::Healthy => HEALTHY_ADVICE,
            Self::MemoryChurn => CHURN_ADVICE,
            Self::ProcessChurn => PROCESS_ADVICE,
            Self::OtherSystemTime => OTHER_ADVICE,
        }
    }
}

/// Gives the verdict of `reading`.
///
/// A value at a limit counts as over that limit. High compressor rates with a
/// low system time are healthy, because the CPU is not in the kernel.
#[must_use]
pub fn judge(reading: &Reading) -> Verdict {
    if reading.system_percent < SYSTEM_PERCENT_LIMIT {
        return Verdict::Healthy;
    }
    if reading.compressions_per_second >= CHURN_LIMIT_PER_SECOND
        && reading.decompressions_per_second >= CHURN_LIMIT_PER_SECOND
    {
        Verdict::MemoryChurn
    } else if reading.new_processes_per_second >= PROCESS_LIMIT_PER_SECOND {
        Verdict::ProcessChurn
    } else {
        Verdict::OtherSystemTime
    }
}

/// One finished check.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Check {
    /// What the counters said.
    pub reading: Reading,
    /// What the rules decided.
    pub verdict: Verdict,
    /// The time from the first read to the second read.
    pub elapsed: Duration,
}

/// The facts of this Mac that a check reads.
pub trait CheckSource {
    /// Reads the counters of the kernel.
    ///
    /// # Errors
    ///
    /// Gives an error when the kernel refuses the read.
    fn counters(&self) -> Result<CheckCounters, MachineError>;

    /// Gives the time now, on a clock that only moves forward.
    fn now(&self) -> Instant;

    /// Waits for `span`.
    fn sleep(&self, span: Duration);
}

/// Takes one check after the other, with no gap between them.
#[derive(Debug)]
pub struct Sampler<S> {
    source: S,
    previous: CheckCounters,
    previous_at: Instant,
}

impl<S: CheckSource> Sampler<S> {
    /// Takes the first read.
    ///
    /// # Errors
    ///
    /// Gives an error when the kernel refuses the read.
    pub fn start(source: S) -> Result<Self, MachineError> {
        let previous = source.counters()?;
        let previous_at = source.now();
        Ok(Self {
            source,
            previous,
            previous_at,
        })
    }

    /// Waits for `interval` and gives the check since the last read.
    ///
    /// The read that ends this check starts the next one, thus a run with
    /// `--watch` loses no time between two lines.
    ///
    /// # Errors
    ///
    /// Gives an error when the kernel refuses the read.
    pub fn sample(&mut self, interval: Duration) -> Result<Check, MachineError> {
        self.source.sleep(interval);
        let after = self.source.counters()?;
        let at = self.source.now();
        let elapsed = at.saturating_duration_since(self.previous_at);
        let reading = Reading::between(self.previous, after, elapsed);
        self.previous = after;
        self.previous_at = at;
        Ok(Check {
            reading,
            verdict: judge(&reading),
            elapsed,
        })
    }
}

/// Gives the lines that a person reads after one check.
///
/// The first line is the verdict, thus a script can read one line.
#[must_use]
pub fn report(check: &Check) -> Vec<String> {
    let reading = &check.reading;
    vec![
        format!("{REPORT_PREFIX}{}", check.verdict.name()),
        format!("window: {:.1} s", check.elapsed.as_secs_f64()),
        format!(
            "CPU: {:.0}% system (limit {SYSTEM_PERCENT_LIMIT:.0}%), {:.0}% user, {:.0}% idle",
            reading.system_percent, reading.user_percent, reading.idle_percent,
        ),
        format!(
            "compressor: {} compressions/s, {} decompressions/s (limit {}/s each)",
            render::rate(reading.compressions_per_second),
            render::rate(reading.decompressions_per_second),
            render::rate(CHURN_LIMIT_PER_SECOND),
        ),
        format!(
            "processes: {} new processes/s (limit {}/s)",
            render::rate(reading.new_processes_per_second),
            render::rate(PROCESS_LIMIT_PER_SECOND),
        ),
        check.verdict.advice().to_owned(),
    ]
}

/// Gives the one line of a run with `--watch`. `clock` is the time of the line.
#[must_use]
pub fn watch_line(check: &Check, clock: &str) -> String {
    let reading = &check.reading;
    format!(
        "{clock}  {:<NAME_WIDTH$}  system {:>3.0}%  user {:>3.0}%  idle {:>3.0}%  compress {}/s  decompress {}/s  new {}/s",
        check.verdict.name(),
        reading.system_percent,
        reading.user_percent,
        reading.idle_percent,
        render::rate(reading.compressions_per_second),
        render::rate(reading.decompressions_per_second),
        render::rate(reading.new_processes_per_second),
    )
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    use super::*;

    /// The longest difference that counts as equal for two percent values.
    const TOLERANCE: f64 = 1e-9;

    /// The interval of each sample in these tests.
    const FIVE_SECONDS: Duration = Duration::from_secs(5);

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < TOLERANCE,
            "expected {expected}, got {actual}"
        );
    }

    /// A reading with one system share and one rate for both compressor
    /// directions.
    fn reading(system_percent: f64, churn: f64) -> Reading {
        Reading {
            user_percent: 100.0 - system_percent,
            system_percent,
            idle_percent: 0.0,
            compressions_per_second: churn,
            decompressions_per_second: churn,
            new_processes_per_second: 0.0,
        }
    }

    /// A reading with a quiet compressor and one rate of new processes.
    fn busy_with_processes(system_percent: f64, new_processes: f64) -> Reading {
        Reading {
            new_processes_per_second: new_processes,
            ..reading(system_percent, 1_000.0)
        }
    }

    /// The kernel gives each new process the next number, thus the change of
    /// the newest number counts the new processes.
    #[test]
    fn the_change_of_the_newest_pid_counts_the_new_processes() {
        let before = CheckCounters {
            newest_pid: 10_000,
            ..CheckCounters::default()
        };
        let after = CheckCounters {
            newest_pid: 11_620,
            ..CheckCounters::default()
        };

        let result = Reading::between(before, after, FIVE_SECONDS);

        assert_close(result.new_processes_per_second, 324.0);
    }

    /// The numbers of a Mac start again after 99,999. The processes after that
    /// point still count.
    #[test]
    fn a_process_number_that_starts_again_still_counts() {
        let before = CheckCounters {
            newest_pid: 99_900,
            ..CheckCounters::default()
        };
        let after = CheckCounters {
            newest_pid: 151,
            ..CheckCounters::default()
        };

        let result = Reading::between(before, after, FIVE_SECONDS);

        assert_close(result.new_processes_per_second, 50.0);
    }

    /// The values are a sample of this Mac with a quiet compressor.
    #[test]
    fn high_system_time_with_many_new_processes_is_process_churn() {
        assert_eq!(
            judge(&busy_with_processes(43.0, 490.0)),
            Verdict::ProcessChurn
        );
    }

    /// A rate at the limit counts as over it, and a rate below it does not.
    #[test]
    fn the_rate_of_new_processes_at_its_limit_is_over_it() {
        assert_eq!(
            judge(&busy_with_processes(43.0, PROCESS_LIMIT_PER_SECOND)),
            Verdict::ProcessChurn
        );
        assert_eq!(
            judge(&busy_with_processes(43.0, PROCESS_LIMIT_PER_SECOND - 1.0)),
            Verdict::OtherSystemTime
        );
    }

    /// Memory churn names the larger cost, and the report shows both numbers.
    #[test]
    fn memory_churn_comes_before_process_churn() {
        let both = Reading {
            new_processes_per_second: 490.0,
            ..reading(58.0, 212_384.0)
        };

        assert_eq!(judge(&both), Verdict::MemoryChurn);
    }

    /// With system time below its limit, new processes do not slow this Mac.
    #[test]
    fn low_system_time_is_healthy_whatever_the_processes_do() {
        assert_eq!(judge(&busy_with_processes(29.9, 490.0)), Verdict::Healthy);
    }

    /// The share of each CPU state is its part of all ticks, and a rate is the
    /// change of a counter divided by the time. The result is a sample that
    /// this Mac gave on 2026-11-08.
    #[test]
    fn a_reading_gives_the_share_of_each_cpu_state_and_each_rate() {
        let before = CheckCounters {
            user_ticks: 1_000,
            nice_ticks: 500,
            system_ticks: 2_000,
            idle_ticks: 3_000,
            compressions: 10_000_000,
            decompressions: 20_000_000,
            ..CheckCounters::default()
        };
        let after = CheckCounters {
            user_ticks: 1_400,
            nice_ticks: 510,
            system_ticks: 2_580,
            idle_ticks: 3_010,
            compressions: 11_061_920,
            decompressions: 21_017_675,
            ..CheckCounters::default()
        };

        let result = Reading::between(before, after, FIVE_SECONDS);

        assert_close(result.user_percent, 41.0);
        assert_close(result.system_percent, 58.0);
        assert_close(result.idle_percent, 1.0);
        assert_close(result.compressions_per_second, 212_384.0);
        assert_close(result.decompressions_per_second, 203_535.0);
    }

    /// The kernel counts ticks in 32 bits, and a busy Mac wraps that counter.
    /// The ticks after the wrap still count.
    #[test]
    fn a_tick_counter_that_wraps_still_counts() {
        let before = CheckCounters {
            system_ticks: u32::MAX - 9,
            idle_ticks: 100,
            ..CheckCounters::default()
        };
        let after = CheckCounters {
            system_ticks: 10,
            idle_ticks: 120,
            ..CheckCounters::default()
        };

        let result = Reading::between(before, after, FIVE_SECONDS);

        assert_close(result.system_percent, 50.0);
        assert_close(result.idle_percent, 50.0);
    }

    /// A compressor counter that goes down means that the Mac started again
    /// between the two reads.
    #[test]
    fn a_compressor_counter_that_goes_down_gives_no_rate() {
        let before = CheckCounters {
            compressions: 500,
            decompressions: 700,
            ..CheckCounters::default()
        };

        let result = Reading::between(before, CheckCounters::default(), FIVE_SECONDS);

        assert_close(result.compressions_per_second, 0.0);
        assert_close(result.decompressions_per_second, 0.0);
    }

    /// An interval of no time and no ticks gives zeros, not infinity.
    #[test]
    fn an_interval_of_no_time_gives_zeros() {
        let counters = CheckCounters {
            compressions: 1,
            ..CheckCounters::default()
        };

        let result = Reading::between(CheckCounters::default(), counters, Duration::ZERO);

        assert_close(result.system_percent, 0.0);
        assert_close(result.compressions_per_second, 0.0);
    }

    /// The values are a sample of the bad state of this Mac.
    #[test]
    fn high_system_time_with_churn_is_memory_churn() {
        assert_eq!(judge(&reading(58.0, 212_384.0)), Verdict::MemoryChurn);
    }

    /// A value at each limit counts as over that limit.
    #[test]
    fn a_value_at_each_limit_is_over_it() {
        assert_eq!(
            judge(&reading(SYSTEM_PERCENT_LIMIT, CHURN_LIMIT_PER_SECOND)),
            Verdict::MemoryChurn
        );
    }

    /// High system time with a quiet compressor has a different cause.
    #[test]
    fn high_system_time_with_a_quiet_compressor_is_another_cause() {
        assert_eq!(
            judge(&reading(58.0, CHURN_LIMIT_PER_SECOND - 1.0)),
            Verdict::OtherSystemTime
        );
    }

    /// Churn needs both directions. Pages that only leave memory are one
    /// movement, and they do not come back.
    #[test]
    fn one_compressor_direction_alone_is_not_churn() {
        let one_way = Reading {
            decompressions_per_second: 1_000.0,
            ..reading(58.0, 212_384.0)
        };

        assert_eq!(judge(&one_way), Verdict::OtherSystemTime);
    }

    /// With system time below its limit, the CPU is not in the kernel, and a
    /// busy compressor does not slow this Mac.
    #[test]
    fn low_system_time_is_healthy_whatever_the_compressor_does() {
        assert_eq!(judge(&reading(29.9, 212_384.0)), Verdict::Healthy);
    }

    /// Only a healthy verdict gives the exit code 0.
    #[test]
    fn each_verdict_other_than_healthy_is_a_problem() {
        assert!(!Verdict::Healthy.is_problem());
        assert!(Verdict::MemoryChurn.is_problem());
        assert!(Verdict::ProcessChurn.is_problem());
        assert!(Verdict::OtherSystemTime.is_problem());
    }

    /// A source that gives planned reads and a clock that moves by each wait.
    struct FakeSource {
        reads: RefCell<VecDeque<Result<CheckCounters, MachineError>>>,
        start: Instant,
        waited: Cell<Duration>,
    }

    impl FakeSource {
        fn new(reads: Vec<Result<CheckCounters, MachineError>>) -> Self {
            Self {
                reads: RefCell::new(reads.into()),
                start: Instant::now(),
                waited: Cell::new(Duration::ZERO),
            }
        }
    }

    impl CheckSource for FakeSource {
        fn counters(&self) -> Result<CheckCounters, MachineError> {
            self.reads
                .borrow_mut()
                .pop_front()
                .expect("the test planned this read")
        }

        fn now(&self) -> Instant {
            self.start + self.waited.get()
        }

        fn sleep(&self, span: Duration) {
            self.waited.set(self.waited.get() + span);
        }
    }

    /// Counters with `compressions` and the same value for decompressions, and
    /// one second of system time for each compression.
    fn counters(compressions: u64) -> CheckCounters {
        CheckCounters {
            system_ticks: u32::try_from(compressions).expect("a small test value"),
            compressions,
            decompressions: compressions,
            ..CheckCounters::default()
        }
    }

    /// A check measures from the first read to the read after the wait, and
    /// it judges what it measured.
    #[test]
    fn a_sample_measures_from_the_read_before_the_wait_to_the_read_after_it() {
        let source = FakeSource::new(vec![Ok(counters(0)), Ok(counters(1_000_000))]);
        let mut sampler = Sampler::start(source).expect("the first read works");

        let check = sampler.sample(FIVE_SECONDS).expect("the second read works");

        assert_eq!(check.elapsed, FIVE_SECONDS);
        assert_close(check.reading.compressions_per_second, 200_000.0);
        assert_close(check.reading.system_percent, 100.0);
        assert_eq!(check.verdict, Verdict::MemoryChurn);
    }

    /// The read that ends one check starts the next one, thus no interval is
    /// lost and no interval counts two times.
    #[test]
    fn each_sample_starts_where_the_last_one_ended() {
        let source = FakeSource::new(vec![
            Ok(counters(0)),
            Ok(counters(1_000_000)),
            Ok(counters(1_500_000)),
        ]);
        let mut sampler = Sampler::start(source).expect("the first read works");
        sampler.sample(FIVE_SECONDS).expect("the second read works");

        let check = sampler.sample(FIVE_SECONDS).expect("the third read works");

        assert_close(check.reading.compressions_per_second, 100_000.0);
    }

    /// A read that failed is an error. A healthy verdict from no data looks
    /// the same as a Mac with no problem.
    #[test]
    fn a_read_that_failed_is_an_error_and_not_a_verdict() {
        let refused = MachineError::KernelRead {
            call: "host_statistics".to_owned(),
            reason: "the call ended with 5".to_owned(),
        };
        let source = FakeSource::new(vec![Ok(counters(0)), Err(refused.clone())]);
        let mut sampler = Sampler::start(source).expect("the first read works");

        let error = sampler.sample(FIVE_SECONDS).expect_err("the read failed");

        assert_eq!(error, refused);
    }

    /// The report starts with the verdict, states each number with its limit,
    /// and ends with what to do.
    #[test]
    fn the_report_states_the_verdict_each_number_and_the_advice() {
        let check = Check {
            reading: Reading {
                user_percent: 41.0,
                system_percent: 58.0,
                idle_percent: 1.0,
                compressions_per_second: 212_384.0,
                decompressions_per_second: 203_535.0,
                new_processes_per_second: 324.0,
            },
            verdict: Verdict::MemoryChurn,
            elapsed: FIVE_SECONDS,
        };

        let lines = report(&check);
        let text = lines.join("\n");

        assert_eq!(
            lines.first().map(String::as_str),
            Some("faulte check: MEMORY CHURN")
        );
        for expected in [
            "5.0 s",
            "58% system (limit 30%)",
            "41% user",
            "1% idle",
            "212,384 compressions/s",
            "203,535 decompressions/s",
            "limit 50,000/s each",
            "324 new processes/s (limit 150/s)",
            "`faulte kill`",
        ] {
            assert!(text.contains(expected), "{expected:?} is not in:\n{text}");
        }
    }

    /// One line of a watch holds the time, the verdict, and each number.
    #[test]
    fn a_watch_line_holds_the_clock_the_verdict_and_each_number() {
        let check = Check {
            reading: Reading {
                new_processes_per_second: 324.0,
                ..reading(58.0, 212_384.0)
            },
            verdict: Verdict::MemoryChurn,
            elapsed: FIVE_SECONDS,
        };

        let line = watch_line(&check, "16:02:31");

        for expected in [
            "16:02:31",
            "MEMORY CHURN",
            "system  58%",
            "212,384/s",
            "new 324/s",
        ] {
            assert!(line.contains(expected), "{expected:?} is not in: {line}");
        }
        assert!(!line.contains('\n'), "the line is one line: {line:?}");
    }
}
