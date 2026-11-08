//! The thread that samples this Mac while the terminal view runs.
//!
//! [`Sampler`] reads a [`crate::stream::FaultStream`], joins the other sources
//! to each finished sample through [`crate::machine::join_sources`], and sends
//! [`Update`] values over a bounded channel. The view never waits for a
//! source, because a `ps` can run for seconds and the view must answer a key
//! at once.

use std::collections::BTreeSet;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::thread;
use std::time::{Duration, SystemTime};

use crate::duration::Span;
use crate::history::{Gap, SampledFrame, Update};
use crate::machine::{join_sources, sample_window, Machine, MachineError, SampleFacts};
use crate::pid::Pid;
use crate::stream::{FaultStream, StreamEvent, TimedSample};
use crate::vm::VmCounters;

/// How many updates the channel holds before the sampler waits for the view.
pub const UPDATE_BACKLOG: usize = 64;

/// How long the sampler waits for an event before it looks at the stop flag.
const POLL: Duration = Duration::from_millis(200);

/// The thread that samples this Mac, and the channel of its updates.
pub struct Sampler {
    /// The updates of the sampler thread.
    updates: Receiver<Update>,
}

impl Sampler {
    /// Starts the sampler thread.
    ///
    /// The thread owns `machine`. `interval` is the delay between two
    /// samples. `own_pid` is the PID of `faulte`, which marks the rows of
    /// `faulte` and of its child processes.
    pub fn spawn<M: Machine + Send + 'static>(machine: M, interval: Span, own_pid: Pid) -> Self {
        let (sender, updates) = sync_channel(UPDATE_BACKLOG);
        let run = Run {
            machine,
            interval,
            own_pid,
            sender,
        };
        thread::spawn(move || run.start());
        Self { updates }
    }

    /// Gives the channel of the updates.
    #[must_use]
    pub fn updates(&self) -> &Receiver<Update> {
        &self.updates
    }
}

/// The result of a read of the counters: the counters, or the text of the
/// error.
type CounterRead = Result<VmCounters, String>;

/// What the sampler thread owns.
struct Run<M> {
    /// The machine that the thread reads.
    machine: M,
    /// The delay between two samples.
    interval: Span,
    /// The PID of `faulte`.
    own_pid: Pid,
    /// The channel to the view.
    sender: SyncSender<Update>,
}

/// What the sampler knows about the time that it covered.
#[derive(Debug, Default)]
struct Ledger {
    /// The end of the last time that a frame or a gap covered. `None` before
    /// the first one.
    covered_until: Option<SystemTime>,
}

impl<M: Machine> Run<M> {
    /// Runs the thread until the view drops the channel.
    fn start(self) {
        let mut ledger = Ledger::default();
        if let Ok(stream) = self.machine.fault_stream(self.interval) {
            self.read(stream, &mut ledger);
        }
    }

    /// Reads one stream until the channel closes.
    fn read(&self, mut stream: Box<dyn FaultStream>, ledger: &mut Ledger) {
        let mut reads: [Option<CounterRead>; 2] = [None, None];
        loop {
            let Some(event) = stream.next_event(POLL) else {
                continue;
            };
            if let StreamEvent::Header { finished, .. } = event {
                let before = reads.clone();
                reads = [reads[1].take(), Some(self.read_counters())];
                if let (Some(sample), [Some(older), Some(newer)]) = (finished, before) {
                    if !self.send_sample(&sample, older, newer, ledger) {
                        return;
                    }
                }
            }
        }
    }

    /// Reads the counters of the virtual memory system.
    fn read_counters(&self) -> CounterRead {
        self.machine
            .vm_counters()
            .map_err(|error: MachineError| error.to_string())
    }

    /// Makes the frame of `sample`, or a gap with the reason that it failed.
    /// Gives `false` when the view dropped the channel.
    fn send_sample(
        &self,
        sample: &TimedSample,
        older: CounterRead,
        newer: CounterRead,
        ledger: &mut Ledger,
    ) -> bool {
        let outcome = older.and_then(|before| {
            newer.and_then(|after| {
                let facts = SampleFacts {
                    interval: self.interval,
                    window: sample_window(self.interval, sample.sample.elapsed, sample.wall),
                    faults: &sample.sample.rows,
                    before,
                    after,
                    swap_window: sample.wall,
                };
                join_sources(&self.machine, &facts).map_err(|error| error.to_string())
            })
        });
        match outcome {
            Ok(observed) => {
                ledger.covered_until = Some(sample.ended_at);
                self.sender
                    .send(Update::Frame(Box::new(SampledFrame {
                        observed,
                        at: sample.ended_at,
                        own: BTreeSet::new(),
                    })))
                    .is_ok()
            }
            Err(reason) => self.send_gap(ledger, sample.ended_at, reason),
        }
    }

    /// Sends a gap that ends at `at`. Gives `false` when the view dropped the
    /// channel.
    fn send_gap(&self, ledger: &mut Ledger, at: SystemTime, reason: String) -> bool {
        let lost = ledger.covered_until.map_or(Duration::ZERO, |covered| {
            at.duration_since(covered).unwrap_or(Duration::ZERO)
        });
        ledger.covered_until = Some(at);
        self.sender
            .send(Update::Gap(Gap { at, lost, reason }))
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::RecvTimeoutError;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::thread;
    use std::time::{Duration, SystemTime};

    use occ::SessionRecord;

    use crate::history::{Gap, SampledFrame};
    use crate::machine::{sample_window, MachineError, Signal};
    use crate::pid::Uid;
    use crate::ranking::{ClaudeRole, Viewer};
    use crate::stream::{FaultStream, Stopper, StreamEvent, TimedSample};
    use crate::table::ProcessRow;
    use crate::top::{FaultCount, TopSample};
    use crate::vm::{SwapUsage, VmCounters, VmDelta};

    /// The longest wait of any test. The machine of the tests can be under a
    /// heavy load, and a wait ends early when its condition holds.
    const BOUND: Duration = Duration::from_secs(60);

    /// The pause between two looks at a condition.
    const LOOK: Duration = Duration::from_millis(1);

    /// The PID of `faulte` in the tests.
    const OWN: u32 = 100;

    /// The account of the viewer in the tests.
    const VIEWER_UID: u32 = 501;

    /// The interval of the tests, in seconds.
    const INTERVAL_SECS: u64 = 2;

    /// The time now on the fake machine, in seconds since the Unix epoch.
    const NOW: u64 = 1_780_000_000;

    /// Gives the interval of the tests.
    fn interval() -> Span {
        format!("{INTERVAL_SECS}s").parse().expect("a span")
    }

    /// Gives the time `secs` seconds after the Unix epoch.
    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// Takes a lock, also after a test thread panicked.
    fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits until `condition` holds, and fails after [`BOUND`].
    fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let began = std::time::Instant::now();
        while !condition() {
            assert!(began.elapsed() < BOUND, "waited too long for {what}");
            thread::sleep(LOOK);
        }
    }

    /// Gives the counters with the swap `swapins` and `swapouts`.
    fn counters(swapins: u64, swapouts: u64) -> VmCounters {
        VmCounters {
            swapins,
            swapouts,
            compressor_pages: 0,
        }
    }

    /// Gives a row of the process table.
    fn process(pid: u32, ppid: u32) -> ProcessRow {
        ProcessRow {
            pid: Pid::new(pid),
            ppid: Pid::new(ppid),
            uid: Uid::new(VIEWER_UID),
            rss_kib: 1_024,
            zombie: false,
            started_at_epoch_secs: NOW - 100,
            command: format!("process {pid}"),
        }
    }

    /// Gives a header event that finished no sample.
    fn header(secs: u64) -> StreamEvent {
        StreamEvent::Header {
            at: at(secs),
            finished: None,
        }
    }

    /// Gives a header event that finished a sample. The sample ended at
    /// `ended` and held `faults` faults of the process 200.
    fn finished(secs: u64, ended: u64, faults: u64) -> StreamEvent {
        StreamEvent::Header {
            at: at(secs),
            finished: Some(TimedSample {
                sample: TopSample {
                    rows: vec![FaultCount {
                        pid: Pid::new(200),
                        faults,
                    }],
                    elapsed: Some(Duration::from_secs(3)),
                },
                wall: Duration::from_secs(4),
                ended_at: at(ended),
            }),
        }
    }

    /// What the fake machine and its tests share.
    #[derive(Default)]
    struct Shared {
        /// The answer to each read of the counters. The last one repeats.
        counters: Mutex<VecDeque<Result<VmCounters, MachineError>>>,
        /// The events of each stream that the sampler starts, or the error of
        /// the start. A stream past the last one gives no event.
        streams: Mutex<VecDeque<Result<Vec<StreamEvent>, MachineError>>>,
        /// The errors that the next reads of the process table give.
        table_errors: Mutex<VecDeque<MachineError>>,
        /// The process table.
        table: Mutex<Vec<ProcessRow>>,
        /// The argument of each call of `sleep`.
        sleeps: Mutex<Vec<Duration>>,
        /// The count of the streams that the sampler started.
        starts: AtomicUsize,
        /// The count of the calls of the stopper of a stream.
        stops: AtomicUsize,
        /// Tells whether the machine was dropped.
        dropped: AtomicBool,
    }

    /// The machine that the sampler owns. It drops with the sampler thread.
    struct FakeMachine {
        /// The state that the test also reads.
        shared: Arc<Shared>,
    }

    impl Drop for FakeMachine {
        fn drop(&mut self) {
            self.shared.dropped.store(true, Ordering::SeqCst);
        }
    }

    /// A stream that gives stated events, then nothing.
    struct FakeStream {
        /// The events that are left.
        events: VecDeque<StreamEvent>,
        /// The state that the test also reads.
        shared: Arc<Shared>,
    }

    impl FaultStream for FakeStream {
        fn next_event(&mut self, _wait: Duration) -> Option<StreamEvent> {
            if let Some(event) = self.events.pop_front() {
                return Some(event);
            }
            thread::sleep(LOOK);
            None
        }

        fn stopper(&self) -> Stopper {
            let shared = Arc::clone(&self.shared);
            Stopper::new(move || {
                shared.stops.fetch_add(1, Ordering::SeqCst);
            })
        }
    }

    impl Machine for FakeMachine {
        fn sample_faults(&self, _interval: Span) -> Result<(TopSample, Duration), MachineError> {
            unreachable!("the sampler reads a stream");
        }

        fn fault_stream(&self, _interval: Span) -> Result<Box<dyn FaultStream>, MachineError> {
            self.shared.starts.fetch_add(1, Ordering::SeqCst);
            let next = locked(&self.shared.streams).pop_front();
            let events = next.unwrap_or_else(|| Ok(Vec::new()))?;
            Ok(Box::new(FakeStream {
                events: events.into(),
                shared: Arc::clone(&self.shared),
            }))
        }

        fn process_table(&self) -> Result<Vec<ProcessRow>, MachineError> {
            if let Some(error) = locked(&self.shared.table_errors).pop_front() {
                return Err(error);
            }
            Ok(locked(&self.shared.table).clone())
        }

        fn claude_roles(&self) -> HashMap<Pid, ClaudeRole> {
            HashMap::new()
        }

        fn record_for(&self, _: Pid, _: Uid, _: u64) -> Option<SessionRecord> {
            None
        }

        fn vm_counters(&self) -> Result<VmCounters, MachineError> {
            let mut queue = locked(&self.shared.counters);
            if queue.len() > 1 {
                queue.pop_front()
            } else {
                queue.front().cloned()
            }
            .unwrap_or_else(|| Ok(VmCounters::default()))
        }

        fn swap_usage(&self) -> Result<SwapUsage, MachineError> {
            Ok(SwapUsage::default())
        }

        fn page_size(&self) -> u64 {
            16_384
        }

        fn viewer(&self) -> Viewer {
            Viewer {
                uid: Uid::new(VIEWER_UID),
                is_root: false,
            }
        }

        fn account_name(&self, _uid: Uid) -> Option<String> {
            None
        }

        fn now(&self) -> SystemTime {
            at(NOW)
        }

        fn signal(&self, _pid: Pid, _signal: Signal) -> Result<(), MachineError> {
            unreachable!("the sampler sends no signal");
        }

        fn sleep(&self, how_long: Duration) {
            locked(&self.shared.sleeps).push(how_long);
        }
    }

    /// A test rig: the shared state and the sampler that it feeds.
    struct Rig {
        /// The state that the fake machine shares.
        shared: Arc<Shared>,
    }

    impl Rig {
        /// Makes a rig whose table holds `faulte`, a child of it, and a
        /// process of another parent.
        fn new() -> Self {
            let shared = Arc::new(Shared::default());
            *locked(&shared.table) = vec![
                process(OWN, 1),
                process(101, OWN),
                process(200, 1),
                process(300, 101),
            ];
            Self { shared }
        }

        /// Adds a stream that gives `events`.
        fn stream(self, events: Vec<StreamEvent>) -> Self {
            locked(&self.shared.streams).push_back(Ok(events));
            self
        }

        /// Adds a start that fails with `error`.
        fn failing_start(self, error: MachineError) -> Self {
            locked(&self.shared.streams).push_back(Err(error));
            self
        }

        /// States the answers of the reads of the counters.
        fn counters(self, reads: Vec<Result<VmCounters, MachineError>>) -> Self {
            *locked(&self.shared.counters) = reads.into();
            self
        }

        /// Adds an error for the next read of the process table.
        fn failing_table(self, error: MachineError) -> Self {
            locked(&self.shared.table_errors).push_back(error);
            self
        }

        /// Starts the sampler on a machine that shares this rig's state.
        fn start(&self) -> Sampler {
            Sampler::spawn(
                FakeMachine {
                    shared: Arc::clone(&self.shared),
                },
                interval(),
                Pid::new(OWN),
            )
        }
    }

    /// Gives the next update, and fails after [`BOUND`].
    fn next(sampler: &Sampler) -> Update {
        match sampler.updates().recv_timeout(BOUND) {
            Ok(update) => update,
            Err(RecvTimeoutError::Timeout) => panic!("no update arrived"),
            Err(RecvTimeoutError::Disconnected) => panic!("the sampler ended with no update"),
        }
    }

    /// Gives the next update as a frame.
    fn next_frame(sampler: &Sampler) -> SampledFrame {
        match next(sampler) {
            Update::Frame(frame) => *frame,
            Update::Gap(gap) => panic!("a gap arrived, and a frame was due: {gap:?}"),
        }
    }

    /// Gives the next update as a gap.
    fn next_gap(sampler: &Sampler) -> Gap {
        match next(sampler) {
            Update::Gap(gap) => gap,
            Update::Frame(_) => panic!("a frame arrived, and a gap was due"),
        }
    }

    /// A finished sample makes a frame. The window is the one that
    /// `sample_window` gives, the time is the end of the sample, and the rows
    /// are the join of the sample with the table of the machine.
    #[test]
    fn a_finished_sample_makes_a_frame() {
        let rig = Rig::new().stream(vec![header(10), header(12), finished(14, 12, 7_000)]);
        let sampler = rig.start();

        let frame = next_frame(&sampler);

        assert_eq!(
            frame.observed.ranking.window,
            sample_window(
                interval(),
                Some(Duration::from_secs(3)),
                Duration::from_secs(4)
            )
        );
        assert_eq!(frame.at, at(12));
        assert_eq!(frame.observed.interval, interval());
        let row = frame
            .observed
            .ranking
            .rows
            .iter()
            .find(|row| row.pid == Pid::new(200))
            .expect("the process 200 has a row");
        assert_eq!(row.faults, 7_000);
    }

    /// The swap traffic of a frame is the change of the counters over the
    /// window of the sample itself: from the read at the header that began
    /// the window to the read at the header that ended it. The read at the
    /// header that finished the sample belongs to the next window. The first
    /// frame follows the same rule.
    #[test]
    fn the_swap_traffic_is_the_change_over_the_window_of_the_sample() {
        let rig = Rig::new()
            .counters(vec![
                Ok(counters(100, 200)),
                Ok(counters(110, 230)),
                Ok(counters(999, 999)),
                Ok(counters(999, 999)),
            ])
            .stream(vec![
                header(10),
                header(12),
                finished(14, 12, 7_000),
                finished(16, 14, 8_000),
            ]);
        let sampler = rig.start();

        let first = next_frame(&sampler);
        let second = next_frame(&sampler);

        assert_eq!(
            first.observed.swap,
            VmDelta {
                swapins: 10,
                swapouts: 30
            }
        );
        assert_eq!(
            second.observed.swap,
            VmDelta {
                swapins: 889,
                swapouts: 769
            }
        );
        assert_eq!(first.observed.swap_window, Duration::from_secs(4));
    }

    /// Gives the error of a `ps` that failed.
    fn ps_failure() -> MachineError {
        MachineError::CommandFailed {
            program: "/bin/ps".to_owned(),
            status: "exit status: 1".to_owned(),
            message: "ps is broken".to_owned(),
        }
    }

    /// A `ps` that fails costs one sample. The view gets a gap with the text
    /// of the error, and the next sample still makes a frame.
    #[test]
    fn a_failed_ps_makes_a_gap_and_the_next_sample_makes_a_frame() {
        let rig = Rig::new().failing_table(ps_failure()).stream(vec![
            header(10),
            header(12),
            finished(14, 12, 7_000),
            finished(16, 14, 8_000),
        ]);
        let sampler = rig.start();

        let gap = next_gap(&sampler);
        let frame = next_frame(&sampler);

        assert_eq!(gap.reason, ps_failure().to_string());
        assert_eq!(gap.at, at(12));
        assert_eq!(gap.lost, Duration::ZERO);
        assert_eq!(frame.at, at(14));
    }
}
