//! Tests of what the binary prints, over a machine that the test makes.
//!
//! Every fact is a plain value that the test states. No test here runs `ps`,
//! calls the kernel, reads a process, or reads a clock.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use faulte::duration::Span;
use faulte::pid::Pid;
use faulte::vm::{SwapUsage, VmCounters};
use whyslow::machine::{observe, Machine, SourceError};
use whyslow::render::render;
use whyslow::report::{report, EXIT_SOURCE_FAILED};
use whyslow::sample::ProcessRow;
use whyslow::verdict::summarize;
use whyslow::verdict::{CpuTicks, Pressure};

/// The size of one page of memory of an Apple Silicon Mac, in bytes.
const PAGE_SIZE: u64 = 16_384;

/// The text that each failed source of the tests states.
const REFUSED: &str = "refused by the test";

/// Gives the interval that the person asked for in the tests.
fn interval() -> Span {
    "2s".parse().expect("2s is a span")
}

/// Gives the error that a failed source of the tests gives.
fn refused(call: &str) -> SourceError {
    SourceError::KernelRead {
        call: call.to_owned(),
        reason: REFUSED.to_owned(),
    }
}

/// The reads of a machine that the test states, and what each read gave.
struct FakeMachine {
    /// The name of each read that the run made, in order.
    log: RefCell<Vec<&'static str>>,
    /// Each time that the run slept.
    slept: RefCell<Vec<Duration>>,
    /// The process tables that the reads give, in order.
    tables: RefCell<VecDeque<Result<Vec<ProcessRow>, SourceError>>>,
    /// The CPU ticks that the reads give, in order.
    ticks: RefCell<VecDeque<Result<CpuTicks, SourceError>>>,
    /// The counters that the reads give, in order.
    counters: RefCell<VecDeque<Result<VmCounters, SourceError>>>,
    /// The times that the clock gives, in order.
    clock: RefCell<VecDeque<Duration>>,
    /// The swap file.
    swap: Result<SwapUsage, SourceError>,
    /// The memory pressure.
    pressure: Result<Pressure, SourceError>,
    /// The load average.
    load: Result<f64, SourceError>,
    /// The number of processors.
    processors: Result<u32, SourceError>,
    /// The footprint of each process that has one.
    footprints: HashMap<Pid, u64>,
    /// The Claude Code processes.
    claude: HashSet<Pid>,
    /// The arguments of each process.
    arguments: Result<HashMap<Pid, String>, SourceError>,
    /// The directories that hold a `.git` entry.
    git: HashSet<PathBuf>,
}

impl FakeMachine {
    /// Gives a machine that answers every read, with an empty process table
    /// and CPU ticks that move.
    fn new() -> Self {
        Self {
            log: RefCell::new(Vec::new()),
            slept: RefCell::new(Vec::new()),
            tables: RefCell::new(VecDeque::from([Ok(Vec::new()), Ok(Vec::new())])),
            ticks: RefCell::new(VecDeque::from([
                Ok(CpuTicks {
                    user: 100,
                    system: 50,
                    idle: 850,
                    nice: 0,
                }),
                Ok(CpuTicks {
                    user: 160,
                    system: 70,
                    idle: 970,
                    nice: 0,
                }),
            ])),
            counters: RefCell::new(VecDeque::from([
                Ok(VmCounters {
                    swapins: 1_000,
                    swapouts: 2_000,
                    compressor_pages: 10,
                }),
                Ok(VmCounters {
                    swapins: 1_036,
                    swapouts: 2_005,
                    compressor_pages: 20,
                }),
            ])),
            clock: RefCell::new(VecDeque::from([
                Duration::from_secs(100),
                Duration::from_millis(102_050),
            ])),
            swap: Ok(SwapUsage {
                total_bytes: 5_368_709_120,
                used_bytes: 4_187_593_113,
            }),
            pressure: Ok(Pressure::Warning),
            load: Ok(152.0),
            processors: Ok(14),
            footprints: HashMap::new(),
            claude: HashSet::new(),
            arguments: Ok(HashMap::new()),
            git: HashSet::new(),
        }
    }

    /// Writes the name of a read in the log.
    fn note(&self, name: &'static str) {
        self.log.borrow_mut().push(name);
    }

    /// Takes the next answer of a queue of answers.
    fn next<T>(queue: &RefCell<VecDeque<Result<T, SourceError>>>) -> Result<T, SourceError> {
        queue
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| Err(refused("the queue ran out")))
    }
}

impl Machine for FakeMachine {
    fn process_table(&self) -> Result<Vec<ProcessRow>, SourceError> {
        self.note("ps");
        Self::next(&self.tables)
    }

    fn cpu_ticks(&self) -> Result<CpuTicks, SourceError> {
        self.note("ticks");
        Self::next(&self.ticks)
    }

    fn vm_counters(&self) -> Result<VmCounters, SourceError> {
        self.note("vm");
        Self::next(&self.counters)
    }

    fn swap_usage(&self) -> Result<SwapUsage, SourceError> {
        self.note("swap");
        self.swap.clone()
    }

    fn pressure(&self) -> Result<Pressure, SourceError> {
        self.note("pressure");
        self.pressure.clone()
    }

    fn load(&self) -> Result<f64, SourceError> {
        self.note("load");
        self.load.clone()
    }

    fn processors(&self) -> Result<u32, SourceError> {
        self.note("processors");
        self.processors.clone()
    }

    fn page_size(&self) -> u64 {
        PAGE_SIZE
    }

    fn footprint(&self, pid: Pid) -> Option<u64> {
        self.footprints.get(&pid).copied()
    }

    fn claude_pids(&self) -> HashSet<Pid> {
        self.note("claude");
        self.claude.clone()
    }

    fn arguments(&self) -> Result<HashMap<Pid, String>, SourceError> {
        self.note("arguments");
        self.arguments.clone()
    }

    fn holds_git(&self, directory: &Path) -> bool {
        self.git.contains(directory)
    }

    fn sleep(&self, how_long: Duration) {
        self.note("sleep");
        self.slept.borrow_mut().push(how_long);
    }

    fn clock(&self) -> Duration {
        self.note("clock");
        self.clock.borrow_mut().pop_front().unwrap_or_default()
    }
}

/// A machine that answers every read gives the lines of the summary, no
/// message on the error output, and the exit code 0.
#[test]
fn a_machine_that_answers_gives_the_summary_and_code_0() {
    let expected = render(&summarize(
        &observe(&FakeMachine::new(), interval()).expect("the fake answers every read"),
    ));
    let outcome = report(&FakeMachine::new(), interval());

    assert!(!expected.is_empty(), "the summary has lines");
    assert_eq!(outcome.stdout, expected);
    assert_eq!(outcome.stderr, None);
    assert_eq!(outcome.code, 0);
}

/// A machine whose process table fails gives no line on the standard output,
/// the reason on the error output with the name of the tool, and the exit code
/// 2. A partial summary could look like a quiet Mac.
#[test]
fn a_failed_source_gives_its_reason_and_code_2() {
    let machine = FakeMachine::new();
    *machine.tables.borrow_mut() = VecDeque::from([Err(SourceError::CommandFailed {
        program: "/bin/ps".to_owned(),
        status: "exit status: 1".to_owned(),
        message: "no such luck".to_owned(),
    })]);
    let outcome = report(&machine, interval());

    assert!(outcome.stdout.is_empty(), "stdout {:?}", outcome.stdout);
    assert_eq!(
        outcome.stderr.as_deref(),
        Some("whyslow: /bin/ps ended with exit status: 1: no such luck")
    );
    assert_eq!(outcome.code, EXIT_SOURCE_FAILED);
    assert_eq!(EXIT_SOURCE_FAILED, 2);
}
