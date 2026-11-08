//! The history of the samples that the view of `faulte tui` shows.
//!
//! The history holds a fixed memory budget of compact frames and gaps. A frame
//! is the ranking and the memory counters of one sample. A gap is a time that
//! the view tried to measure and did not. The module holds no terminal, no
//! clock, and no process. A [`SystemTime`] arrives only as data.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::mem::size_of;
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

/// A ranking that the history made from one frame or from a span of frames.
///
/// The extras of the rows are private and `rows` pairs each row with its own
/// extras, so that no caller can pair a row with the wrong ones.
#[derive(Debug)]
pub struct Reading {
    /// The ranking.
    ranking: Ranking,
    /// One extra for each row of the ranking.
    extras: Vec<RowExtra>,
}

impl Reading {
    /// Gives each row with its facts.
    pub fn rows(&self) -> impl ExactSizeIterator<Item = RowView<'_>> {
        std::iter::empty()
    }
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
    /// The oldest entries leave while the history holds more than its budget
    /// and more than one entry. The entry that arrived always stays.
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
        (!self.entries.is_empty()).then(|| EntryId(self.next_id - self.entries.len() as u64))
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

    /// Gives the reading of the span of `width` that ends at `end`.
    #[must_use]
    pub fn span(&self, _end: EntryId, _width: SpanWidth) -> Option<Reading> {
        None
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
}
