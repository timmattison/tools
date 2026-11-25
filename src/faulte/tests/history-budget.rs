//! The memory budget of the history, measured against the allocator.
//!
//! This file holds exactly one `#[test]`. The global allocator below counts the
//! live bytes of the whole process, so a second test that runs on another
//! thread would add its own bytes to the count. Three worst cases run one after
//! the other inside that one test, each on a new history.
//!
//! The sizes of the first two cases are the sizes that the design states. They
//! are no larger, because the Mac that runs this test is short of memory. The
//! third case is a long run of frames with no row, so the list of the entries
//! is the largest part of the history.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use faulte::duration::Span;
use faulte::history::{EntryId, History, SampledFrame, Update, BUDGET_BYTES};
use faulte::machine::Observed;
use faulte::pid::{Pid, Uid};
use faulte::ranking::{ClaudeTotal, ClaudeView, RankedRow, Ranking, Skipped};
use faulte::render::Accounts;
use faulte::vm::{SwapUsage, VmDelta};

/// The bytes that are allocated now and not yet freed.
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

/// An allocator that counts the live bytes and leaves the work to the system.
struct CountingAllocator;

// SAFETY: every method hands the call to `System` with the same arguments, so
// the contract of `GlobalAlloc` holds exactly as it holds for `System`. The
// counter is an atomic and has no effect on the memory.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller meets the contract of `alloc`, and we pass the
        // same layout on.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller gives a pointer and a layout from `alloc`, and we
        // pass both on unchanged.
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The slack that the live bytes may have over the budget.
const SLACK_BYTES: usize = BUDGET_BYTES / 16;

/// The count of frames of the first worst case.
const MANY_ROWS_FRAMES: usize = 3;

/// The count of rows of each frame of the first worst case.
const MANY_ROWS: usize = 16_000;

/// The bytes of each command of the first worst case. It is over the cut of
/// `budget / (2 * rows)`, about 2,097 bytes.
const MANY_ROWS_COMMAND_BYTES: usize = 3 * 1024;

/// The count of frames of the second worst case.
const LONG_COMMAND_FRAMES: usize = 12;

/// The count of rows of each frame of the second worst case.
const LONG_COMMAND_ROWS: usize = 500;

/// The bytes of each command of the second worst case. It is over
/// `STORED_COMMAND_BYTES`.
const LONG_COMMAND_BYTES: usize = 32 * 1024;

/// The count of frames of the third worst case. An entry takes more than 168
/// bytes, so the entries of this many frames with no row are more than the
/// budget.
const SMALL_FRAMES: usize = 400_000;

/// A character of two bytes, so that every cut must find a boundary.
const TWO_BYTE_CHAR: char = 'é';

/// The window of each frame.
const WINDOW: Duration = Duration::from_secs(2);

/// Gives the bytes that are live now.
fn live_bytes() -> usize {
    LIVE_BYTES.load(Ordering::Relaxed)
}

/// Makes one frame of `rows` rows. Every PID is new in each frame.
fn frame(index: usize, rows: usize, command_bytes: usize) -> Update {
    let template: String =
        std::iter::repeat_n(TWO_BYTE_CHAR, command_bytes / TWO_BYTE_CHAR.len_utf8()).collect();
    let ranked: Vec<RankedRow> = (0..rows)
        .map(|row| {
            let pid = u32::try_from(index * rows + row + 1).expect("the PID fits");
            RankedRow {
                pid: Pid::new(pid),
                uid: Uid::new(501),
                faults: 1 + (row % 7) as u64,
                rss_kib: Some(1_024),
                started_at_epoch_secs: Some(1_700_000_000 + u64::from(pid)),
                command: template.clone(),
                claude: ClaudeView::NotClaude,
            }
        })
        .collect();
    let total_faults = ranked.iter().map(|row| row.faults).sum();
    let observed = Observed {
        ranking: Ranking {
            rows: ranked,
            window: WINDOW,
            total_faults,
            claude: ClaudeTotal::default(),
            skipped: Skipped::default(),
        },
        table: Vec::new(),
        swap: VmDelta::default(),
        usage: SwapUsage::default(),
        compressor_bytes: 0,
        swap_window: WINDOW,
        accounts: Accounts::new(),
        now: SystemTime::UNIX_EPOCH,
        interval: "2s".parse::<Span>().expect("2s is a span"),
    };
    Update::Frame(Box::new(SampledFrame {
        observed,
        at: SystemTime::UNIX_EPOCH + Duration::from_secs(2 * index as u64),
        own: BTreeSet::new(),
    }))
}

/// Adds `frames` frames to a new history and checks the budget after each.
///
/// The check has three parts. The history must say that it holds no more than
/// the budget. The live bytes must stay near the budget. And the history must
/// not say less than the allocator measured, because then `bytes_used` would
/// tell something that is not true.
fn run_case(frames: usize, rows: usize, command_bytes: usize) -> usize {
    let mut history = History::new();
    let baseline = live_bytes();
    let mut first: Option<EntryId> = None;
    let mut peak = 0;
    for index in 0..frames {
        let id = history.add(frame(index, rows, command_bytes));
        first.get_or_insert(id);
        let live = live_bytes().saturating_sub(baseline);
        peak = peak.max(live);
        assert!(history.contains(id), "frame {index} is in the history");
        assert!(
            history.bytes_used() <= BUDGET_BYTES,
            "frame {index}: the history says {} bytes",
            history.bytes_used()
        );
        assert!(
            live <= BUDGET_BYTES + SLACK_BYTES,
            "frame {index}: {live} bytes are live"
        );
        assert!(
            history.bytes_used() + SLACK_BYTES >= live,
            "frame {index}: the history says {} bytes and {live} are live",
            history.bytes_used()
        );
    }
    assert!(
        !history.contains(first.expect("a frame was added")),
        "the budget must have pushed the first frame out"
    );
    peak
}

/// The three worst cases stay inside the budget: many rows, long commands,
/// and a long run of frames with no row.
#[test]
fn worst_cases_stay_inside_the_budget() {
    let many_rows = run_case(MANY_ROWS_FRAMES, MANY_ROWS, MANY_ROWS_COMMAND_BYTES);
    let long_commands = run_case(LONG_COMMAND_FRAMES, LONG_COMMAND_ROWS, LONG_COMMAND_BYTES);
    let small_frames = run_case(SMALL_FRAMES, 0, 0);
    eprintln!(
        "peak live bytes: many rows {many_rows}, long commands {long_commands}, \
         small frames {small_frames}"
    );
}
