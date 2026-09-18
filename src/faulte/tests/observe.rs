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
use faulte::ranking::{ClaudeRole, ClaudeView, Viewer};
use faulte::state::SessionState;
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

/// The name of the other account of this Mac.
const OTHER_NAME: &str = "ada";

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
            names: HashMap::from([
                (VIEWER, VIEWER_NAME.to_owned()),
                (OTHER, OTHER_NAME.to_owned()),
            ]),
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
    fn running_as_root(mut self) -> Self {
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

/// The PID of the Claude Code session of the viewer in the tests, and its
/// faults in the second sample of the capture.
const SESSION_PID: u32 = 35455;

/// The faults of [`SESSION_PID`] in the second sample of the capture.
const SESSION_FAULTS: u64 = 18_510;

/// The PID of the Claude Code process of the other account in the tests.
const OTHER_PID: u32 = 46733;

/// The faults of [`OTHER_PID`] in the second sample of the capture.
const OTHER_FAULTS: u64 = 16_910;

/// The PID of a process of the viewer that is not Claude Code.
const PLAIN_PID: u32 = 659;

/// The faults of [`PLAIN_PID`] in the second sample of the capture.
const PLAIN_FAULTS: u64 = 2_799;

/// The PID of the kernel, which `top` lists and `ps` does not.
const KERNEL_PID: u32 = 0;

/// The faults of the kernel in the second sample of the capture.
const KERNEL_FAULTS: u64 = 1_329;

/// The PID of a process of the viewer that made one fault.
const QUIET_PID: u32 = 38493;

/// The faults of [`QUIET_PID`] in the second sample of the capture.
const QUIET_FAULTS: u64 = 1;

/// The PID of a zombie. `top` does not list a zombie, so this PID is in the
/// table and not in the capture.
const ZOMBIE_PID: u32 = 99999;

/// The PID of a live process that the capture does not list.
const UNSAMPLED_PID: u32 = 99998;

/// The number of rows in the second sample of the capture.
const SAMPLED_PROCESSES: usize = 1549;

/// The session ID that the registry record of the tests names.
const SESSION_ID: &str = "34ffff5a-3324-4038-89bb-d5cc5972cfd0";

/// The time since the session of the tests became idle, in seconds.
const IDLE_SECONDS: u64 = 600;

/// Gives a machine whose table holds one process of each kind that the ranking
/// must tell apart.
fn machine_of_the_capture() -> FakeMachine {
    let mut zombie = row(ZOMBIE_PID, VIEWER, "(claude)");
    zombie.zombie = true;
    FakeMachine::new()
        .with_table(vec![
            row(SESSION_PID, VIEWER, "claude"),
            row(OTHER_PID, OTHER, "claude"),
            row(PLAIN_PID, VIEWER, "/usr/sbin/cfprefsd agent"),
            row(QUIET_PID, VIEWER, "/bin/sleep 900"),
            row(UNSAMPLED_PID, VIEWER, "/usr/bin/true"),
            zombie,
        ])
        .with_claude(SESSION_PID, ClaudeRole::Session)
        .with_claude(OTHER_PID, ClaudeRole::Unreadable)
        .with_record(SESSION_PID, record(SESSION_ID))
}

/// The run joins the sample, the table, the roles and the records into one
/// ranking. The rows are the processes that both sources name, and the kernel,
/// which `top` names and `ps` does not. Every other process of either source
/// is a count of the skips, because a ranking that drops a process looks the
/// same as a correct one.
#[test]
fn the_run_ranks_the_sample_against_the_table() {
    let machine = machine_of_the_capture();

    let observed = observe(&machine, interval()).expect("every source answers");

    let ranking = &observed.ranking;
    let order: Vec<(u32, u64)> = ranking
        .rows
        .iter()
        .map(|row| (row.pid.get(), row.faults))
        .collect();
    assert_eq!(
        order,
        vec![
            (SESSION_PID, SESSION_FAULTS),
            (OTHER_PID, OTHER_FAULTS),
            (PLAIN_PID, PLAIN_FAULTS),
            (KERNEL_PID, KERNEL_FAULTS),
            (QUIET_PID, QUIET_FAULTS),
        ],
        "the rows are the processes of both sources, and the kernel, by faults"
    );
    assert_eq!(
        ranking.rows[3].command, "kernel_task",
        "the kernel is in the sample and not in the table, and keeps its name"
    );
    assert_eq!(
        ranking.window,
        Duration::from_secs(4),
        "the two clock lines of the capture are 4 seconds apart"
    );
    let total: u64 = sample().rows.iter().map(|count| count.faults).sum();
    assert_eq!(
        ranking.total_faults, total,
        "the total holds every process of the sample, and not the rows alone"
    );
    assert_eq!(
        ranking.skipped.exited,
        SAMPLED_PROCESSES - 1 - 4,
        "each sampled process that the table lacks exited, and the kernel is not one"
    );
    assert_eq!(ranking.skipped.zombies, 1, "the table holds one zombie");
    assert_eq!(
        ranking.skipped.unsampled, 1,
        "the table holds one live process that the sample lacks"
    );
}

/// A row of a Claude Code session states the session, the state and the
/// directory of its registry record. A row of another account states none of
/// them, because the registry folder of that account has the mode `0700`.
#[test]
fn a_claude_row_states_what_the_account_of_the_viewer_can_read() {
    let machine = machine_of_the_capture();

    let observed = observe(&machine, interval()).expect("every source answers");

    let ranking = &observed.ranking;
    assert_eq!(
        ranking.rows[0].claude,
        ClaudeView::Session {
            id: SessionId::parse(SESSION_ID).expect("the text is a session ID"),
            state: SessionState::Idle {
                for_: Some(Duration::from_secs(IDLE_SECONDS)),
            },
            directory: Some(PathBuf::from("/Volumes/SamsungSSDs/code/tools")),
        },
        "the session of the viewer states its record"
    );
    assert_eq!(
        ranking.rows[1].claude,
        ClaudeView::OtherAccount,
        "the session of the other account states no record"
    );
    assert_eq!(
        ranking.claude.processes, 2,
        "both Claude Code processes count in the total"
    );
    assert_eq!(
        ranking.claude.other_account, 1,
        "one of them belongs to another account"
    );
    assert_eq!(
        ranking.claude.faults,
        SESSION_FAULTS + OTHER_FAULTS,
        "the total holds the faults of both of them"
    );
}

/// The counters of the kernel come before and after the sample, so the swap
/// traffic of the run is the difference of the two reads. The compressor holds
/// what it holds now, so the count after the sample is the one that says what
/// this Mac is doing.
#[test]
fn the_run_states_the_swap_traffic_over_the_sample() {
    let machine = machine_of_the_capture();

    let observed = observe(&machine, interval()).expect("every source answers");

    assert_eq!(observed.swap.swapins, 36, "1,036 swap-ins less 1,000");
    assert_eq!(observed.swap.swapouts, 5, "2,005 swap-outs less 2,000");
    assert_eq!(
        observed.compressor_bytes,
        20 * PAGE_SIZE,
        "20 pages of the compressor after the sample"
    );
    assert_eq!(
        observed.usage.used_bytes, 11_811_160_064,
        "the swap file states how much of it is in use"
    );
}

/// The output states the name of each account that owns a row, because a
/// number says nothing about who runs a session. An account with no name
/// keeps its number, which is the whole answer that the machine gave.
#[test]
fn the_run_names_the_account_of_each_row() {
    let machine = machine_of_the_capture();

    let observed = observe(&machine, interval()).expect("every source answers");

    let accounts = &observed.accounts;
    assert_eq!(
        accounts.name_of(VIEWER),
        VIEWER_NAME,
        "the account of the viewer has a name"
    );
    assert_eq!(
        accounts.name_of(OTHER),
        OTHER_NAME,
        "the other account of this Mac has a name"
    );
    assert_eq!(
        accounts.name_of(Uid::new(0)),
        "0",
        "the kernel owns a row, and the machine gave no name for its account"
    );
}

/// The registry folder of another account has the mode `0700`. A read of it
/// fails whatever the folder holds, so a missing record there means nothing.
/// The run therefore never asks, and the row states `other account` instead of
/// a state that no read could give.
#[test]
fn the_run_never_asks_the_registry_about_another_account() {
    let machine = machine_of_the_capture();

    observe(&machine, interval()).expect("every source answers");

    assert_eq!(
        machine.asked(),
        vec![Pid::new(SESSION_PID)],
        "the run asks about the session of the viewer, and about no other PID"
    );
}

/// Root reads the home directory of every account, so it asks about every
/// Claude Code process. `faulte` never runs `sudo` itself, the same as `crap`.
#[test]
fn root_asks_the_registry_about_every_account() {
    let machine = machine_of_the_capture().running_as_root();

    observe(&machine, interval()).expect("every source answers");

    let mut asked = machine.asked();
    asked.sort();
    assert_eq!(
        asked,
        vec![Pid::new(SESSION_PID), Pid::new(OTHER_PID)],
        "root asks about the session of each account"
    );
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
