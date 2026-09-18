//! The output unit plays through the real default output device.
//!
//! These tests open the default output device and play silence or the
//! inaudible keepalive signal. They do not skip when the machine has no
//! output device. They fail with a message that says so.

// Core Audio exists on macOS only.
#![cfg(target_os = "macos")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use popstop::output::{OutputUnit, Render};
use popstop::signal::SampleRate;

/// The sample rate of the stream in these tests, in hertz. The output unit
/// converts it to the rate of the device.
const RATE_HZ: f64 = 48_000.0;

/// The longest time that a test waits for the audio thread.
const RENDER_BOUND: Duration = Duration::from_secs(2);

/// The time between two looks at a state that the audio thread changes.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// The number of calls that show that the audio thread calls the renderer
/// again and again, not only once.
const CALLS_THAT_SHOW_A_STREAM: usize = 3;

/// The time that a test waits after a stop to see that no call comes.
const QUIET_AFTER_STOP: Duration = Duration::from_millis(100);

/// A renderer that writes silence and counts its calls.
struct CountingSilence {
    /// The number of calls. The test holds a clone.
    calls: Arc<AtomicUsize>,
}

impl Render for CountingSilence {
    fn render(&mut self, interleaved: &mut [f32], _channels: usize) {
        interleaved.fill(0.0);
        self.calls.fetch_add(1, Ordering::Relaxed);
    }
}

/// Starts an output unit at `RATE_HZ`, or fails the test with a message that
/// tells why.
fn start(renderer: impl Render) -> OutputUnit {
    let rate = SampleRate::new(RATE_HZ).expect("48000 Hz is a valid sample rate");
    OutputUnit::start(rate, renderer).unwrap_or_else(|error| {
        panic!(
            "the output unit did not start: {error}. This test plays on the default output \
             device, so the machine must have one"
        )
    })
}

/// Waits until `condition` is true, for `RENDER_BOUND` at most. Tells whether
/// it became true.
fn wait_until(condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + RENDER_BOUND;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

#[test]
fn a_started_unit_calls_the_renderer_until_it_stops() {
    let calls = Arc::new(AtomicUsize::new(0));
    let unit = start(CountingSilence {
        calls: Arc::clone(&calls),
    });

    let streams = wait_until(|| calls.load(Ordering::Relaxed) >= CALLS_THAT_SHOW_A_STREAM);
    let seen = calls.load(Ordering::Relaxed);
    assert!(
        streams,
        "the output unit called the renderer {seen} times in {RENDER_BOUND:?}, and a stream \
         calls it at least {CALLS_THAT_SHOW_A_STREAM} times"
    );

    unit.stop().expect("the output unit stops");
    let at_stop = calls.load(Ordering::Relaxed);
    thread::sleep(QUIET_AFTER_STOP);
    assert_eq!(
        calls.load(Ordering::Relaxed),
        at_stop,
        "the output unit called the renderer after the stop"
    );
}
