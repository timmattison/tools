//! Tests of the read of the whole machine, over a machine that the test makes.
//!
//! The fault sample of every test is the real capture in
//! `tests/fixtures/top-l2-cd.txt`, read through the parser of the tool. Every
//! other fact is a plain value that the test states. No test here runs `top`,
//! runs `ps`, reads the registry, or reads a clock.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use faulte::duration::Span;
use faulte::machine::{observe, Machine, MachineError};
use faulte::pid::{Pid, Uid};
use faulte::ranking::{ClaudeRole, Viewer};
use faulte::table::ProcessRow;
use faulte::top::{self, TopSample};
use faulte::vm::{SwapUsage, VmCounters};
use occ::{SessionId, SessionRecord, SessionStatus};

/// The real capture of `top -l 2 -s 2 -c d -n <kern.maxproc> -stats pid,faults`.
const CAPTURE: &str = include_str!("fixtures/top-l2-cd.txt");

/// The account that runs the tests.
const VIEWER: Uid = Uid::new(501);

/// The other account of this Mac. The registry folder of that account has the
/// mode `0700`, so the viewer reads nothing of it.
const OTHER: Uid = Uid::new(502);

/// The name of the account that runs the tests.
const VIEWER_NAME: &str = "tim";

/// The time now in the tests, in seconds since the Unix epoch.
const NOW: u64 = 1_790_000_000;

/// The time when each process of the tests started, in seconds since the Unix
/// epoch.
const STARTED: u64 = 1_780_000_000;

/// The size of one page of memory of an Apple Silicon Mac, in bytes.
const PAGE_SIZE: u64 = 16_384;

/// Gives the interval that the person asked for in the tests.
fn interval() -> Span {
    "2s".parse().expect("2s is a span")
}

/// Gives the time now in the tests.
fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(NOW)
}

/// Gives the fault sample of the real capture.
fn sample() -> TopSample {
    top::parse(CAPTURE).expect("the capture is a sample of faults")
}

/// Gives one row of the process table.
fn row(pid: u32, uid: Uid, command: &str) -> ProcessRow {
    ProcessRow {
        pid: Pid::new(pid),
        ppid: Pid::new(1),
        uid,
        rss_kib: 100_000,
        zombie: false,
        started_at_epoch_secs: STARTED,
        command: command.to_owned(),
    }
}

/// Gives the registry record of an idle session.
fn record(id: &str) -> SessionRecord {
    SessionRecord {
        session: SessionId::parse(id).expect("the text is a session ID"),
        status: Some(SessionStatus::Idle),
        status_changed_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(NOW - 600)),
        directory: Some(PathBuf::from("/Volumes/SamsungSSDs/code/tools")),
    }
}

/// The reads of a machine that the test states, and what each read gave.
struct FakeMachine {
    /// The counters that the reads of the counters give, in order.
    counters: RefCell<VecDeque<Result<VmCounters, MachineError>>>,
    /// The sample of faults, and the time that the run of the sampler took.
    sample: Result<(TopSample, Duration), MachineError>,
    /// The process table.
    table: Result<Vec<ProcessRow>, MachineError>,
    /// The swap file of the Mac.
    usage: Result<SwapUsage, MachineError>,
    /// The Claude Code role of each process that has one.
    claude: HashMap<Pid, ClaudeRole>,
    /// The registry record under each PID.
    records: HashMap<Pid, SessionRecord>,
    /// Each PID that the run asked the registry about, in order.
    asked: RefCell<Vec<Pid>>,
    /// The account that runs `faulte`.
    viewer: Viewer,
    /// The name of each account that has one.
    names: HashMap<Uid, String>,
}

impl FakeMachine {
    /// Gives a machine that answers every read, with no Claude Code process in
    /// it.
    fn new() -> Self {
        Self {
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
            sample: Ok((sample(), Duration::from_secs(5))),
            table: Ok(Vec::new()),
            usage: Ok(SwapUsage {
                total_bytes: 12_884_901_888,
                used_bytes: 11_811_160_064,
            }),
            claude: HashMap::new(),
            records: HashMap::new(),
            asked: RefCell::new(Vec::new()),
            viewer: Viewer {
                uid: VIEWER,
                is_root: false,
            },
            names: HashMap::from([(VIEWER, VIEWER_NAME.to_owned())]),
        }
    }

    /// Gives this machine, with `table` as its process table.
    fn with_table(mut self, table: Vec<ProcessRow>) -> Self {
        self.table = Ok(table);
        self
    }

    /// Gives this machine, with `pid` as a Claude Code process of `role`.
    fn with_claude(mut self, pid: u32, role: ClaudeRole) -> Self {
        self.claude.insert(Pid::new(pid), role);
        self
    }

    /// Gives this machine, with `record` under `pid` in the registry.
    fn with_record(mut self, pid: u32, record: SessionRecord) -> Self {
        self.records.insert(Pid::new(pid), record);
        self
    }

    /// Gives this machine, with root as the account that runs `faulte`.
    fn as_root(mut self) -> Self {
        self.viewer = Viewer {
            uid: Uid::new(0),
            is_root: true,
        };
        self
    }

    /// Gives each PID that the run asked the registry about.
    fn asked(&self) -> Vec<Pid> {
        self.asked.borrow().clone()
    }
}

impl Machine for FakeMachine {
    fn sample_faults(&self, _interval: Span) -> Result<(TopSample, Duration), MachineError> {
        self.sample.clone()
    }

    fn process_table(&self) -> Result<Vec<ProcessRow>, MachineError> {
        self.table.clone()
    }

    fn claude_roles(&self) -> HashMap<Pid, ClaudeRole> {
        self.claude.clone()
    }

    fn record_for(
        &self,
        pid: Pid,
        _owner: Uid,
        _started_at_epoch_secs: u64,
    ) -> Option<SessionRecord> {
        self.asked.borrow_mut().push(pid);
        self.records.get(&pid).cloned()
    }

    fn vm_counters(&self) -> Result<VmCounters, MachineError> {
        self.counters
            .borrow_mut()
            .pop_front()
            .expect("the run reads the counters twice")
    }

    fn swap_usage(&self) -> Result<SwapUsage, MachineError> {
        self.usage.clone()
    }

    fn page_size(&self) -> u64 {
        PAGE_SIZE
    }

    fn viewer(&self) -> Viewer {
        self.viewer
    }

    fn account_name(&self, uid: Uid) -> Option<String> {
        self.names.get(&uid).cloned()
    }

    fn now(&self) -> SystemTime {
        now()
    }
}

/// Gives the error of a read of the kernel that failed.
fn kernel_error(call: &str) -> MachineError {
    MachineError::KernelRead {
        call: call.to_owned(),
        reason: "Operation not permitted".to_owned(),
    }
}

/// Gives the error of a command that ended with a status that is not success.
fn command_error(program: &str) -> MachineError {
    MachineError::CommandFailed {
        program: program.to_owned(),
        status: "exit status: 1".to_owned(),
        message: "no such option".to_owned(),
    }
}

/// A source that fails stops the run, and the error of that source is the
/// error of the run. `faulte` then prints the reason and exits 2. A ranking
/// that is empty because a source failed looks the same as a Mac that does
/// nothing, so the run never gives one.
#[test]
fn the_error_of_a_source_that_failed_is_the_error_of_the_run() {
    let counters = kernel_error("host_statistics64(HOST_VM_INFO64)");
    let sampler = command_error(top::PROGRAM);
    let swap = kernel_error("sysctlbyname(vm.swapusage)");
    let table = command_error(faulte::table::PROGRAM);

    let mut failing_counters = FakeMachine::new();
    failing_counters.counters = RefCell::new(VecDeque::from([Err(counters.clone())]));
    let mut failing_sampler = FakeMachine::new();
    failing_sampler.sample = Err(sampler.clone());
    let mut failing_swap = FakeMachine::new();
    failing_swap.usage = Err(swap.clone());
    let mut failing_table = FakeMachine::new();
    failing_table.table = Err(table.clone());

    let cases: [(&str, FakeMachine, MachineError); 4] = [
        ("the counters", failing_counters, counters),
        ("the sampler", failing_sampler, sampler),
        ("the swap file", failing_swap, swap),
        ("the process table", failing_table, table),
    ];
    for (source, machine, expected) in cases {
        let error = observe(&machine, interval())
            .expect_err(&format!("a run that cannot read {source} gives an error"));

        assert_eq!(error, expected, "the error of {source} comes back whole");
    }
}
