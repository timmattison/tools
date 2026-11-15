//! Tests of the read of the whole machine, over a machine that the test makes.
//!
//! Every fact is a plain value that the test states. No test here runs `ps`,
//! calls the kernel, reads a process, or reads a clock. The one test that
//! reads the real Mac is `tests/this-mac.rs`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use faulte::duration::Span;
use faulte::pid::Pid;
use faulte::vm::{SwapUsage, VmCounters};
use whyslow::machine::{observe, Machine, SourceError};
use whyslow::sample::ProcessRow;
use whyslow::verdict::{CpuTicks, Facts, Pressure};

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

/// The run reads the sources in one fixed order, and it sleeps the interval
/// between the two snapshots. The order is the design: the counters of the
/// virtual memory system bracket the window, and the clock sits beside each
/// process table, so the window is the time between the two tables.
#[test]
fn the_run_reads_the_sources_in_the_fixed_order_and_sleeps_the_interval() {
    let machine = FakeMachine::new();
    let _ = observe(&machine, interval());
    assert_eq!(
        *machine.log.borrow(),
        [
            "vm",
            "ps",
            "clock",
            "ticks",
            "sleep",
            "ps",
            "clock",
            "ticks",
            "vm",
            "pressure",
            "swap",
            "load",
            "processors",
            "claude",
            "arguments",
        ]
    );
    assert_eq!(*machine.slept.borrow(), [Duration::from_secs(2)]);
}

/// Runs the run over a machine that `change` spoiled, and gives its answer.
fn run_with(change: impl FnOnce(&mut FakeMachine)) -> Result<Facts, SourceError> {
    let mut machine = FakeMachine::new();
    change(&mut machine);
    observe(&machine, interval())
}

/// The first process table fails: the run stops with its error.
#[test]
fn a_failed_first_process_table_stops_the_run() {
    let result = run_with(|m| {
        m.tables
            .get_mut()
            .front_mut()
            .unwrap()
            .clone_from(&Err(refused("ps1")))
    });
    assert_eq!(result, Err(refused("ps1")));
}

/// The second process table fails: the run stops with its error.
#[test]
fn a_failed_second_process_table_stops_the_run() {
    let result = run_with(|m| {
        m.tables
            .get_mut()
            .back_mut()
            .unwrap()
            .clone_from(&Err(refused("ps2")))
    });
    assert_eq!(result, Err(refused("ps2")));
}

/// The CPU ticks fail: the run stops with their error.
#[test]
fn failed_cpu_ticks_stop_the_run() {
    let result = run_with(|m| *m.ticks.get_mut() = VecDeque::from([Err(refused("ticks"))]));
    assert_eq!(result, Err(refused("ticks")));
}

/// The counters of the virtual memory system fail: the run stops.
#[test]
fn failed_vm_counters_stop_the_run() {
    let result = run_with(|m| *m.counters.get_mut() = VecDeque::from([Err(refused("vm"))]));
    assert_eq!(result, Err(refused("vm")));
}

/// The swap file fails: the run stops with its error.
#[test]
fn a_failed_swap_read_stops_the_run() {
    let result = run_with(|m| m.swap = Err(refused("swap")));
    assert_eq!(result, Err(refused("swap")));
}

/// The pressure fails: the run stops with its error.
#[test]
fn a_failed_pressure_read_stops_the_run() {
    let result = run_with(|m| m.pressure = Err(SourceError::UnknownPressure { level: 3 }));
    assert_eq!(result, Err(SourceError::UnknownPressure { level: 3 }));
}

/// The load average fails: the run stops with its error.
#[test]
fn a_failed_load_read_stops_the_run() {
    let result = run_with(|m| m.load = Err(refused("load")));
    assert_eq!(result, Err(refused("load")));
}

/// The number of processors fails: the run stops with its error.
#[test]
fn a_failed_processors_read_stops_the_run() {
    let result = run_with(|m| m.processors = Err(refused("processors")));
    assert_eq!(result, Err(refused("processors")));
}

/// The arguments are missing because `ps` did not run: the run stops. A
/// compiler with no known arguments would show as `no worktree`, which is a
/// false answer.
#[test]
fn missing_arguments_stop_the_run() {
    let result = run_with(|m| m.arguments = Err(SourceError::NoArguments));
    assert_eq!(result, Err(SourceError::NoArguments));
}

/// The facts hold the change of each counter, and the values that the machine
/// gave as they are. The window is the time between the two readings of the
/// clock, and the compressor is the pages after the window times the page
/// size.
#[test]
fn the_facts_hold_the_changes_and_the_values_of_the_machine() {
    let facts = run_with(|_| {}).expect("every source answers");
    assert_eq!(facts.window, Duration::from_millis(2_050));
    assert_eq!(
        facts.ticks,
        CpuTicks {
            user: 60,
            system: 20,
            idle: 120,
            nice: 0
        }
    );
    assert_eq!(facts.swapouts, 5);
    assert_eq!(facts.compressor_bytes, 20 * PAGE_SIZE);
    assert_eq!(facts.pressure, Pressure::Warning);
    assert_eq!(
        facts.swap,
        SwapUsage {
            total_bytes: 5_368_709_120,
            used_bytes: 4_187_593_113,
        }
    );
    assert!((facts.load - 152.0).abs() < f64::EPSILON);
    assert_eq!(facts.processors, 14);
}

/// A clock that went back gives a window of zero and no panic.
#[test]
fn a_clock_that_went_back_gives_a_window_of_zero() {
    let facts = run_with(|m| {
        *m.clock.get_mut() = VecDeque::from([Duration::from_secs(9), Duration::from_secs(3)]);
    })
    .expect("every source answers");
    assert_eq!(facts.window, Duration::ZERO);
}

/// CPU ticks that are the same twice give no window to measure in. The run
/// refuses them, because a percent of nothing has no meaning.
#[test]
fn cpu_ticks_that_did_not_move_stop_the_run() {
    let still = CpuTicks {
        user: 5,
        system: 5,
        idle: 5,
        nice: 5,
    };
    let result = run_with(|m| *m.ticks.get_mut() = VecDeque::from([Ok(still), Ok(still)]));
    assert_eq!(result, Err(SourceError::CpuTicksDidNotMove));
}
