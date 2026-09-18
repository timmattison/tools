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

use popstop::output::{default_output_device_name, default_output_sample_rate, OutputUnit, Render};
use popstop::signal::{KeepaliveSignal, SampleRate, RAMP_DURATION};

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

/// The longest time that the ramp down of the keepalive signal takes through
/// a real unit. The ramp lasts `RAMP_DURATION`, and the audio thread renders
/// a few buffers ahead.
const RAMP_DOWN_BOUND: Duration = Duration::from_millis(500);

/// The time that a test waits after a stop to see that no call comes.
const QUIET_AFTER_STOP: Duration = Duration::from_millis(100);

/// The lowest nominal rate of a real output device, in hertz.
const LOWEST_DEVICE_RATE_HZ: f64 = 8_000.0;

/// The highest nominal rate of a real output device, in hertz.
const HIGHEST_DEVICE_RATE_HZ: f64 = 384_000.0;

/// A stream rate, in hertz, that the output unit refuses. It refuses a
/// stream format from about 10 MHz up, with `kAudioUnitErr_FormatNotSupported`.
const REFUSED_RATE_HZ: f64 = 1.0e12;

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
    wait_within(RENDER_BOUND, condition)
}

/// Waits until `condition` is true, for `bound` at most. Tells whether it
/// became true.
fn wait_within(bound: Duration, condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + bound;
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

#[test]
fn the_default_output_device_has_a_name() {
    let name = default_output_device_name().unwrap_or_else(|error| {
        panic!(
            "the name of the default output device is not known: {error}. This test reads the \
             default output device, so the machine must have one"
        )
    });
    assert!(
        !name.trim().is_empty(),
        "the default output device has the name {name:?}, which is empty"
    );
}

#[test]
fn the_default_output_device_has_a_nominal_rate_in_the_range_of_real_devices() {
    let rate = default_output_sample_rate().unwrap_or_else(|error| {
        panic!(
            "the nominal sample rate of the default output device is not known: {error}. This \
             test reads the default output device, so the machine must have one"
        )
    });
    assert_eq!(
        SampleRate::new(rate.hz()),
        Some(rate),
        "the rate is a valid sample rate"
    );
    assert!(
        (LOWEST_DEVICE_RATE_HZ..=HIGHEST_DEVICE_RATE_HZ).contains(&rate.hz()),
        "the default output device has a nominal rate of {} Hz, outside {LOWEST_DEVICE_RATE_HZ} \
         Hz to {HIGHEST_DEVICE_RATE_HZ} Hz",
        rate.hz()
    );
}

#[test]
fn a_stop_frees_the_renderer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let unit = start(CountingSilence {
        calls: Arc::clone(&calls),
    });
    assert!(
        wait_until(|| calls.load(Ordering::Relaxed) > 0),
        "the output unit plays before the stop"
    );
    assert_eq!(
        Arc::strong_count(&calls),
        2,
        "the output unit holds the renderer while it plays"
    );

    unit.stop().expect("the output unit stops");
    assert_eq!(
        Arc::strong_count(&calls),
        1,
        "the output unit did not free the renderer at the stop"
    );
}

#[test]
fn a_drop_without_a_stop_stops_the_unit_and_frees_the_renderer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let unit = start(CountingSilence {
        calls: Arc::clone(&calls),
    });
    assert!(
        wait_until(|| calls.load(Ordering::Relaxed) > 0),
        "the output unit plays before the drop"
    );

    drop(unit);
    assert_eq!(
        Arc::strong_count(&calls),
        1,
        "the drop of the output unit did not free the renderer"
    );
    let at_drop = calls.load(Ordering::Relaxed);
    thread::sleep(QUIET_AFTER_STOP);
    assert_eq!(
        calls.load(Ordering::Relaxed),
        at_drop,
        "the output unit called the renderer after the drop"
    );
}

#[test]
fn a_start_that_fails_names_the_call_and_frees_the_renderer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let rate = SampleRate::new(REFUSED_RATE_HZ).expect("1 THz is a valid sample rate");

    let started = OutputUnit::start(
        rate,
        CountingSilence {
            calls: Arc::clone(&calls),
        },
    );
    let error = match started {
        Ok(unit) => {
            let stopped = unit.stop();
            panic!(
                "the output unit started at {REFUSED_RATE_HZ} Hz (stop: {stopped:?}), and this \
                 test needs a start that fails"
            );
        }
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .starts_with("AudioUnitSetProperty(kAudioUnitProperty_StreamFormat) failed"),
        "the error does not name the call that failed: {error}"
    );
    assert_eq!(
        Arc::strong_count(&calls),
        1,
        "the failed start did not free the renderer"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "a unit that did not start called the renderer"
    );
}

#[test]
fn the_audio_thread_plays_the_keepalive_signal_through_its_ramp_down() {
    let rate = SampleRate::new(RATE_HZ).expect("48000 Hz is a valid sample rate");
    let (signal, stop) = KeepaliveSignal::new(rate);
    let unit = start(signal);

    // The signal ramps up and plays at the level before the stop.
    thread::sleep(RAMP_DURATION * 2);
    assert!(
        !stop.is_ramp_down_complete(),
        "the ramp down is complete before the stop"
    );

    stop.start_ramp_down();
    assert!(
        wait_within(RAMP_DOWN_BOUND, || stop.is_ramp_down_complete()),
        "the ramp down of {RAMP_DURATION:?} is not complete after {RAMP_DOWN_BOUND:?}, so the \
         audio thread does not play the signal"
    );

    unit.stop().expect("the output unit stops");
}
