//! A fill of the keepalive signal allocates nothing.
//!
//! The real-time audio thread calls `fill`, and an allocation on that thread
//! can block it. This binary replaces the global allocator with one that
//! counts allocations. A global allocator applies to the whole binary, so the
//! test has a binary of its own.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use popstop::signal::{KeepaliveSignal, SampleRate, LEVEL};

/// The number of allocations that a measuring thread made.
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// True while this thread measures. The other threads of the test harness
    /// can allocate at the same time, and the count leaves them out.
    static MEASURING: Cell<bool> = const { Cell::new(false) };
}

/// Passes every call to [`System`], and counts the allocations and
/// reallocations of a thread that measures.
struct CountingAllocator;

impl CountingAllocator {
    /// Counts one allocation when the calling thread measures.
    ///
    /// It touches only an atomic and a thread-local that has a constant
    /// initial value and no destructor, so it allocates nothing itself.
    fn count() {
        // `try_with` fails only while the thread exits. Such a thread does
        // not measure.
        if MEASURING.try_with(Cell::get).unwrap_or(false) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// SAFETY: every method passes its call to `System` with the same arguments,
// so this allocator keeps each rule of `GlobalAlloc` that `System` keeps. The
// count before the call allocates nothing and does not unwind.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::count();
        // SAFETY: the caller keeps the contract of `GlobalAlloc::alloc`, and
        // `System::alloc` has the same contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::count();
        // SAFETY: the caller keeps the contract of
        // `GlobalAlloc::alloc_zeroed`, and `System::alloc_zeroed` has the same
        // contract.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::count();
        // SAFETY: the caller keeps the contract of `GlobalAlloc::realloc`.
        // `ptr` came from this allocator, thus from `System`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller keeps the contract of `GlobalAlloc::dealloc`.
        // `ptr` came from this allocator, thus from `System`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// Runs `body` and gives the number of allocations that it made on this
/// thread.
fn allocations_during(body: impl FnOnce()) -> usize {
    let before = ALLOCATIONS.load(Ordering::SeqCst);
    MEASURING.with(|measuring| measuring.set(true));
    body();
    MEASURING.with(|measuring| measuring.set(false));
    ALLOCATIONS.load(Ordering::SeqCst) - before
}

#[test]
fn a_fill_of_the_keepalive_signal_allocates_nothing() {
    const CHANNELS: usize = 2;
    const BUFFER_FRAMES: [usize; 4] = [1, 7, 512, 4096];
    // The number of rounds through the four buffers before the stop and
    // after it. The first round before the stop holds the ramp up, and the
    // first round after it holds the ramp down.
    const ROUNDS: usize = 20;

    // Everything that allocates comes before the measurement.
    let rate = SampleRate::new(48_000.0).expect("48000 Hz is a valid sample rate");
    let (mut signal, stop) = KeepaliveSignal::new(rate);
    let mut buffers: Vec<Vec<f32>> = BUFFER_FRAMES
        .iter()
        .map(|frames| vec![f32::NAN; frames * CHANNELS])
        .collect();
    let mut loudest = 0.0_f32;
    let mut ends_in_silence = false;
    let mut complete = false;

    let allocations = allocations_during(|| {
        for _ in 0..ROUNDS {
            for buffer in &mut buffers {
                signal.fill(buffer, CHANNELS);
                loudest = buffer.iter().copied().fold(loudest, f32::max);
            }
        }
        stop.start_ramp_down();
        for _ in 0..ROUNDS {
            for buffer in &mut buffers {
                signal.fill(buffer, CHANNELS);
            }
        }
        ends_in_silence = buffers
            .iter()
            .all(|buffer| buffer.iter().all(|sample| *sample == 0.0));
        complete = stop.is_ramp_down_complete();
    });

    assert_eq!(loudest, LEVEL, "the run reaches the level");
    assert!(complete, "the run reaches the end of the ramp down");
    assert!(ends_in_silence, "the run ends in silence");
    assert_eq!(
        allocations, 0,
        "the fills allocated {allocations} times, and the audio thread must not allocate"
    );
}
