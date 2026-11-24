//! The thread that samples this Mac while the terminal view runs.
//!
//! [`Sampler`] reads a [`crate::stream::FaultStream`], joins the other sources
//! to each finished sample through [`crate::machine::join_sources`], and sends
//! [`Update`] values over a bounded channel. The view never waits for a
//! source, because a `ps` can run for seconds and the view must answer a key
//! at once.
//!
//! The sampler reads the other sources when the header of a sample arrives,
//! and joins them one header later, when the sample is complete. A process can
//! exit between those two moments, and a table that came later has no row for
//! it. On 2026-11-08, a table of one sample later left 42% to 51% of the
//! faults of this Mac with no row.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::duration::Span;
use crate::history::{Gap, SampledFrame, Update};
use crate::machine::{
    join_sources, read_sources, sample_window, Machine, MachineError, SampleFacts, Sources,
};
use crate::pid::Pid;
use crate::stream::{FaultStream, Stopper, StreamEvent, TimedSample};
use crate::table::ProcessRow;
use crate::vm::VmCounters;

/// How many updates the channel holds before the sampler waits for the view.
pub const UPDATE_BACKLOG: usize = 64;

/// How long the sampler waits for an event before it looks at the stop flag.
const POLL: Duration = Duration::from_millis(200);

/// Why the gap before the first sample of a new `top` happened.
pub const RESTART_REASON: &str = "faulte waited for a new top to start";

/// The thread that samples this Mac, and the channel of its updates.
pub struct Sampler {
    /// The updates of the sampler thread.
    updates: Receiver<Update>,
    /// The stop state that the thread shares.
    control: Arc<Control>,
}

/// The stop state that the owner and the thread share.
#[derive(Default)]
struct Control {
    /// Tells whether the owner asked the thread to stop.
    stopped: AtomicBool,
    /// The stopper of the stream that runs now.
    stopper: Mutex<Option<Stopper>>,
}

impl Control {
    /// Asks the thread to stop, and stops the child of the current stream.
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let current = self
            .stopper
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(stopper) = current {
            stopper.stop();
        }
    }

    /// Tells whether the owner asked the thread to stop.
    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Makes `stopper` the stopper of the current stream, or clears it.
    fn publish(&self, stopper: Option<Stopper>) {
        *self.stopper.lock().unwrap_or_else(PoisonError::into_inner) = stopper;
    }
}

impl Sampler {
    /// Starts the sampler thread.
    ///
    /// The thread owns `machine`. `interval` is the delay between two
    /// samples. `own_pid` is the PID of `faulte`, which marks the rows of
    /// `faulte` and of its child processes.
    pub fn spawn<M: Machine + Send + 'static>(machine: M, interval: Span, own_pid: Pid) -> Self {
        let (sender, updates) = sync_channel(UPDATE_BACKLOG);
        let control = Arc::new(Control::default());
        let run = Run {
            machine,
            interval,
            own_pid,
            sender,
            control: Arc::clone(&control),
        };
        thread::spawn(move || run.start());
        Self { updates, control }
    }

    /// Stops the sampler and its child at once.
    ///
    /// This call does not join the thread, because a `ps` can run for
    /// seconds. The `Stopper` of the current stream stops `top` at once.
    pub fn stop(&self) {
        self.control.stop();
    }

    /// Gives the channel of the updates.
    #[must_use]
    pub fn updates(&self) -> &Receiver<Update> {
        &self.updates
    }
}

impl Drop for Sampler {
    /// Stops the thread and its child, so no `top` outlives the view.
    fn drop(&mut self) {
        self.stop();
    }
}

/// The result of a read of the counters: the counters, or the text of the
/// error.
type CounterRead = Result<VmCounters, String>;

/// A read of the counters, and when it happened.
#[derive(Clone)]
struct TimedCounters {
    /// The counters, or the text of the error.
    read: CounterRead,
    /// When the sampler read the counters, on the monotonic clock. The clock
    /// of the wall can move back between two reads.
    at: Instant,
}

/// The result of a read of the other sources: what they said, or the text of
/// the error.
type SourceRead = Result<Sources, String>;

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
    /// The stop state that the owner shares.
    control: Arc<Control>,
}

/// What the thread does after it reads a stream.
enum Flow {
    /// The stream ended. Wait one interval, then start a new one.
    Restart,
    /// The view dropped the channel. End the thread.
    Stop,
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
        while !self.control.is_stopped() {
            let flow = match self.machine.fault_stream(self.interval) {
                Ok(stream) => {
                    // Publish the stopper first, and look at the flag after
                    // it. A stop that came during the start sees either the
                    // stopper or the flag, so the new child never runs on.
                    self.control.publish(Some(stream.stopper()));
                    let flow = if self.control.is_stopped() {
                        stream.stopper().stop();
                        Flow::Stop
                    } else {
                        self.read(stream, &mut ledger)
                    };
                    self.control.publish(None);
                    flow
                }
                Err(error) => {
                    let now = self.machine.now();
                    if self.send_gap(&mut ledger, now, error.to_string()) {
                        Flow::Restart
                    } else {
                        Flow::Stop
                    }
                }
            };
            match flow {
                Flow::Restart if !self.control.is_stopped() => {
                    self.machine.sleep(Duration::from(self.interval));
                }
                Flow::Restart | Flow::Stop => return,
            }
        }
    }

    /// Reads one stream until it ends or the channel closes.
    fn read(&self, mut stream: Box<dyn FaultStream>, ledger: &mut Ledger) -> Flow {
        let mut reads: [Option<TimedCounters>; 2] = [None, None];
        let mut sources: Option<SourceRead> = None;
        let mut first_header = true;
        loop {
            let Some(event) = stream.next_event(POLL) else {
                if self.control.is_stopped() {
                    return Flow::Stop;
                }
                continue;
            };
            let delivered = match event {
                StreamEvent::Header { at, finished } => {
                    let first = std::mem::replace(&mut first_header, false);
                    let waited = first && ledger.covered_until.is_some();
                    let restart_sent =
                        !waited || self.send_gap(ledger, at, RESTART_REASON.to_owned());
                    let before = reads.clone();
                    reads = [reads[1].take(), Some(self.read_counters())];
                    let sent = restart_sent
                        && match (finished, before, sources.take()) {
                            (Some(sample), [Some(older), Some(newer)], Some(read)) => {
                                self.send_sample(&sample, older, newer, read, ledger)
                            }
                            _ => true,
                        };
                    // The rows of the sample of this header are in the pipe
                    // now, so this read is the closest to that sample. It
                    // comes after the send, because it is the slow one. The
                    // first sample of a child makes no frame and needs no
                    // read.
                    if sent && !first {
                        sources = Some(self.read_sources());
                    }
                    sent
                }
                StreamEvent::Refused { at, reason } => self.send_gap(ledger, at, reason),
                StreamEvent::Ended { at, reason } => {
                    return if self.send_gap(ledger, at, reason) {
                        Flow::Restart
                    } else {
                        Flow::Stop
                    };
                }
            };
            if !delivered {
                return Flow::Stop;
            }
        }
    }

    /// Reads the counters of the virtual memory system, and notes when.
    fn read_counters(&self) -> TimedCounters {
        let at = Instant::now();
        let read = self
            .machine
            .vm_counters()
            .map_err(|error: MachineError| error.to_string());
        TimedCounters { read, at }
    }

    /// Reads every source other than the sample.
    fn read_sources(&self) -> SourceRead {
        read_sources(&self.machine).map_err(|error: MachineError| error.to_string())
    }

    /// Makes the frame of `sample`, or a gap with the reason that it failed.
    /// Gives `false` when the view dropped the channel.
    fn send_sample(
        &self,
        sample: &TimedSample,
        older: TimedCounters,
        newer: TimedCounters,
        sources: SourceRead,
        ledger: &mut Ledger,
    ) -> bool {
        // The swap delta covers the time between the two reads. A slow read of
        // the other sources delays the read of the counters after it, so the
        // window can be longer or shorter than the sample.
        let swap_window = newer.at.saturating_duration_since(older.at);
        let outcome = older.read.and_then(|before| {
            newer.read.and_then(|after| {
                let facts = SampleFacts {
                    interval: self.interval,
                    window: sample_window(self.interval, sample.sample.elapsed, sample.wall),
                    faults: &sample.sample.rows,
                    before,
                    after,
                    swap_window,
                };
                sources.map(|sources| join_sources(&self.machine, &facts, sources))
            })
        });
        match outcome {
            Ok(observed) => {
                ledger.covered_until = Some(sample.ended_at);
                let own = self.own_set(&observed.table);
                self.sender
                    .send(Update::Frame(Box::new(SampledFrame {
                        observed,
                        at: sample.ended_at,
                        own,
                    })))
                    .is_ok()
            }
            Err(reason) => self.send_gap(ledger, sample.ended_at, reason),
        }
    }

    /// Gives the PID of `faulte` and the PID of each child process of it in
    /// `table`.
    fn own_set(&self, table: &[ProcessRow]) -> BTreeSet<Pid> {
        table
            .iter()
            .filter(|row| row.ppid == self.own_pid)
            .map(|row| row.pid)
            .chain(std::iter::once(self.own_pid))
            .collect()
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

    use std::collections::{BTreeSet, HashMap, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
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
        /// The process table after a stated count of events, in place of
        /// `table`. A test uses it to see when the sampler read the table.
        tables_at: Mutex<HashMap<usize, Vec<ProcessRow>>>,
        /// The count of the events that the streams gave.
        given: AtomicUsize,
        /// The count of the reads of the process table.
        table_reads: AtomicUsize,
        /// The count of the polls that found no event.
        idle_polls: AtomicUsize,
        /// The argument of each call of `sleep`.
        sleeps: Mutex<Vec<Duration>>,
        /// The count of the streams that the sampler started.
        starts: AtomicUsize,
        /// The count of the calls of the stopper of a stream.
        stops: AtomicUsize,
        /// Tells whether the machine was dropped.
        dropped: AtomicBool,
        /// When set, `fault_stream` waits for a message here before it gives
        /// its stream.
        gate: Mutex<Option<Receiver<()>>>,
        /// How long each read of the process table takes.
        table_delay: Mutex<Duration>,
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

    /// A stream that gives stated events, then nothing. After its stopper
    /// ran, it gives the end of the child once, as a real `top` does.
    struct FakeStream {
        /// The events that are left.
        events: VecDeque<StreamEvent>,
        /// Tells whether the stopper ran.
        stopped: Arc<AtomicBool>,
        /// The state that the test also reads.
        shared: Arc<Shared>,
    }

    impl FaultStream for FakeStream {
        fn next_event(&mut self, _wait: Duration) -> Option<StreamEvent> {
            if let Some(event) = self.events.pop_front() {
                self.shared.given.fetch_add(1, Ordering::SeqCst);
                return Some(event);
            }
            if self.stopped.swap(false, Ordering::SeqCst) {
                return Some(StreamEvent::Ended {
                    at: at(NOW),
                    reason: "stopped by faulte".to_owned(),
                });
            }
            thread::sleep(LOOK);
            self.shared.idle_polls.fetch_add(1, Ordering::SeqCst);
            None
        }

        fn stopper(&self) -> Stopper {
            let shared = Arc::clone(&self.shared);
            let stopped = Arc::clone(&self.stopped);
            Stopper::new(move || {
                shared.stops.fetch_add(1, Ordering::SeqCst);
                stopped.store(true, Ordering::SeqCst);
            })
        }
    }

    impl Machine for FakeMachine {
        fn sample_faults(&self, _interval: Span) -> Result<(TopSample, Duration), MachineError> {
            unreachable!("the sampler reads a stream");
        }

        fn fault_stream(&self, _interval: Span) -> Result<Box<dyn FaultStream>, MachineError> {
            self.shared.starts.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = locked(&self.shared.gate).take() {
                let _ = gate.recv_timeout(BOUND);
            }
            let next = locked(&self.shared.streams).pop_front();
            let events = next.unwrap_or_else(|| Ok(Vec::new()))?;
            Ok(Box::new(FakeStream {
                events: events.into(),
                stopped: Arc::new(AtomicBool::new(false)),
                shared: Arc::clone(&self.shared),
            }))
        }

        fn process_table(&self) -> Result<Vec<ProcessRow>, MachineError> {
            self.shared.table_reads.fetch_add(1, Ordering::SeqCst);
            thread::sleep(*locked(&self.shared.table_delay));
            if let Some(error) = locked(&self.shared.table_errors).pop_front() {
                return Err(error);
            }
            let given = self.shared.given.load(Ordering::SeqCst);
            if let Some(table) = locked(&self.shared.tables_at).get(&given) {
                return Ok(table.clone());
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

        /// States the process table that a read gets after `events` events.
        fn table_after(self, events: usize, table: Vec<ProcessRow>) -> Self {
            locked(&self.shared.tables_at).insert(events, table);
            self
        }

        /// Makes each read of the process table take `delay`.
        fn slow_table(self, delay: Duration) -> Self {
            *locked(&self.shared.table_delay) = delay;
            self
        }

        /// Adds an error for the next read of the process table.
        fn failing_table(self, error: MachineError) -> Self {
            locked(&self.shared.table_errors).push_back(error);
            self
        }

        /// Makes the next start wait until the returned sender sends.
        fn gated(&self) -> Sender<()> {
            let (sender, receiver) = channel();
            *locked(&self.shared.gate) = Some(receiver);
            sender
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
    }

    /// The swap window of a frame is the time between its two reads of the
    /// counters, on the monotonic clock. A slow read of the other sources
    /// delays the next read of the counters, and the window then holds that
    /// delay too, so the swap rate stays true. Each sample here states a
    /// window of 4 s, and the reads are a short time apart.
    #[test]
    fn the_swap_window_is_the_time_between_the_two_reads_of_the_counters() {
        let slow = Duration::from_millis(300);
        let started = std::time::Instant::now();
        let rig = Rig::new().slow_table(slow).stream(vec![
            header(10),
            header(12),
            finished(14, 12, 7_000),
            finished(16, 14, 8_000),
        ]);
        let sampler = rig.start();

        let first = next_frame(&sampler);
        let second = next_frame(&sampler);
        let elapsed = started.elapsed();

        assert!(
            first.observed.swap_window <= elapsed,
            "{:?} is longer than the test",
            first.observed.swap_window
        );
        assert!(
            (slow..=elapsed).contains(&second.observed.swap_window),
            "{:?} holds the slow read and is not longer than the test",
            second.observed.swap_window
        );
    }

    /// `top` prints a sample at the end of its window, and `faulte` learns
    /// only at the next header that the sample is complete. A process can
    /// exit in that time, so the table of a sample comes from the moment
    /// that its own header arrived. A table of one sample later left 42% to
    /// 51% of the faults of a busy Mac with no row.
    #[test]
    fn a_frame_joins_the_table_that_was_read_when_its_sample_arrived() {
        let rig = Rig::new()
            .table_after(2, vec![process(200, 1)])
            .table_after(3, vec![process(300, 1)])
            .stream(vec![header(10), header(12), finished(14, 12, 7_000)]);
        let sampler = rig.start();

        let frame = next_frame(&sampler);

        let pids: Vec<u32> = frame
            .observed
            .ranking
            .rows
            .iter()
            .map(|row| row.pid.get())
            .collect();
        assert_eq!(pids, [200]);
        assert_eq!(frame.observed.ranking.skipped.exited, 0);
    }

    /// The first sample of a child holds the counter since each process
    /// started and makes no frame, so the sampler reads no table for it. This
    /// tool runs on a Mac that is short of memory.
    #[test]
    fn the_first_header_of_a_child_reads_no_table() {
        let rig = Rig::new().stream(vec![header(10), header(12), finished(14, 12, 7_000)]);
        let shared = Arc::clone(&rig.shared);
        let sampler = rig.start();

        let _ = next_frame(&sampler);
        wait_until("the stream is idle", || {
            shared.idle_polls.load(Ordering::SeqCst) > 0
        });

        assert_eq!(shared.table_reads.load(Ordering::SeqCst), 2);
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

    /// Output that the parser refused is a gap. The gap ends when the output
    /// arrived, and it begins where the last frame ended.
    #[test]
    fn refused_output_makes_a_gap() {
        let rig = Rig::new().stream(vec![
            header(10),
            header(12),
            finished(14, 12, 7_000),
            StreamEvent::Refused {
                at: at(20),
                reason: "top printed rubbish".to_owned(),
            },
        ]);
        let sampler = rig.start();

        let _frame = next_frame(&sampler);
        let gap = next_gap(&sampler);

        assert_eq!(gap.reason, "top printed rubbish");
        assert_eq!(gap.at, at(20));
        assert_eq!(gap.lost, Duration::from_secs(8));
    }

    /// A `top` that ended is a gap with the reason of its end. The sampler
    /// waits for one interval and starts a new `top`. The first header of the
    /// new `top` is a second gap, with the reason of the wait, and the lost
    /// times of the two gaps add up to the time between the last frame and
    /// that header.
    #[test]
    fn an_ended_top_makes_a_gap_and_a_new_top_starts() {
        let rig = Rig::new()
            .stream(vec![
                header(10),
                header(12),
                finished(14, 12, 7_000),
                StreamEvent::Ended {
                    at: at(20),
                    reason: "top ended with exit status: 1".to_owned(),
                },
            ])
            .stream(vec![header(30), header(32), finished(34, 32, 8_000)]);
        let sampler = rig.start();

        let _first = next_frame(&sampler);
        let ended = next_gap(&sampler);
        let restart = next_gap(&sampler);
        let second = next_frame(&sampler);

        assert_eq!(ended.reason, "top ended with exit status: 1");
        assert_eq!(ended.at, at(20));
        assert_eq!(ended.lost, Duration::from_secs(8));
        assert_eq!(restart.reason, RESTART_REASON);
        assert_eq!(restart.at, at(30));
        assert_eq!(restart.lost, Duration::from_secs(10));
        assert_eq!(second.at, at(32));
        assert_eq!(
            *locked(&rig.shared.sleeps),
            vec![Duration::from(interval())]
        );
        assert_eq!(rig.shared.starts.load(Ordering::SeqCst), 2);
    }

    /// A `top` that does not start is a gap with the reason. The sampler
    /// waits for one interval and tries again. The wait that the first
    /// header of the next `top` ends is the second gap.
    #[test]
    fn a_failed_start_makes_a_gap_and_the_sampler_tries_again() {
        let rig = Rig::new()
            .failing_start(MachineError::CommandDidNotStart {
                program: "/usr/bin/top".to_owned(),
                reason: "no such file".to_owned(),
            })
            .stream(vec![
                header(NOW + 10),
                header(NOW + 12),
                finished(NOW + 14, NOW + 12, 7_000),
            ]);
        let sampler = rig.start();

        let failed = next_gap(&sampler);
        let restart = next_gap(&sampler);
        let frame = next_frame(&sampler);

        assert_eq!(failed.reason, "/usr/bin/top did not start: no such file");
        assert_eq!(failed.at, at(NOW));
        assert_eq!(failed.lost, Duration::ZERO);
        assert_eq!(restart.reason, RESTART_REASON);
        assert_eq!(restart.lost, Duration::from_secs(10));
        assert_eq!(frame.at, at(NOW + 12));
        assert_eq!(
            *locked(&rig.shared.sleeps),
            vec![Duration::from(interval())]
        );
        assert_eq!(rig.shared.starts.load(Ordering::SeqCst), 2);
    }

    /// The own set of a frame holds `faulte` and each child process of it. A
    /// grandchild and a process of another parent are not in it.
    #[test]
    fn the_own_set_holds_faulte_and_its_children() {
        let rig = Rig::new().stream(vec![header(10), header(12), finished(14, 12, 7_000)]);
        let sampler = rig.start();

        let frame = next_frame(&sampler);

        assert_eq!(frame.own, BTreeSet::from([Pid::new(OWN), Pid::new(101)]));
    }

    /// Reads updates until the channel disconnects. Fails after [`BOUND`].
    fn drain_until_disconnected(sampler: &Sampler) {
        loop {
            match sampler.updates().recv_timeout(BOUND) {
                Ok(_) => {}
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => panic!("the sampler did not end"),
            }
        }
    }

    /// `stop` calls the stopper of the current stream, and no new child
    /// starts after it. The stream gives the end of its child after the stop,
    /// and a sampler that ignored the stop would start a new one.
    #[test]
    fn stop_stops_the_child_and_starts_no_new_one() {
        let rig = Rig::new().stream(vec![header(10)]).stream(vec![header(30)]);
        let sampler = rig.start();
        wait_until("the first child", || {
            rig.shared.starts.load(Ordering::SeqCst) == 1
        });

        sampler.stop();

        wait_until("the stopper", || {
            rig.shared.stops.load(Ordering::SeqCst) >= 1
        });
        drain_until_disconnected(&sampler);
        assert_eq!(rig.shared.starts.load(Ordering::SeqCst), 1);
    }

    /// A stop that arrives while a child starts still stops that child. The
    /// sampler publishes the stopper first, and then looks at the flag.
    #[test]
    fn a_stop_during_a_start_stops_the_new_child() {
        let rig = Rig::new().stream(vec![header(10)]);
        let release = rig.gated();
        let sampler = rig.start();
        wait_until("the start", || {
            rig.shared.starts.load(Ordering::SeqCst) == 1
        });

        sampler.stop();
        release.send(()).expect("the start waits");

        drain_until_disconnected(&sampler);
        assert_eq!(rig.shared.stops.load(Ordering::SeqCst), 1);
        assert_eq!(rig.shared.starts.load(Ordering::SeqCst), 1);
    }

    /// Dropping the sampler ends the thread. The thread owns the machine, so
    /// the drop of the machine shows that the thread ended, and the stopper
    /// of the stream ran.
    #[test]
    fn dropping_the_sampler_ends_the_thread() {
        let rig = Rig::new().stream(vec![header(10)]);
        let sampler = rig.start();
        wait_until("the first child", || {
            rig.shared.starts.load(Ordering::SeqCst) == 1
        });

        drop(sampler);

        wait_until("the end of the thread", || {
            rig.shared.dropped.load(Ordering::SeqCst)
        });
        assert!(rig.shared.stops.load(Ordering::SeqCst) >= 1);
        assert_eq!(rig.shared.starts.load(Ordering::SeqCst), 1);
    }
}
