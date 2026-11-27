//! The history of the samples that the view of `faulte tui` shows.
//!
//! The history holds a fixed memory budget of compact frames and gaps. A frame
//! is the ranking and the memory counters of one sample. A gap is a time that
//! the view tried to measure and did not. The module holds no terminal, no
//! clock, and no process. A [`SystemTime`] arrives only as data.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::mem::size_of;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use occ::SessionId;

use crate::duration::Span;
use crate::machine::Observed;
use crate::pid::{Pid, Uid};
use crate::ranking::{ClaudeTotal, ClaudeView, RankedRow, Ranking, Skipped};
use crate::render::{self, Accounts};
use crate::state::SessionState;
use crate::vm::{SwapUsage, VmDelta};

/// The memory that a history keeps, in bytes.
pub const BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// The most text of one command that the history keeps.
pub const STORED_COMMAND_BYTES: usize = 16 * 1024;

/// The most text of one directory, status, or gap reason that the history
/// keeps.
const STORED_TEXT_BYTES: usize = 1024;

/// The share of the budget that the text of one frame may use, as a divisor.
/// A frame whose commands are all as long as the cut then holds at most half
/// of the budget in text, and the rest of the frame fits beside it.
const TEXT_SHARE_DIVISOR: usize = 2;

/// The share of the budget that the deque of the entries may use, as a
/// divisor. The deque doubles its capacity as it grows and never gives it
/// back. A push that would double the capacity past this share makes the
/// oldest entry leave first, so the push uses the free slot. The rows of the
/// frames get the rest of the budget.
const ENTRY_SHARE_DIVISOR: usize = 8;

/// The value of `rss_kib` of a compact row that means "no value".
const NO_RSS: u32 = u32::MAX;

/// The names of an entry of the history. A name never repeats, so a holder of
/// a name can tell that its entry left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryId(u64);

/// The name of a process. A PID can return for another process, so the start
/// time is part of the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessKey {
    /// The PID of the process.
    pub pid: Pid,
    /// The start time of the process, in seconds since the Unix epoch. `None`
    /// for PID 0.
    pub started_at_epoch_secs: Option<u64>,
}

/// One sample, as the sampler hands it over.
pub struct SampledFrame {
    /// What the run read from the machine.
    pub observed: Observed,
    /// The time of the sample.
    pub at: SystemTime,
    /// The PIDs of `faulte` and of its child processes.
    pub own: BTreeSet<Pid>,
}

/// A time that the view tried to measure and did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    /// The time when the gap began.
    pub at: SystemTime,
    /// The length of the time that was lost.
    pub lost: Duration,
    /// Why the time was lost.
    pub reason: String,
}

/// What the sampler gives to the history.
pub enum Update {
    /// A sample.
    Frame(Box<SampledFrame>),
    /// A time with no sample.
    Gap(Gap),
}

/// How much of the history a reading covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanWidth {
    /// The one frame that the reading ends at.
    OneFrame,
    /// The last minute.
    OneMinute,
    /// The last five minutes.
    FiveMinutes,
    /// The last fifteen minutes.
    FifteenMinutes,
    /// Every entry up to the end.
    Whole,
}

impl SpanWidth {
    /// Gives the label that the header of a span states.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::OneFrame => "this frame",
            Self::OneMinute => "the last 1 m",
            Self::FiveMinutes => "the last 5 m",
            Self::FifteenMinutes => "the last 15 m",
            Self::Whole => "the whole history",
        }
    }

    /// Gives the next width in the cycle: one frame, 1 m, 5 m, 15 m, the whole
    /// history, and then one frame again.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::OneFrame => Self::OneMinute,
            Self::OneMinute => Self::FiveMinutes,
            Self::FiveMinutes => Self::FifteenMinutes,
            Self::FifteenMinutes => Self::Whole,
            Self::Whole => Self::OneFrame,
        }
    }

    /// Gives how far back from the end the span reaches. `None` means one
    /// frame, or the whole history.
    fn reach(self) -> Option<Duration> {
        const MINUTE_SECS: u64 = 60;
        match self {
            Self::OneFrame | Self::Whole => None,
            Self::OneMinute => Some(Duration::from_secs(MINUTE_SECS)),
            Self::FiveMinutes => Some(Duration::from_secs(5 * MINUTE_SECS)),
            Self::FifteenMinutes => Some(Duration::from_secs(15 * MINUTE_SECS)),
        }
    }
}

/// One row of a reading, with the facts that belong to it alone.
#[derive(Debug, Clone, Copy)]
pub struct RowView<'a> {
    /// The row.
    pub row: &'a RankedRow,
    /// The name of the process.
    pub key: ProcessKey,
    /// The time of the frame that the facts of the row come from.
    pub as_of: SystemTime,
    /// Whether the process is `faulte` or its child.
    pub own: bool,
}

/// What a reading knows of one row, beside the row itself.
#[derive(Debug)]
struct RowExtra {
    /// The time of the frame that the facts of the row come from.
    as_of: SystemTime,
    /// Whether the process is `faulte` or its child.
    own: bool,
}

/// One cell of a row of the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// Nothing to show: the process has no row in the cell.
    Blank,
    /// The cell holds gaps only.
    Gap,
    /// The cell holds a frame.
    Bar {
        /// The height of the bar, from 0 to 7.
        level: u8,
        /// Whether the cell holds a gap besides the frame.
        holds_gap: bool,
    },
}

/// The two rows of the timeline, and the cell of the selected entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    /// The fault rate of the Mac, one cell for each slice of the history.
    pub faults: Vec<Cell>,
    /// The swap rate of the Mac, in the same slices.
    pub swap: Vec<Cell>,
    /// The cell of the selected entry.
    pub marker: Option<usize>,
}

/// What the history holds, in counts and times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Extent {
    /// The count of frames.
    pub frames: usize,
    /// The count of gaps.
    pub gaps: usize,
    /// The time of the oldest entry.
    pub oldest_at: Option<SystemTime>,
    /// The time of the newest entry.
    pub newest_at: Option<SystemTime>,
    /// The sum of the windows of the frames: the time that the history
    /// measured.
    pub measured: Duration,
}

/// The history of one process, as a row of cells.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    /// The name of the process.
    pub key: ProcessKey,
    /// The account.
    pub uid: Uid,
    /// The newest command, as the history stored it.
    pub command: String,
    /// Whether the process is `faulte` or its child.
    pub own: bool,
    /// The id of the session, when the process is one.
    pub session: Option<String>,
    /// The directory of the session.
    pub directory: Option<String>,
    /// The fault rate of the process in the slices of the timeline.
    pub cells: Vec<Cell>,
    /// The cell of the selected entry.
    pub marker: Option<usize>,
    /// The highest fault rate of the process over the whole history.
    pub peak_rate: f64,
    /// The time of the oldest frame in which the process made faults.
    pub first_at: Option<SystemTime>,
    /// The time of the newest frame in which the process made faults.
    pub last_at: Option<SystemTime>,
    /// The count of frames in which the process made faults.
    pub frames_with_faults: usize,
}

/// The numbers of the header that a reading states beside its ranking.
#[derive(Debug)]
struct HeaderFacts {
    /// The count of processes that the first line states.
    processes: usize,
    /// The swap traffic.
    swap: VmDelta,
    /// The swap file.
    usage: SwapUsage,
    /// The size of the compressor in bytes.
    compressor_bytes: u64,
    /// The time between the two reads of the counters.
    swap_window: Duration,
    /// The interval that the person asked for.
    interval: Span,
    /// The span that the reading covers. `None` for one frame.
    span: Option<SpanFacts>,
}

/// What the header says about the span of a reading.
#[derive(Debug)]
struct SpanFacts {
    /// The label of the width of the span.
    label: &'static str,
    /// The count of frames in the span.
    frames: usize,
    /// The count of gaps in the span.
    gaps: usize,
}

/// A ranking that the history made from one frame or from a span of frames.
///
/// The extras of the rows are private and `rows` pairs each row with its own
/// extras, so that no caller can pair a row with the wrong ones.
#[derive(Debug)]
pub struct Reading {
    /// The name of the last entry of the reading.
    end: EntryId,
    /// The time of the last entry of the reading.
    at: SystemTime,
    /// The ranking.
    ranking: Ranking,
    /// One extra for each row of the ranking.
    extras: Vec<RowExtra>,
    /// The count of processes with no fault. A span gives 0.
    others: usize,
    /// The facts of the header.
    header: HeaderFacts,
}

impl Reading {
    /// Gives the name of the last entry of the reading.
    #[must_use]
    pub fn end(&self) -> EntryId {
        self.end
    }

    /// Gives the time of the last entry of the reading.
    #[must_use]
    pub fn at(&self) -> SystemTime {
        self.at
    }

    /// Gives the ranking. Its rows are the rows with at least one fault.
    #[must_use]
    pub fn ranking(&self) -> &Ranking {
        &self.ranking
    }

    /// Gives each row with its facts.
    pub fn rows(&self) -> impl ExactSizeIterator<Item = RowView<'_>> {
        self.ranking
            .rows
            .iter()
            .zip(self.extras.iter())
            .map(|(row, extra)| RowView {
                row,
                key: ProcessKey {
                    pid: row.pid,
                    started_at_epoch_secs: row.started_at_epoch_secs,
                },
                as_of: extra.as_of,
                own: extra.own,
            })
    }

    /// Gives the row at `index`.
    #[must_use]
    pub fn row(&self, index: usize) -> Option<RowView<'_>> {
        self.rows().nth(index)
    }

    /// Gives the index of the row of `key`.
    #[must_use]
    pub fn position_of(&self, key: ProcessKey) -> Option<usize> {
        self.rows().position(|view| view.key == key)
    }

    /// Gives the count of processes with no faults. A span gives 0.
    #[must_use]
    pub fn other_rows(&self) -> usize {
        self.others
    }

    /// Gives the lines above the table: [`render::header`] for one frame and
    /// [`render::span_header`] for a span.
    #[must_use]
    pub fn header_lines(&self) -> Vec<String> {
        let facts = &self.header;
        let measurement = render::Measurement {
            ranking: &self.ranking,
            processes: facts.processes,
            swap: facts.swap,
            usage: facts.usage,
            compressor_bytes: facts.compressor_bytes,
            swap_window: facts.swap_window,
            interval: facts.interval,
        };
        match &facts.span {
            None => render::header(&measurement),
            Some(span) => render::span_header(
                &measurement,
                &render::SpanTotals {
                    label: span.label,
                    frames: span.frames,
                    gaps: span.gaps,
                },
            ),
        }
    }
}

/// The newest row of one identity in a span.
#[derive(Debug, Clone, Copy)]
struct Newest {
    /// The place of the frame in the list of frames, newest first.
    frame: usize,
    /// The resident memory in KiB.
    rss_kib: u32,
    /// What the row said about Claude Code.
    state: CompactState,
}

/// The highest level of a bar.
const MAX_LEVEL: u8 = 7;

/// The count of levels of a bar. A rate that equals the peak is the top level.
const LEVELS: f64 = 8.0;

/// What one entry says to a row of cells.
#[derive(Debug, Clone, Copy)]
enum Sample {
    /// The entry is a gap.
    Gap,
    /// The entry is a frame. `None` means that the row has no value in it.
    Frame(Option<f64>),
}

/// Gives `count` per second over `window`, or 0 for a window of no time.
fn per_second(count: u64, window: Duration) -> f64 {
    if window.is_zero() {
        return 0.0;
    }
    count as f64 / window.as_secs_f64()
}

/// Gives the highest rate of `samples`, or 0 when there is none.
fn peak_of(samples: &[Sample]) -> f64 {
    samples
        .iter()
        .filter_map(|sample| match sample {
            Sample::Frame(Some(rate)) => Some(*rate),
            _ => None,
        })
        .fold(0.0, f64::max)
}

/// Gives `floor(rate / peak * 8)` clamped to the top level. A peak of 0 gives
/// level 0.
fn level_of(rate: f64, peak: f64) -> u8 {
    if peak <= 0.0 {
        return 0;
    }
    let reached = (1..=MAX_LEVEL)
        .take_while(|step| rate * LEVELS >= f64::from(*step) * peak)
        .count();
    u8::try_from(reached).unwrap_or(MAX_LEVEL)
}

/// Squeezes `samples` into `min(cells, samples.len())` cells.
///
/// A slice with a value shows the highest of them, and `holds_gap` when the
/// slice also holds a gap. A slice with frames and no value is blank. A slice
/// with gaps only is a gap.
fn cells_of(samples: &[Sample], cells: usize, peak: f64) -> Vec<Cell> {
    let total = samples.len();
    let used = cells.min(total);
    (0..used)
        .map(|index| {
            let slice = &samples[index * total / used..(index + 1) * total / used];
            let mut best: Option<f64> = None;
            let mut holds_gap = false;
            let mut holds_frame = false;
            for sample in slice {
                match sample {
                    Sample::Gap => holds_gap = true,
                    Sample::Frame(rate) => {
                        holds_frame = true;
                        if let Some(rate) = rate {
                            best = Some(best.map_or(*rate, |best| best.max(*rate)));
                        }
                    }
                }
            }
            match best {
                Some(rate) => Cell::Bar {
                    level: level_of(rate, peak),
                    holds_gap,
                },
                None if holds_frame => Cell::Blank,
                None => Cell::Gap,
            }
        })
        .collect()
}

/// What a compact row says about Claude Code.
///
/// The text and the session of a row live in its identity record, because
/// many rows of one process share them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompactState {
    /// The process is not a Claude Code session.
    NotClaude,
    /// A Claude Code session with no registry record.
    NoRecord,
    /// A Claude Code process of another account.
    OtherAccount,
    /// The session runs a turn.
    Busy,
    /// The session waits for an answer.
    Waiting,
    /// The session is idle for a whole number of seconds, when known.
    Idle(Option<u32>),
    /// A status that `occ` does not know. The text is in the identity.
    Other,
    /// The record gives no status.
    Unknown,
}

/// One row of a frame, in 24 bytes.
#[derive(Debug, Clone, Copy)]
struct CompactRow {
    /// The faults of the process over the window of the frame.
    faults: u64,
    /// The index of the identity record in the slab.
    identity: u32,
    /// The resident memory in KiB. [`NO_RSS`] means no value.
    rss_kib: u32,
    /// What the row says about Claude Code.
    state: CompactState,
}

const _: () = assert!(size_of::<CompactRow>() == 24);

/// What the identity of a Claude Code session remembers about its session.
#[derive(Debug)]
struct SessionDetail {
    /// The session id.
    id: SessionId,
    /// The directory of the session.
    directory: Option<String>,
    /// The text of the last status that `occ` does not know.
    other_status: Option<String>,
}

/// The facts of one process that do not change from frame to frame.
#[derive(Debug)]
struct Identity {
    /// The PID.
    pid: Pid,
    /// The account.
    uid: Uid,
    /// The start time.
    started: Option<u64>,
    /// The newest command, cut.
    command: String,
    /// The session, when the process is a session.
    session: Option<SessionDetail>,
    /// Whether the process is `faulte` or its child.
    own: bool,
    /// The count of rows that point at this record.
    rows: u32,
}

impl Identity {
    /// Gives the name of the process.
    fn key(&self) -> ProcessKey {
        ProcessKey {
            pid: self.pid,
            started_at_epoch_secs: self.started,
        }
    }

    /// Gives the bytes of text that this record owns.
    fn text_bytes(&self) -> usize {
        let session = self.session.as_ref().map_or(0, |detail| {
            detail.id.as_str().len()
                + detail.directory.as_ref().map_or(0, String::capacity)
                + detail.other_status.as_ref().map_or(0, String::capacity)
        });
        self.command.capacity() + session
    }
}

/// Cuts `text` to at most `limit` bytes at a character boundary, and ends a
/// cut text with [`render::MORE`]. A text that fits stays as it is.
fn cut_text(text: &str, limit: usize) -> Cow<'_, str> {
    if text.len() <= limit {
        return Cow::Borrowed(text);
    }
    let room = limit.saturating_sub(render::MORE.len_utf8());
    let mut cut = String::with_capacity(room + render::MORE.len_utf8());
    cut.extend(
        text.char_indices()
            .take_while(|(index, character)| index + character.len_utf8() <= room)
            .map(|(_, character)| character),
    );
    cut.push(render::MORE);
    cut.shrink_to_fit();
    Cow::Owned(cut)
}

/// Gives an owned, exact-size copy of `text` cut at `limit`.
fn stored_text(text: &str, limit: usize) -> String {
    let mut stored = cut_text(text, limit).into_owned();
    stored.shrink_to_fit();
    stored
}

/// Sets `slot` to the cut of `text`, unless it holds that text already.
fn set_text(slot: &mut String, text: &str, limit: usize) {
    let cut = cut_text(text, limit);
    if slot.as_str() != cut {
        *slot = stored_text(text, limit);
    }
}

/// The identity records, and the index that finds one by process.
#[derive(Debug, Default)]
struct Identities {
    /// The records. A freed slot holds `None`.
    slab: Vec<Option<Identity>>,
    /// The freed slots.
    free: Vec<u32>,
    /// The slot of each process.
    index: HashMap<ProcessKey, u32>,
    /// The bytes of text that the live records own.
    text_bytes: usize,
}

impl Identities {
    /// Points one more row at the record of `row`, and makes the record when
    /// it is new. The newest command wins, a session detail that is present
    /// wins, and `own` can only turn on.
    fn acquire(&mut self, row: &RankedRow, own: bool, command_limit: usize) -> u32 {
        let key = ProcessKey {
            pid: row.pid,
            started_at_epoch_secs: row.started_at_epoch_secs,
        };
        let slot = match self.index.get(&key) {
            Some(&slot) => slot,
            None => {
                let fresh = Identity {
                    pid: row.pid,
                    uid: row.uid,
                    started: row.started_at_epoch_secs,
                    command: String::new(),
                    session: None,
                    own: false,
                    rows: 0,
                };
                let slot = if let Some(slot) = self.free.pop() {
                    self.slab[slot as usize] = Some(fresh);
                    slot
                } else {
                    self.slab.push(Some(fresh));
                    u32::try_from(self.slab.len() - 1).expect("fewer than 2^32 identities")
                };
                self.index.insert(key, slot);
                slot
            }
        };
        let record = self.slab[slot as usize]
            .as_mut()
            .expect("an indexed slot is live");
        let before = record.text_bytes();
        record.rows += 1;
        record.uid = row.uid;
        record.own |= own;
        set_text(&mut record.command, &row.command, command_limit);
        if let ClaudeView::Session {
            id,
            state,
            directory,
            ..
        } = &row.claude
        {
            let detail = record.session.get_or_insert_with(|| SessionDetail {
                id: id.clone(),
                directory: None,
                other_status: None,
            });
            if detail.id != *id {
                detail.id = id.clone();
            }
            if let Some(directory) = directory {
                let text = directory.to_string_lossy();
                if detail.directory.as_deref() != Some(&*cut_text(&text, STORED_TEXT_BYTES)) {
                    detail.directory = Some(stored_text(&text, STORED_TEXT_BYTES));
                }
            }
            if let SessionState::Other(text) = state {
                if detail.other_status.as_deref() != Some(&*cut_text(text, STORED_TEXT_BYTES)) {
                    detail.other_status = Some(stored_text(text, STORED_TEXT_BYTES));
                }
            }
        }
        self.text_bytes = self.text_bytes - before + record.text_bytes();
        slot
    }

    /// Takes one row away from the record `slot`. A record with no row leaves.
    fn release(&mut self, slot: u32) {
        let Some(record) = self.slab[slot as usize].as_mut() else {
            return;
        };
        record.rows -= 1;
        if record.rows > 0 {
            return;
        }
        self.text_bytes -= record.text_bytes();
        let key = record.key();
        self.index.remove(&key);
        self.slab[slot as usize] = None;
        self.free.push(slot);
    }

    /// Gives the bytes that the records and their index hold.
    fn bytes(&self) -> usize {
        /// The control bytes that a table of the index keeps for each bucket.
        const CONTROL_BYTES: usize = 1;
        /// The buckets of a table hold `7/8` of its capacity.
        const LOAD_NUMERATOR: usize = 8;
        /// See [`LOAD_NUMERATOR`].
        const LOAD_DENOMINATOR: usize = 7;
        let capacity = self.index.capacity();
        let buckets = if capacity == 0 {
            0
        } else {
            (capacity * LOAD_NUMERATOR / LOAD_DENOMINATOR).next_power_of_two()
        };
        let index = buckets * (size_of::<(ProcessKey, u32)>() + CONTROL_BYTES);
        self.slab.capacity() * size_of::<Option<Identity>>()
            + self.free.capacity() * size_of::<u32>()
            + self.text_bytes
            + index
    }
}

/// One sample, kept in compact form.
#[derive(Debug)]
struct Frame {
    /// The time of the sample.
    at: SystemTime,
    /// The time that the counts cover.
    window: Duration,
    /// The faults of every process of the sample.
    total_faults: u64,
    /// The total of the Claude Code rows.
    claude: ClaudeTotal,
    /// The processes with no row.
    skipped: Skipped,
    /// The swap traffic.
    swap: VmDelta,
    /// The swap file.
    usage: SwapUsage,
    /// The size of the compressor in bytes.
    compressor_bytes: u64,
    /// The time between the two reads of the counters.
    swap_window: Duration,
    /// The interval that the person asked for.
    interval: Span,
    /// The count of processes with no fault. They have no stored row.
    others: u32,
    /// The rows with a fault, in the order of the ranking.
    rows: Box<[CompactRow]>,
}

/// One entry of the history.
#[derive(Debug)]
enum Entry {
    /// A sample.
    Frame(Frame),
    /// A time with no sample.
    Gap(Gap),
}

/// The compact history. See the module doc.
pub struct History {
    /// The most bytes that the history may hold.
    budget: usize,
    /// The entries, oldest first.
    entries: VecDeque<Entry>,
    /// The name of the next entry. The oldest entry has the name
    /// `next_id - entries.len()`.
    next_id: u64,
    /// The identity records.
    identities: Identities,
    /// The names of the accounts that the frames mentioned.
    accounts: Accounts,
    /// The rows that the frames store.
    stored_rows: usize,
    /// The bytes of the reasons of the gaps.
    gap_text_bytes: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    /// Makes an empty history with the budget [`BUDGET_BYTES`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_budget(BUDGET_BYTES)
    }

    /// Makes an empty history with a budget of `bytes`.
    #[must_use]
    pub fn with_budget(bytes: usize) -> Self {
        Self {
            budget: bytes,
            entries: VecDeque::new(),
            next_id: 0,
            identities: Identities::default(),
            accounts: Accounts::new(),
            stored_rows: 0,
            gap_text_bytes: 0,
        }
    }

    /// Adds an update, and gives its name.
    ///
    /// The oldest entry leaves first when the deque of the entries is full and
    /// a doubling would pass its share of the budget. Then the oldest entries
    /// leave while the history holds more than its budget and more than one
    /// entry. The entry that arrived always stays.
    pub fn add(&mut self, update: Update) -> EntryId {
        let entry = match update {
            Update::Frame(sampled) => Entry::Frame(self.compact(*sampled)),
            Update::Gap(gap) => {
                let reason = stored_text(&gap.reason, STORED_TEXT_BYTES);
                self.gap_text_bytes += reason.capacity();
                Entry::Gap(Gap { reason, ..gap })
            }
        };
        let id = EntryId(self.next_id);
        self.next_id += 1;
        if self.a_push_passes_the_entry_share() {
            self.evict_oldest();
        }
        self.entries.push_back(entry);
        while self.bytes_used() > self.budget && self.entries.len() > 1 {
            self.evict_oldest();
        }
        id
    }

    /// Turns a sample into a compact frame.
    ///
    /// The cut of a command is `min(STORED_COMMAND_BYTES, budget / (2 * rows))`.
    /// That bound holds the budget when one frame alone holds worst-case text.
    /// The count `rows` is the count of stored rows.
    fn compact(&mut self, sampled: SampledFrame) -> Frame {
        let Observed {
            ranking,
            swap,
            usage,
            compressor_bytes,
            swap_window,
            accounts,
            interval,
            ..
        } = sampled.observed;
        self.accounts.merge(&accounts);
        let total_rows = ranking.rows.len();
        let stored = ranking.rows.iter().filter(|row| row.faults > 0).count();
        let command_limit =
            STORED_COMMAND_BYTES.min(self.budget / (TEXT_SHARE_DIVISOR * stored.max(1)));
        let mut rows = Vec::with_capacity(stored);
        for row in ranking.rows {
            if row.faults == 0 {
                continue;
            }
            let own = sampled.own.contains(&row.pid);
            let identity = self.identities.acquire(&row, own, command_limit);
            rows.push(CompactRow {
                faults: row.faults,
                identity,
                rss_kib: row.rss_kib.map_or(NO_RSS, |kib| {
                    u32::try_from(kib).map_or(NO_RSS - 1, |kib| kib.min(NO_RSS - 1))
                }),
                state: compact_state(&row.claude),
            });
        }
        self.stored_rows += rows.len();
        Frame {
            at: sampled.at,
            window: ranking.window,
            total_faults: ranking.total_faults,
            claude: ranking.claude,
            skipped: ranking.skipped,
            swap,
            usage,
            compressor_bytes,
            swap_window,
            interval,
            others: u32::try_from(total_rows - rows.len()).unwrap_or(u32::MAX),
            rows: rows.into_boxed_slice(),
        }
    }

    /// Removes the oldest entry, and lets go of its identities.
    fn evict_oldest(&mut self) {
        match self.entries.pop_front() {
            Some(Entry::Frame(frame)) => {
                self.stored_rows -= frame.rows.len();
                for row in frame.rows.iter() {
                    self.identities.release(row.identity);
                }
            }
            Some(Entry::Gap(gap)) => {
                self.gap_text_bytes -= gap.reason.capacity();
            }
            None => {}
        }
    }

    /// Gives the place of `id` in the entries.
    fn position(&self, id: EntryId) -> Option<usize> {
        let oldest = self.next_id - self.entries.len() as u64;
        let offset = id.0.checked_sub(oldest)?;
        let position = usize::try_from(offset).ok()?;
        (position < self.entries.len()).then_some(position)
    }

    /// Tells whether the entry `id` is still in the history.
    #[must_use]
    pub fn contains(&self, id: EntryId) -> bool {
        self.position(id).is_some()
    }

    /// Gives the name of the oldest entry.
    #[must_use]
    pub fn oldest(&self) -> Option<EntryId> {
        (!self.entries.is_empty()).then(|| self.id_at(0))
    }

    /// Gives the name of the newest entry.
    #[must_use]
    pub fn newest(&self) -> Option<EntryId> {
        self.entries
            .len()
            .checked_sub(1)
            .map(|last| self.id_at(last))
    }

    /// Gives the name of the newest frame.
    #[must_use]
    pub fn newest_frame(&self) -> Option<EntryId> {
        let position = self
            .entries
            .iter()
            .rposition(|entry| matches!(entry, Entry::Frame(_)))?;
        Some(self.id_at(position))
    }

    /// Gives the gap `id`, or `None` for a frame or an unknown name.
    #[must_use]
    pub fn gap(&self, id: EntryId) -> Option<&Gap> {
        match &self.entries[self.position(id)?] {
            Entry::Gap(gap) => Some(gap),
            Entry::Frame(_) => None,
        }
    }

    /// Gives the time of the entry `id`.
    #[must_use]
    pub fn at(&self, id: EntryId) -> Option<SystemTime> {
        Some(self.entry_at(self.position(id)?))
    }

    /// Gives the counts and the times of the whole history.
    #[must_use]
    pub fn extent(&self) -> Extent {
        let mut extent = Extent {
            oldest_at: (!self.entries.is_empty()).then(|| self.entry_at(0)),
            newest_at: self
                .entries
                .len()
                .checked_sub(1)
                .map(|last| self.entry_at(last)),
            ..Extent::default()
        };
        for entry in &self.entries {
            match entry {
                Entry::Frame(frame) => {
                    extent.frames += 1;
                    extent.measured = extent.measured.saturating_add(frame.window);
                }
                Entry::Gap(_) => extent.gaps += 1,
            }
        }
        extent
    }

    /// Gives the name of the entry at `position`.
    fn id_at(&self, position: usize) -> EntryId {
        EntryId(self.next_id - self.entries.len() as u64 + position as u64)
    }

    /// Gives the name of the entry before `id`.
    #[must_use]
    pub fn previous(&self, id: EntryId) -> Option<EntryId> {
        let position = self.position(id)?;
        position.checked_sub(1).map(|earlier| self.id_at(earlier))
    }

    /// Gives the name of the entry after `id`.
    #[must_use]
    pub fn next(&self, id: EntryId) -> Option<EntryId> {
        let position = self.position(id)? + 1;
        (position < self.entries.len()).then(|| self.id_at(position))
    }

    /// Gives the newest entry before `id` that is at least `by` older than it,
    /// or `None` when no entry is old enough.
    #[must_use]
    pub fn back_by(&self, id: EntryId, by: Duration) -> Option<EntryId> {
        let position = self.position(id)?;
        let limit = self.entry_at(position);
        (0..position)
            .rev()
            .find(|earlier| {
                self.entry_at(*earlier)
                    .checked_add(by)
                    .is_some_and(|reach| reach <= limit)
            })
            .map(|earlier| self.id_at(earlier))
    }

    /// Gives the oldest entry after `id` that is at least `by` newer than it,
    /// or `None` when no entry is new enough.
    #[must_use]
    pub fn forward_by(&self, id: EntryId, by: Duration) -> Option<EntryId> {
        let position = self.position(id)?;
        let limit = self.entry_at(position).checked_add(by)?;
        (position + 1..self.entries.len())
            .find(|later| self.entry_at(*later) >= limit)
            .map(|later| self.id_at(later))
    }

    /// Tells whether a push would double the capacity of the deque of the
    /// entries past its share of the budget. See [`ENTRY_SHARE_DIVISOR`].
    fn a_push_passes_the_entry_share(&self) -> bool {
        let capacity = self.entries.capacity();
        let doubled = capacity.saturating_mul(2 * size_of::<Entry>());
        self.entries.len() == capacity && doubled > self.budget / ENTRY_SHARE_DIVISOR
    }

    /// Gives the bytes that the history holds. It counts the real sizes: the
    /// entries and the rows, the text of the identities and of the gaps, and
    /// each container by its capacity.
    #[must_use]
    pub fn bytes_used(&self) -> usize {
        self.entries.capacity() * size_of::<Entry>()
            + self.stored_rows * size_of::<CompactRow>()
            + self.gap_text_bytes
            + self.identities.bytes()
    }

    /// Gives the time of the entry at `position`.
    fn entry_at(&self, position: usize) -> SystemTime {
        match &self.entries[position] {
            Entry::Frame(frame) => frame.at,
            Entry::Gap(gap) => gap.at,
        }
    }

    /// Makes the row of `slot` as the history stored it, with `faults`.
    fn decode(&self, slot: u32, faults: u64, rss_kib: u32, state: CompactState) -> RankedRow {
        let identity = self.identities.slab[slot as usize]
            .as_ref()
            .expect("a stored row points at a live identity");
        let detail = identity.session.as_ref();
        let session = |state: SessionState| match detail {
            None => ClaudeView::NoRecord,
            Some(detail) => ClaudeView::Session {
                id: detail.id.clone(),
                state,
                directory: detail.directory.as_ref().map(PathBuf::from),
                status_changed_at: None,
            },
        };
        let claude = match state {
            CompactState::NotClaude => ClaudeView::NotClaude,
            CompactState::NoRecord => ClaudeView::NoRecord,
            CompactState::OtherAccount => ClaudeView::OtherAccount,
            CompactState::Busy => session(SessionState::Busy),
            CompactState::Waiting => session(SessionState::Waiting),
            CompactState::Idle(seconds) => session(SessionState::Idle {
                for_: seconds.map(|seconds| Duration::from_secs(u64::from(seconds))),
            }),
            CompactState::Other => session(SessionState::Other(
                detail
                    .and_then(|detail| detail.other_status.clone())
                    .unwrap_or_default(),
            )),
            CompactState::Unknown => session(SessionState::Unknown),
        };
        RankedRow {
            pid: identity.pid,
            uid: identity.uid,
            faults,
            rss_kib: (rss_kib != NO_RSS).then_some(u64::from(rss_kib)),
            started_at_epoch_secs: identity.started,
            command: identity.command.clone(),
            claude,
        }
    }

    /// Gives the reading of the span of `width` that ends at `end`.
    ///
    /// The span holds the entries with `at + width > end.at`, and the whole
    /// history up to `end` for [`SpanWidth::Whole`]. The faults of a process
    /// are the sum over the frames, and the window is the sum of the windows.
    /// A gap adds to neither sum. A span with no frame gives `None`.
    ///
    /// The processes that exited are a sum over the frames, because each one
    /// exited in one window. The zombies and the unsampled processes both
    /// come from the newest frame of the span, because they are snapshots.
    #[must_use]
    pub fn span(&self, end: EntryId, width: SpanWidth) -> Option<Reading> {
        if width == SpanWidth::OneFrame {
            return self.frame(end);
        }
        let end_position = self.position(end)?;
        let end_at = self.entry_at(end_position);
        let mut start = end_position;
        while start > 0 {
            let earlier = start - 1;
            if let Some(reach) = width.reach() {
                let ends = self.entry_at(earlier).checked_add(reach);
                if ends.is_some_and(|ends| ends <= end_at) {
                    break;
                }
            }
            start = earlier;
        }
        let mut frames: Vec<&Frame> = Vec::new();
        let mut gaps = 0;
        for entry in self.entries.range(start..=end_position).rev() {
            match entry {
                Entry::Frame(frame) => frames.push(frame),
                Entry::Gap(_) => gaps += 1,
            }
        }
        let newest = *frames.first()?;
        // The sums use a vector indexed by the slab. A whole-history span
        // visits about three million rows, and a hash map costs more there.
        let mut faults = vec![0_u64; self.identities.slab.len()];
        let mut latest: Vec<Option<Newest>> = vec![None; self.identities.slab.len()];
        let mut touched: Vec<u32> = Vec::new();
        for (place, frame) in frames.iter().enumerate() {
            for row in frame.rows.iter() {
                let slot = row.identity as usize;
                faults[slot] = faults[slot].saturating_add(row.faults);
                if latest[slot].is_none() {
                    latest[slot] = Some(Newest {
                        frame: place,
                        rss_kib: row.rss_kib,
                        state: row.state,
                    });
                    touched.push(row.identity);
                }
            }
        }
        let mut ranked: Vec<(RankedRow, RowExtra)> = touched
            .iter()
            .filter_map(|&slot| {
                let seen = latest[slot as usize]?;
                let row = self.decode(slot, faults[slot as usize], seen.rss_kib, seen.state);
                let own = self.identities.slab[slot as usize]
                    .as_ref()
                    .is_some_and(|identity| identity.own);
                let extra = RowExtra {
                    as_of: frames[seen.frame].at,
                    own,
                };
                Some((row, extra))
            })
            .collect();
        ranked.sort_by(|(left, _), (right, _)| {
            right
                .faults
                .cmp(&left.faults)
                .then(left.pid.cmp(&right.pid))
                .then(left.started_at_epoch_secs.cmp(&right.started_at_epoch_secs))
        });
        let claude = ClaudeTotal {
            processes: touched
                .iter()
                .filter(|&&slot| {
                    latest[slot as usize].is_some_and(|seen| seen.state != CompactState::NotClaude)
                })
                .count(),
            other_account: touched
                .iter()
                .filter(|&&slot| {
                    latest[slot as usize]
                        .is_some_and(|seen| seen.state == CompactState::OtherAccount)
                })
                .count(),
            faults: frames
                .iter()
                .map(|frame| frame.claude.faults)
                .fold(0, u64::saturating_add),
        };
        let skipped = Skipped {
            exited: frames
                .iter()
                .map(|frame| frame.skipped.exited)
                .fold(0, usize::saturating_add),
            exited_faults: frames
                .iter()
                .map(|frame| frame.skipped.exited_faults)
                .fold(0, u64::saturating_add),
            // The zombies and the unsampled processes are snapshots of the
            // table of `ps`, not events. A sum counts one zombie once for each
            // frame that saw it, so the span states the newest frame, as the
            // one-frame view does.
            zombies: newest.skipped.zombies,
            unsampled: newest.skipped.unsampled,
        };
        let (rows, extras): (Vec<RankedRow>, Vec<RowExtra>) = ranked.into_iter().unzip();
        let processes = rows.len();
        Some(Reading {
            end,
            at: end_at,
            ranking: Ranking {
                rows,
                window: frames
                    .iter()
                    .map(|frame| frame.window)
                    .fold(Duration::ZERO, Duration::saturating_add),
                total_faults: frames
                    .iter()
                    .map(|frame| frame.total_faults)
                    .fold(0, u64::saturating_add),
                claude,
                skipped,
            },
            extras,
            others: 0,
            header: HeaderFacts {
                processes,
                swap: VmDelta {
                    swapins: frames
                        .iter()
                        .map(|frame| frame.swap.swapins)
                        .fold(0, u64::saturating_add),
                    swapouts: frames
                        .iter()
                        .map(|frame| frame.swap.swapouts)
                        .fold(0, u64::saturating_add),
                },
                usage: newest.usage,
                compressor_bytes: newest.compressor_bytes,
                swap_window: frames
                    .iter()
                    .map(|frame| frame.swap_window)
                    .fold(Duration::ZERO, Duration::saturating_add),
                interval: newest.interval,
                span: Some(SpanFacts {
                    label: width.label(),
                    frames: frames.len(),
                    gaps,
                }),
            },
        })
    }

    /// Gives the timeline of `cells` cells, with the marker at `selected`.
    ///
    /// `E` entries map onto `n = min(cells, E)` cells. Cell `i` holds the
    /// entries from `floor(i * E / n)` up to, and not including,
    /// `floor((i + 1) * E / n)`. A cell with a frame shows the highest rate of
    /// its slice, so a spike survives the squeeze.
    #[must_use]
    pub fn timeline(&self, cells: usize, selected: Option<EntryId>) -> Timeline {
        let fault_rates: Vec<Sample> = self
            .entries
            .iter()
            .map(|entry| match entry {
                Entry::Frame(frame) => {
                    Sample::Frame(Some(per_second(frame.total_faults, frame.window)))
                }
                Entry::Gap(_) => Sample::Gap,
            })
            .collect();
        let swap_rates: Vec<Sample> = self
            .entries
            .iter()
            .map(|entry| match entry {
                Entry::Frame(frame) => Sample::Frame(Some(per_second(
                    frame.swap.swapins.saturating_add(frame.swap.swapouts),
                    frame.swap_window,
                ))),
                Entry::Gap(_) => Sample::Gap,
            })
            .collect();
        Timeline {
            faults: cells_of(&fault_rates, cells, peak_of(&fault_rates)),
            swap: cells_of(&swap_rates, cells, peak_of(&swap_rates)),
            marker: self.marker_cell(cells, selected),
        }
    }

    /// Gives the series of the process `key` in `cells` cells, with the marker
    /// at `selected`, or `None` for a process that the history does not hold.
    #[must_use]
    pub fn series(
        &self,
        key: ProcessKey,
        cells: usize,
        selected: Option<EntryId>,
    ) -> Option<Series> {
        let slot = *self.identities.index.get(&key)?;
        let identity = self.identities.slab[slot as usize].as_ref()?;
        let mut first_at = None;
        let mut last_at = None;
        let mut frames_with_faults = 0;
        let samples: Vec<Sample> = self
            .entries
            .iter()
            .map(|entry| match entry {
                Entry::Gap(_) => Sample::Gap,
                Entry::Frame(frame) => {
                    let found = frame.rows.iter().find(|row| row.identity == slot);
                    Sample::Frame(found.map(|row| {
                        frames_with_faults += 1;
                        first_at.get_or_insert(frame.at);
                        last_at = Some(frame.at);
                        per_second(row.faults, frame.window)
                    }))
                }
            })
            .collect();
        let peak_rate = peak_of(&samples);
        Some(Series {
            key,
            uid: identity.uid,
            command: identity.command.clone(),
            own: identity.own,
            session: identity
                .session
                .as_ref()
                .map(|detail| detail.id.as_str().to_string()),
            directory: identity
                .session
                .as_ref()
                .and_then(|detail| detail.directory.clone()),
            cells: cells_of(&samples, cells, peak_rate),
            marker: self.marker_cell(cells, selected),
            peak_rate,
            first_at,
            last_at,
            frames_with_faults,
        })
    }

    /// Gives the cell of the selected entry in a timeline of `cells` cells.
    fn marker_cell(&self, cells: usize, selected: Option<EntryId>) -> Option<usize> {
        let position = self.position(selected?)?;
        let entries = self.entries.len();
        let used = cells.min(entries);
        (used > 0).then(|| ((position + 1) * used).div_ceil(entries) - 1)
    }

    /// Gives the reading of the one frame `id`, or `None` for a gap or an
    /// unknown name.
    ///
    /// The rows are the rows with at least one fault, in the order of the
    /// ranking. The count of the other processes stays in the reading, so the
    /// header can state the full count.
    #[must_use]
    pub fn frame(&self, id: EntryId) -> Option<Reading> {
        let Entry::Frame(frame) = &self.entries[self.position(id)?] else {
            return None;
        };
        let (rows, extras): (Vec<RankedRow>, Vec<RowExtra>) = frame
            .rows
            .iter()
            .map(|row| {
                let ranked = self.decode(row.identity, row.faults, row.rss_kib, row.state);
                let own = self.identities.slab[row.identity as usize]
                    .as_ref()
                    .is_some_and(|identity| identity.own);
                (
                    ranked,
                    RowExtra {
                        as_of: frame.at,
                        own,
                    },
                )
            })
            .unzip();
        let others = frame.others as usize;
        let processes = rows.len() + others;
        Some(Reading {
            end: id,
            at: frame.at,
            ranking: Ranking {
                rows,
                window: frame.window,
                total_faults: frame.total_faults,
                claude: frame.claude,
                skipped: frame.skipped,
            },
            extras,
            others,
            header: HeaderFacts {
                processes,
                swap: frame.swap,
                usage: frame.usage,
                compressor_bytes: frame.compressor_bytes,
                swap_window: frame.swap_window,
                interval: frame.interval,
                span: None,
            },
        })
    }

    /// Gives the names of the accounts that the frames mentioned.
    #[must_use]
    pub fn accounts(&self) -> &Accounts {
        &self.accounts
    }
}

/// Gives the compact state of a view.
fn compact_state(view: &ClaudeView) -> CompactState {
    match view {
        ClaudeView::NotClaude => CompactState::NotClaude,
        ClaudeView::NoRecord => CompactState::NoRecord,
        ClaudeView::OtherAccount => CompactState::OtherAccount,
        ClaudeView::Session { state, .. } => match state {
            SessionState::Busy => CompactState::Busy,
            SessionState::Waiting => CompactState::Waiting,
            SessionState::Idle { for_ } => CompactState::Idle(
                for_.map(|time| u32::try_from(time.as_secs()).unwrap_or(u32::MAX)),
            ),
            SessionState::Other(_) => CompactState::Other,
            SessionState::Unknown => CompactState::Unknown,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window of a test frame, in seconds.
    const WINDOW_SECS: u64 = 2;

    /// The id of the session of the test rows.
    const SESSION: &str = "d3b0d921-f0a1-41fc-b309-c11aa30c1173";

    /// The account of every test row.
    const ACCOUNT: Uid = Uid::new(501);

    /// Gives the time `secs` seconds after the Unix epoch.
    fn time(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// Makes a row of a process that is not a session.
    fn row(pid: u32, faults: u64) -> RankedRow {
        RankedRow {
            pid: Pid::new(pid),
            uid: ACCOUNT,
            faults,
            rss_kib: Some(1_000),
            started_at_epoch_secs: Some(1_700_000_000),
            command: format!("/bin/command-{pid}"),
            claude: ClaudeView::NotClaude,
        }
    }

    /// Makes the sample that holds `rows`, taken at `secs`.
    fn sample(secs: u64, rows: Vec<RankedRow>) -> SampledFrame {
        let window = Duration::from_secs(WINDOW_SECS);
        let claude = ClaudeTotal {
            processes: rows
                .iter()
                .filter(|row| row.claude != ClaudeView::NotClaude)
                .count(),
            other_account: rows
                .iter()
                .filter(|row| row.claude == ClaudeView::OtherAccount)
                .count(),
            faults: rows
                .iter()
                .filter(|row| row.claude != ClaudeView::NotClaude)
                .map(|row| row.faults)
                .sum(),
        };
        let total_faults = rows.iter().map(|row| row.faults).sum();
        SampledFrame {
            observed: Observed {
                ranking: crate::ranking::Ranking {
                    rows,
                    window,
                    total_faults,
                    claude,
                    skipped: Skipped::default(),
                },
                table: Vec::new(),
                swap: VmDelta::default(),
                usage: SwapUsage::default(),
                compressor_bytes: 0,
                swap_window: window,
                accounts: Accounts::new(),
                now: time(secs),
                interval: "2s".parse().expect("2s is a span"),
            },
            at: time(secs),
            own: BTreeSet::new(),
        }
    }

    /// Makes the update that holds one sample.
    fn frame(secs: u64, rows: Vec<RankedRow>) -> Update {
        Update::Frame(Box::new(sample(secs, rows)))
    }

    /// Makes the update that holds one gap.
    fn gap(secs: u64, lost_secs: u64) -> Update {
        Update::Gap(Gap {
            at: time(secs),
            lost: Duration::from_secs(lost_secs),
            reason: "top stopped".to_string(),
        })
    }

    /// The oldest entry leaves first, and `contains` says which ones are left.
    /// The time of each kept name is the time of its own frame, so an
    /// eviction of the newest entry fails here too.
    #[test]
    fn the_oldest_entry_leaves_first() {
        let mut history = History::with_budget(8_000);
        let ids: Vec<EntryId> = (0..100)
            .map(|index| history.add(frame(index * 2, vec![row(1, 5), row(2, 3)])))
            .collect();

        let kept: Vec<bool> = ids.iter().map(|id| history.contains(*id)).collect();
        let first_kept = kept
            .iter()
            .position(|kept| *kept)
            .expect("the newest stays");

        assert!(
            first_kept > 0,
            "a budget of 8,000 bytes cannot hold 100 frames"
        );
        assert!(
            kept[first_kept..].iter().all(|kept| *kept),
            "the kept ids are a suffix"
        );
        assert!(history.contains(ids[99]));
        assert_eq!(history.oldest(), Some(ids[first_kept]));
        let first_kept_secs = u64::try_from(first_kept).expect("an index fits in a u64") * 2;
        assert_eq!(history.at(ids[first_kept]), Some(time(first_kept_secs)));
        assert_eq!(history.at(ids[99]), Some(time(198)));
    }

    /// A PID that returns with a new start time is a new process: the span
    /// holds two rows and not one.
    #[test]
    fn a_reused_pid_is_two_processes() {
        let mut first = row(7, 4);
        first.started_at_epoch_secs = Some(100);
        let mut second = row(7, 6);
        second.started_at_epoch_secs = Some(200);
        let mut history = History::new();
        history.add(frame(10, vec![first]));
        let end = history.add(frame(12, vec![second]));

        let reading = history
            .span(end, SpanWidth::Whole)
            .expect("the span has frames");
        let rows: Vec<(u32, Option<u64>, u64)> = reading
            .rows()
            .map(|view| {
                (
                    view.key.pid.get(),
                    view.key.started_at_epoch_secs,
                    view.row.faults,
                )
            })
            .collect();

        assert_eq!(rows, vec![(7, Some(200), 6), (7, Some(100), 4)]);
    }

    /// A gap adds to neither the faults nor the window of a span, so the rate
    /// of a process is the same with the gap and without it.
    #[test]
    fn a_gap_changes_no_rate() {
        let mut history = History::new();
        history.add(frame(10, vec![row(1, 20)]));
        history.add(gap(12, 2));
        let end = history.add(frame(14, vec![row(1, 20)]));

        let reading = history
            .span(end, SpanWidth::Whole)
            .expect("the span has frames");
        let ranking = reading.ranking();

        assert_eq!(ranking.window, Duration::from_secs(2 * WINDOW_SECS));
        assert_eq!(ranking.rows[0].faults, 40);
        assert!((ranking.faults_per_second(ranking.rows[0].faults) - 10.0).abs() < f64::EPSILON);
        assert!(reading.header_lines()[0].contains("(2 frames, 1 gap)"));
    }

    /// A process that exited keeps its row in a span, with the faults that it
    /// made and the time of the frame that last saw it.
    #[test]
    fn an_exited_process_keeps_its_row_in_a_span() {
        let mut history = History::new();
        history.add(frame(10, vec![row(1, 9), row(2, 3)]));
        let end = history.add(frame(12, vec![row(2, 4)]));

        let reading = history
            .span(end, SpanWidth::Whole)
            .expect("the span has frames");
        let rows: Vec<(u32, u64, SystemTime)> = reading
            .rows()
            .map(|view| (view.row.pid.get(), view.row.faults, view.as_of))
            .collect();

        assert_eq!(rows, vec![(1, 9, time(10)), (2, 7, time(12))]);
    }

    /// A spike stays visible when the history holds more frames than cells: a
    /// cell shows the highest rate of its slice, not the mean.
    #[test]
    fn a_spike_stays_visible_with_more_frames_than_cells() {
        let mut history = History::new();
        for index in 0..100 {
            let faults = if index == 37 { 1_000 } else { 2 };
            history.add(frame(index * WINDOW_SECS, vec![row(1, faults)]));
        }

        let timeline = history.timeline(10, None);
        let levels: Vec<Option<u8>> = timeline
            .faults
            .iter()
            .map(|cell| match cell {
                Cell::Bar { level, .. } => Some(*level),
                _ => None,
            })
            .collect();

        assert_eq!(levels.len(), 10);
        assert_eq!(levels.iter().filter(|level| **level == Some(7)).count(), 1);
        assert_eq!(levels[3], Some(7));
    }

    /// A frame reading holds the rows with a fault, counts the others, and
    /// states the full count of processes in its header.
    #[test]
    fn a_frame_reading_keeps_only_rows_with_faults() {
        let mut rows = vec![row(30, 5), row(10, 2)];
        rows.extend((100..108).map(|pid| row(pid, 0)));
        let mut history = History::new();
        let id = history.add(frame(10, rows));
        let gap_id = history.add(gap(12, 2));

        let reading = history.frame(id).expect("the frame is there");
        let pids: Vec<u32> = reading.rows().map(|view| view.row.pid.get()).collect();

        assert_eq!(pids, vec![30, 10]);
        assert_eq!(reading.other_rows(), 8);
        assert_eq!(reading.end(), id);
        assert_eq!(reading.at(), time(10));
        assert_eq!(reading.ranking().total_faults, 7);
        assert!(reading.header_lines()[0].starts_with("10 processes over"));
        assert!(history.frame(gap_id).is_none());
        assert!(history.span(gap_id, SpanWidth::OneFrame).is_none());
    }

    /// Makes the view of a session in `state`, with an optional directory.
    fn session(state: SessionState, directory: Option<&str>) -> ClaudeView {
        ClaudeView::Session {
            id: SessionId::parse(SESSION).expect("a valid id"),
            state,
            directory: directory.map(PathBuf::from),
            status_changed_at: None,
        }
    }

    /// Makes a row with the view `claude`.
    fn claude_row(pid: u32, faults: u64, claude: ClaudeView) -> RankedRow {
        RankedRow {
            claude,
            ..row(pid, faults)
        }
    }

    /// Every state comes back as the frame saw it: the seconds of an idle
    /// session, and the text of a status that `occ` does not know.
    #[test]
    fn the_state_of_a_session_comes_back_as_of_the_frame() {
        let views = vec![
            ClaudeView::NotClaude,
            ClaudeView::NoRecord,
            ClaudeView::OtherAccount,
            session(SessionState::Busy, Some("/work/a")),
            session(SessionState::Waiting, Some("/work/a")),
            session(
                SessionState::Idle {
                    for_: Some(Duration::from_secs(90)),
                },
                Some("/work/a"),
            ),
            session(SessionState::Idle { for_: None }, Some("/work/a")),
            session(SessionState::Other("shell".to_string()), Some("/work/a")),
            session(SessionState::Unknown, None),
        ];
        let rows: Vec<RankedRow> = views
            .iter()
            .enumerate()
            .map(|(index, view)| {
                claude_row(
                    u32::try_from(index).unwrap() + 1,
                    100 - index as u64,
                    view.clone(),
                )
            })
            .collect();
        let mut history = History::new();
        let id = history.add(frame(10, rows));

        let reading = history.frame(id).expect("the frame is there");
        let back: Vec<ClaudeView> = reading.rows().map(|view| view.row.claude.clone()).collect();

        assert_eq!(back, views);
    }

    /// A later frame gives the newest directory to the process. A frame with
    /// no directory does not erase the one that is known. The frame with no
    /// directory comes last, so nothing after it can put a directory back.
    #[test]
    fn an_absent_detail_never_erases_a_known_one() {
        let mut history = History::new();
        let first = history.add(frame(
            10,
            vec![claude_row(
                1,
                5,
                session(SessionState::Busy, Some("/work/a")),
            )],
        ));
        history.add(frame(
            12,
            vec![claude_row(
                1,
                5,
                session(SessionState::Busy, Some("/work/b")),
            )],
        ));
        history.add(frame(
            14,
            vec![claude_row(1, 5, session(SessionState::Busy, None))],
        ));

        let reading = history.frame(first).expect("the frame is there");

        assert_eq!(
            reading.row(0).unwrap().row.claude,
            session(SessionState::Busy, Some("/work/b"))
        );
    }

    /// A span sums the faults and the windows, counts each Claude Code process
    /// once, orders by faults and then by PID, and takes the state and the
    /// resident memory from the newest frame of each process.
    #[test]
    fn a_span_sums_orders_and_takes_the_newest_state() {
        let busy = session(SessionState::Busy, Some("/work/a"));
        let idle = session(
            SessionState::Idle {
                for_: Some(Duration::from_secs(30)),
            },
            None,
        );
        let mut newest_row = claude_row(1, 4, idle);
        newest_row.rss_kib = Some(7_000);
        let mut updates = Vec::new();
        for (secs, rows) in [
            (10, vec![claude_row(1, 4, busy.clone()), row(3, 6)]),
            (12, vec![row(2, 10), claude_row(1, 4, busy)]),
            (14, vec![newest_row, row(3, 1)]),
        ] {
            let mut sampled = sample(secs, rows);
            sampled.observed.ranking.skipped.exited = 1;
            sampled.observed.ranking.skipped.exited_faults = 5;
            updates.push(Update::Frame(Box::new(sampled)));
        }
        let mut history = History::new();
        let ids: Vec<EntryId> = updates
            .into_iter()
            .map(|update| history.add(update))
            .collect();

        let reading = history
            .span(ids[2], SpanWidth::Whole)
            .expect("the span has frames");
        let rows: Vec<(u32, u64)> = reading
            .rows()
            .map(|view| (view.row.pid.get(), view.row.faults))
            .collect();
        let ranking = reading.ranking();
        let first = reading
            .row(
                reading
                    .position_of(ProcessKey {
                        pid: Pid::new(1),
                        started_at_epoch_secs: Some(1_700_000_000),
                    })
                    .unwrap(),
            )
            .unwrap();

        assert_eq!(rows, vec![(1, 12), (2, 10), (3, 7)]);
        assert_eq!(ranking.window, Duration::from_secs(3 * WINDOW_SECS));
        assert_eq!(ranking.total_faults, 29);
        assert_eq!(ranking.claude.processes, 1);
        assert_eq!(ranking.claude.faults, 12);
        assert_eq!(ranking.skipped.exited, 3);
        assert_eq!(ranking.skipped.exited_faults, 15);
        assert_eq!(
            first.row.claude,
            session(
                SessionState::Idle {
                    for_: Some(Duration::from_secs(30))
                },
                Some("/work/a")
            )
        );
        assert_eq!(first.row.rss_kib, Some(7_000));
        assert_eq!(first.as_of, time(14));
        assert!(reading.header_lines()[0].contains("of the whole history (3 frames, 0 gaps)"));
    }

    /// A span of 1 m or more takes the zombies and the unsampled processes
    /// from its newest frame, the same as the one-frame view of that frame.
    /// The two counts are snapshots of the table of `ps`, so a sum over the
    /// frames counts one zombie once for each frame that saw it.
    #[test]
    fn a_span_states_the_zombies_and_the_unsampled_of_its_newest_frame() {
        let mut updates = Vec::new();
        for (secs, zombies, unsampled) in [(10, 9, 4), (12, 0, 0), (14, 72, 5)] {
            let mut sampled = sample(secs, vec![row(1, 4)]);
            sampled.observed.ranking.skipped.zombies = zombies;
            sampled.observed.ranking.skipped.unsampled = unsampled;
            updates.push(Update::Frame(Box::new(sampled)));
        }
        let mut history = History::new();
        let ids: Vec<EntryId> = updates
            .into_iter()
            .map(|update| history.add(update))
            .collect();
        let one_frame = history.frame(ids[2]).expect("the frame is there");
        let expected = "skipped: 72 zombies, 5 were not in the top sample";

        assert_eq!(
            one_frame.header_lines().last().map(String::as_str),
            Some(expected)
        );
        for width in [
            SpanWidth::OneMinute,
            SpanWidth::FiveMinutes,
            SpanWidth::FifteenMinutes,
            SpanWidth::Whole,
        ] {
            let reading = history.span(ids[2], width).expect("the span has frames");

            assert_eq!(
                reading.header_lines().last().map(String::as_str),
                Some(expected),
                "the span {}",
                width.label()
            );
            assert_eq!(reading.ranking().skipped, one_frame.ranking().skipped);
        }
    }

    /// A narrow span walks back only over the entries that are young enough,
    /// and a span can end at a gap.
    #[test]
    fn a_narrow_span_leaves_out_old_entries() {
        let mut history = History::new();
        history.add(frame(0, vec![row(1, 100)]));
        history.add(frame(100, vec![row(1, 3)]));
        history.add(frame(130, vec![row(1, 4)]));
        let gap_id = history.add(gap(150, 2));

        let minute = history
            .span(gap_id, SpanWidth::OneMinute)
            .expect("frames in range");
        let all = history
            .span(gap_id, SpanWidth::FifteenMinutes)
            .expect("frames in range");

        assert_eq!(minute.ranking().rows[0].faults, 7);
        assert_eq!(all.ranking().rows[0].faults, 107);
        assert_eq!(minute.end(), gap_id);
        assert!(history.span(gap_id, SpanWidth::OneFrame).is_none());
    }

    /// Builds a history of seven entries: five frames and two gaps. The swap
    /// traffic is 10 pages in frame 0 and 80 pages in frame 3.
    fn seven_entries() -> (History, Vec<EntryId>) {
        let mut history = History::new();
        let mut ids = Vec::new();
        for (index, faults) in [
            (0, Some(2)),
            (1, Some(4)),
            (2, None),
            (3, Some(16)),
            (4, None),
            (5, None),
            (6, Some(8)),
        ] {
            let secs = index * WINDOW_SECS;
            let id = match faults {
                None => history.add(gap(secs, WINDOW_SECS)),
                Some(faults) => {
                    let mut sampled = sample(secs, vec![row(1, faults)]);
                    sampled.observed.swap = match index {
                        0 => VmDelta {
                            swapins: 10,
                            swapouts: 0,
                        },
                        3 => VmDelta {
                            swapins: 40,
                            swapouts: 40,
                        },
                        _ => VmDelta::default(),
                    };
                    history.add(Update::Frame(Box::new(sampled)))
                }
            };
            ids.push(id);
        }
        (history, ids)
    }

    /// Makes the cell of a bar.
    fn bar(level: u8, holds_gap: bool) -> Cell {
        Cell::Bar { level, holds_gap }
    }

    /// Seven entries squeezed into three cells: each cell shows the highest
    /// rate of its slice, and the marker is the cell of the selected entry.
    #[test]
    fn the_timeline_maps_entries_to_cells_by_integer_slices() {
        let (history, ids) = seven_entries();

        let timeline = history.timeline(3, Some(ids[3]));

        assert_eq!(
            timeline.faults,
            vec![bar(2, false), bar(7, true), bar(4, true)]
        );
        assert_eq!(timeline.marker, Some(1));
        assert_eq!(history.timeline(3, Some(ids[6])).marker, Some(2));
        assert_eq!(history.timeline(3, Some(ids[0])).marker, Some(0));
        assert_eq!(history.timeline(3, None).marker, None);
    }

    /// With a cell for each entry, a gap is a gap cell, and the swap row
    /// scales to its own peak.
    #[test]
    fn the_timeline_shows_gaps_and_the_swap_row() {
        let (history, _) = seven_entries();

        let timeline = history.timeline(50, None);

        assert_eq!(
            timeline.faults,
            vec![
                bar(1, false),
                bar(2, false),
                Cell::Gap,
                bar(7, false),
                Cell::Gap,
                Cell::Gap,
                bar(4, false)
            ]
        );
        assert_eq!(
            timeline.swap,
            vec![
                bar(1, false),
                bar(0, false),
                Cell::Gap,
                bar(7, false),
                Cell::Gap,
                Cell::Gap,
                bar(0, false)
            ]
        );
    }

    /// A history with no entry has no cell and no marker.
    #[test]
    fn an_empty_history_has_an_empty_timeline() {
        let history = History::new();

        let timeline = history.timeline(10, None);

        assert!(timeline.faults.is_empty() && timeline.swap.is_empty());
        assert_eq!(timeline.marker, None);
    }

    /// A terminal can be too narrow for one cell of the timeline. A timeline of
    /// no cells then has no place for the marker.
    #[test]
    fn a_timeline_of_no_cells_has_no_marker() {
        let mut history = History::new();
        let id = history.add(frame(10, vec![row(1, 5)]));

        let timeline = history.timeline(0, Some(id));

        assert!(timeline.faults.is_empty() && timeline.swap.is_empty());
        assert_eq!(timeline.marker, None);
    }

    /// The series of one process: blank cells before it started, a level
    /// against its own peak, and the full command.
    #[test]
    fn a_series_is_blank_before_the_process_started() {
        let command = format!("/bin/long {}", "x".repeat(5_000));
        let mut late = row(9, 10);
        late.command = command.clone();
        let mut history = History::new();
        let mut ids = Vec::new();
        for index in 0..6_u64 {
            let mut rows = vec![row(1, 2)];
            match index {
                3 => rows.push(late.clone()),
                4 => rows.push(RankedRow {
                    faults: 30,
                    ..late.clone()
                }),
                _ => {}
            }
            ids.push(history.add(frame(index * WINDOW_SECS, rows)));
        }
        let key = ProcessKey {
            pid: Pid::new(9),
            started_at_epoch_secs: Some(1_700_000_000),
        };

        let series = history
            .series(key, 6, Some(ids[4]))
            .expect("the process is known");

        assert_eq!(
            series.cells,
            vec![
                Cell::Blank,
                Cell::Blank,
                Cell::Blank,
                bar(2, false),
                bar(7, false),
                Cell::Blank,
            ]
        );
        assert_eq!(series.marker, Some(4));
        assert!((series.peak_rate - 15.0).abs() < f64::EPSILON);
        assert_eq!(series.command, command);
        assert_eq!(series.uid, ACCOUNT);
        assert_eq!(series.frames_with_faults, 2);
        assert_eq!(series.first_at, Some(time(6)));
        assert_eq!(series.last_at, Some(time(8)));
        assert!(!series.own);
        assert_eq!(series.session, None);
        let unknown = ProcessKey {
            pid: Pid::new(77),
            ..key
        };
        assert!(history.series(unknown, 6, None).is_none());
    }

    /// Builds five entries, two seconds apart, with a gap at the third.
    fn five_entries() -> (History, Vec<EntryId>) {
        let mut history = History::new();
        let ids = vec![
            history.add(frame(0, vec![row(1, 1)])),
            history.add(frame(2, vec![row(1, 1)])),
            history.add(gap(4, 2)),
            history.add(frame(6, vec![row(1, 1)])),
            history.add(frame(8, vec![row(1, 1)])),
        ];
        (history, ids)
    }

    /// `previous` and `next` step one entry, a gap included, and stop at the
    /// ends. An unknown name has no neighbor.
    #[test]
    fn previous_and_next_step_one_entry() {
        let (history, ids) = five_entries();

        assert_eq!(history.oldest(), Some(ids[0]));
        assert_eq!(history.newest(), Some(ids[4]));
        assert_eq!(history.previous(ids[0]), None);
        assert_eq!(history.previous(ids[3]), Some(ids[2]));
        assert_eq!(history.next(ids[2]), Some(ids[3]));
        assert_eq!(history.next(ids[4]), None);
        assert_eq!(history.next(EntryId(99)), None);
        assert_eq!(History::new().newest(), None);
    }

    /// `back_by` and `forward_by` find the entry that is at least a time away,
    /// and give `None` when no entry is far enough.
    #[test]
    fn back_by_and_forward_by_find_an_entry_far_enough_away() {
        let (history, ids) = five_entries();
        let five = Duration::from_secs(5);

        assert_eq!(history.back_by(ids[4], five), Some(ids[1]));
        assert_eq!(
            history.back_by(ids[4], Duration::from_secs(2)),
            Some(ids[3])
        );
        assert_eq!(history.back_by(ids[1], five), None);
        assert_eq!(history.forward_by(ids[0], five), Some(ids[3]));
        assert_eq!(history.forward_by(ids[3], five), None);
        assert_eq!(history.forward_by(ids[0], Duration::ZERO), Some(ids[1]));
    }

    /// A jump and a span near the latest time that the clock holds give an
    /// answer and do not panic. A time is data from outside the program.
    #[test]
    fn a_time_near_the_end_of_the_clock_does_not_panic() {
        let last = u64::try_from(i64::MAX).expect("i64::MAX fits in a u64");
        let mut history = History::new();
        let first = history.add(frame(last - 4, vec![row(1, 1)]));
        let second = history.add(frame(last - 2, vec![row(1, 2)]));
        let minute = Duration::from_secs(60);

        assert_eq!(history.back_by(second, minute), None);
        assert_eq!(history.forward_by(first, minute), None);
        let reading = history
            .span(second, SpanWidth::OneMinute)
            .expect("the span holds two frames");
        assert_eq!(reading.ranking().total_faults, 3);
    }

    /// The sums of a span and of the extent stop at the largest value and do
    /// not overflow. A bad value of `top` can give a saturated count.
    #[test]
    fn the_sums_of_a_span_saturate() {
        let huge = |secs: u64| {
            let mut sampled = sample(secs, vec![row(1, u64::MAX)]);
            let observed = &mut sampled.observed;
            observed.ranking.window = Duration::MAX;
            observed.ranking.claude.faults = u64::MAX;
            observed.ranking.skipped.exited = usize::MAX;
            observed.ranking.skipped.exited_faults = u64::MAX;
            observed.swap = VmDelta {
                swapins: u64::MAX,
                swapouts: u64::MAX,
            };
            observed.swap_window = Duration::MAX;
            Update::Frame(Box::new(sampled))
        };
        let mut history = History::new();
        history.add(huge(0));
        let end = history.add(huge(2));

        let reading = history
            .span(end, SpanWidth::Whole)
            .expect("the span holds two frames");

        let ranking = reading.ranking();
        assert_eq!(ranking.total_faults, u64::MAX);
        assert_eq!(ranking.window, Duration::MAX);
        assert_eq!(ranking.claude.faults, u64::MAX);
        assert_eq!(ranking.skipped.exited, usize::MAX);
        assert_eq!(ranking.skipped.exited_faults, u64::MAX);
        assert_eq!(reading.header.swap.swapins, u64::MAX);
        assert_eq!(reading.header.swap.swapouts, u64::MAX);
        assert_eq!(reading.header.swap_window, Duration::MAX);
        assert_eq!(history.extent().measured, Duration::MAX);
    }

    /// Makes `count` rows with PIDs from `first`, each with a long command.
    fn long_rows(first: u32, count: u32) -> Vec<RankedRow> {
        (first..first + count)
            .map(|pid| RankedRow {
                command: format!("/bin/worker-{pid} {}", "x".repeat(400)),
                ..row(pid, 3)
            })
            .collect()
    }

    /// An identity record leaves when the last frame that points at it
    /// leaves, and its text leaves with it.
    #[test]
    fn an_identity_leaves_when_no_frame_points_at_it() {
        let mut probe = History::new();
        probe.add(frame(0, long_rows(1, 50)));
        let one_frame = probe.bytes_used();
        let mut history = History::with_budget(one_frame + one_frame / 2);
        let first = history.add(frame(0, long_rows(1, 50)));
        let second = history.add(frame(2, long_rows(100, 50)));

        let mut reference = History::new();
        reference.add(frame(2, long_rows(100, 50)));

        assert!(!history.contains(first));
        assert!(history.contains(second));
        assert_eq!(history.identities.index.len(), 50);
        assert_eq!(
            history.identities.text_bytes,
            reference.identities.text_bytes
        );
        assert!(history
            .series(
                ProcessKey {
                    pid: Pid::new(1),
                    started_at_epoch_secs: Some(1_700_000_000)
                },
                4,
                None
            )
            .is_none());
        assert!(history.bytes_used() < one_frame + one_frame / 2);
    }

    /// A process in two frames has one record, and the record counts both
    /// rows. The record stays when the first frame leaves.
    #[test]
    fn a_shared_identity_stays_while_a_frame_points_at_it() {
        let mut history = History::new();
        history.add(frame(0, long_rows(1, 50)));
        history.add(frame(2, long_rows(1, 50)));

        assert_eq!(history.identities.index.len(), 50);
        assert!(history.identities.index.values().all(|slot| {
            history.identities.slab[*slot as usize]
                .as_ref()
                .is_some_and(|record| record.rows == 2)
        }));

        history.evict_oldest();

        assert_eq!(history.identities.index.len(), 50);
        assert!(history.identities.index.values().all(|slot| {
            history.identities.slab[*slot as usize]
                .as_ref()
                .is_some_and(|record| record.rows == 1)
        }));
    }

    /// The widths cycle from one frame to the whole history and back, and each
    /// one has the label that the view shows.
    #[test]
    fn the_widths_cycle_and_have_labels() {
        let mut width = SpanWidth::OneFrame;
        let mut labels = Vec::new();
        for _ in 0..5 {
            labels.push(width.label());
            width = width.next();
        }

        assert_eq!(
            labels,
            vec![
                "this frame",
                "the last 1 m",
                "the last 5 m",
                "the last 15 m",
                "the whole history"
            ]
        );
        assert_eq!(width, SpanWidth::OneFrame);
    }

    /// The facts of the history: the newest frame, the gap of a name, the time
    /// of an entry, and the counts.
    #[test]
    fn the_history_states_its_facts() {
        let (mut history, ids) = five_entries();
        let trailing = history.add(gap(10, 2));

        assert_eq!(history.newest_frame(), Some(ids[4]));
        assert_eq!(history.newest(), Some(trailing));
        assert_eq!(
            history.gap(ids[2]).map(|gap| gap.reason.as_str()),
            Some("top stopped")
        );
        assert!(history.gap(ids[0]).is_none());
        assert_eq!(history.at(ids[3]), Some(time(6)));
        assert_eq!(history.at(EntryId(99)), None);
        assert_eq!(
            history.extent(),
            Extent {
                frames: 4,
                gaps: 2,
                oldest_at: Some(time(0)),
                newest_at: Some(time(10)),
                measured: Duration::from_secs(4 * WINDOW_SECS),
            }
        );
        assert_eq!(History::new().newest_frame(), None);
        assert_eq!(History::new().extent(), Extent::default());
    }

    /// The reason of a gap is cut at the stored size, on a character boundary.
    #[test]
    fn the_reason_of_a_gap_is_cut_on_a_boundary() {
        let mut history = History::new();
        let id = history.add(Update::Gap(Gap {
            at: time(0),
            lost: Duration::from_secs(2),
            reason: "é".repeat(STORED_TEXT_BYTES),
        }));

        let reason = &history.gap(id).expect("a gap").reason;

        assert!(reason.len() <= STORED_TEXT_BYTES);
        assert!(reason.ends_with(render::MORE));
        assert!(reason.starts_with('é'));
    }

    /// A command is cut at a character boundary, ends with the mark of a cut
    /// text, and the cut shrinks when one frame holds many rows.
    #[test]
    fn a_command_is_cut_on_a_boundary_and_by_the_share_of_the_budget() {
        let long = RankedRow {
            command: "日本語".repeat(10_000),
            ..row(1, 3)
        };
        let mut history = History::new();
        let id = history.add(frame(0, vec![long.clone()]));
        let reading = history.frame(id).expect("a frame");
        let command = &reading.row(0).unwrap().row.command;

        assert!(command.len() <= STORED_COMMAND_BYTES);
        assert!(command.ends_with(render::MORE));
        assert!(command.starts_with("日本語"));

        let mut tight = History::with_budget(10_000);
        let rows: Vec<RankedRow> = (1..=10)
            .map(|pid| RankedRow {
                pid: Pid::new(pid),
                ..long.clone()
            })
            .collect();
        let id = tight.add(frame(0, rows));
        let reading = tight.frame(id).expect("a frame");

        assert!(reading
            .rows()
            .all(|view| view.row.command.len() <= 10_000 / 20));
        assert_eq!(cut_text("abc", 3), "abc");
        assert_eq!(
            cut_text("日本語", 3 + render::MORE.len_utf8()),
            format!("日{}", render::MORE)
        );
    }

    /// A frame with no row is small, so a long run of them fills the history
    /// with entries alone. The deque of the entries doubles its capacity as it
    /// grows, and its capacity never gets smaller. One doubling must not make
    /// every frame but the newest leave: each add to a full history makes one
    /// frame leave. The deque takes at most an eighth of the budget, the share
    /// that the README states. The test writes the 8 itself, because a bound
    /// that reads `ENTRY_SHARE_DIVISOR` moves with a change of that constant.
    #[test]
    fn a_full_history_of_small_frames_keeps_more_than_one_frame() {
        let mut history = History::new();
        let first = history.add(frame(0, Vec::new()));
        let mut secs = 0;
        while history.contains(first) {
            secs += WINDOW_SECS;
            history.add(frame(secs, Vec::new()));
        }
        let full = history.extent().frames;

        history.add(frame(secs + WINDOW_SECS, Vec::new()));

        let kept = history.extent().frames;
        assert!(
            full > 1,
            "the add that filled the history kept {full} frames"
        );
        assert!(kept > 1, "the next add kept {kept} frames");
        assert_eq!(
            kept, full,
            "one add to a full history makes one frame leave"
        );
        let deque_bytes = history.entries.capacity() * size_of::<Entry>();
        assert!(
            deque_bytes <= BUDGET_BYTES / 8,
            "the deque of the entries holds {deque_bytes} bytes, past an eighth of the budget"
        );
    }
}
