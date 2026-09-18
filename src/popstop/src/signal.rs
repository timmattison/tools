//! The keepalive signal.
//!
//! The signal is a constant offset that is too small to hear. A short linear
//! ramp starts it and a short linear ramp stops it, because a step is a click.
//!
//! [`KeepaliveSignal`] runs on the real-time audio thread. It allocates
//! nothing and takes no lock. The main thread stops it through a
//! [`StopHandle`], and the two cross threads only through atomics.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The level of the signal: 2^-12 of full scale, about -72 dBFS.
///
/// That is 8 steps of 16-bit audio. When macOS scales the samples in
/// software for a device with no hardware volume, 8 steps stay above zero
/// after about 18 dB of attenuation.
pub const LEVEL: f32 = 1.0 / 4096.0;

/// The duration of the ramp up at the start and of the ramp down at the stop.
pub const RAMP_DURATION: Duration = Duration::from_millis(50);

/// A sample rate in hertz. The value is finite and larger than zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SampleRate(f64);

impl SampleRate {
    /// Makes a sample rate from a value in hertz.
    ///
    /// Returns `None` when the value is zero, negative, NaN, or infinite.
    #[must_use]
    pub fn new(hz: f64) -> Option<Self> {
        Some(Self(hz))
    }

    /// Gives the rate in hertz.
    #[must_use]
    pub fn hz(self) -> f64 {
        self.0
    }
}

/// The state that the signal and its stop handle share.
#[derive(Debug, Default)]
struct Shared {
    /// The main thread sets it to ask for the ramp down.
    stop_requested: AtomicBool,
    /// The render thread sets it after the ramp down reached silence.
    ramp_down_complete: AtomicBool,
}

/// The phase of the signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The ramp up, then the level.
    Playing,
    /// The ramp down from the current position to silence.
    RampingDown,
    /// Silence, after the ramp down.
    Silent,
}

/// The keepalive signal. The audio render thread owns it.
///
/// The signal keeps its position in the ramp across calls to
/// [`KeepaliveSignal::fill`], so the size of the buffers does not change the
/// samples.
#[derive(Debug)]
pub struct KeepaliveSignal {
    /// The number of frames from silence to the level.
    ramp_frames: u32,
    /// The position in the ramp, from 0 to `ramp_frames`. The value of a
    /// frame is `LEVEL * position / ramp_frames`.
    ///
    /// While the signal plays, it is the position of the next frame. While it
    /// ramps down, it is the position of the last frame.
    position: u32,
    /// The phase of the signal.
    phase: Phase,
    /// The state that the stop handle shares.
    shared: Arc<Shared>,
}

/// The handle that stops the signal. The main thread holds it.
#[derive(Debug, Clone)]
pub struct StopHandle {
    /// The state that the signal shares.
    shared: Arc<Shared>,
}

impl KeepaliveSignal {
    /// Makes a signal for the given sample rate, and the handle that stops it.
    ///
    /// This is the only call that allocates. Make the signal before the audio
    /// starts.
    #[must_use]
    pub fn new(rate: SampleRate) -> (Self, StopHandle) {
        let shared = Arc::new(Shared::default());
        let signal = Self {
            ramp_frames: frames_in(RAMP_DURATION, rate),
            position: 0,
            phase: Phase::Playing,
            shared: Arc::clone(&shared),
        };
        (signal, StopHandle { shared })
    }

    /// Writes the next samples of the signal into an interleaved buffer.
    ///
    /// The buffer holds `buffer.len() / channels` frames. Every channel of a
    /// frame gets the same value.
    ///
    /// The samples after the last whole frame get 0.0, and so does the whole
    /// buffer when `channels` is 0. Those samples do not move the ramp.
    pub fn fill(&mut self, buffer: &mut [f32], channels: usize) {
        if self.phase == Phase::Playing && self.shared.stop_requested.load(Ordering::Acquire) {
            self.phase = Phase::RampingDown;
        }
        let was_silent = self.phase == Phase::Silent;

        if channels == 0 {
            buffer.fill(0.0);
        } else {
            let mut frames = buffer.chunks_exact_mut(channels);
            for frame in &mut frames {
                frame.fill(self.next_sample());
            }
            frames.into_remainder().fill(0.0);
        }

        if !was_silent && self.phase == Phase::Silent {
            self.shared
                .ramp_down_complete
                .store(true, Ordering::Release);
        }
    }

    /// Gives the value of the next frame and moves the ramp on by one frame.
    ///
    /// The ramp down starts from the position of the last frame, so a stop
    /// during the ramp up makes no step.
    fn next_sample(&mut self) -> f32 {
        match self.phase {
            Phase::Playing => {
                let sample = self.value();
                if self.position < self.ramp_frames {
                    self.position += 1;
                }
                sample
            }
            Phase::RampingDown => {
                self.position = self.position.saturating_sub(1);
                if self.position == 0 {
                    self.phase = Phase::Silent;
                }
                self.value()
            }
            Phase::Silent => 0.0,
        }
    }

    /// Gives the value at the current position in the ramp.
    fn value(&self) -> f32 {
        LEVEL * (self.position as f32 / self.ramp_frames as f32)
    }
}

impl StopHandle {
    /// Asks the signal to ramp down to silence.
    ///
    /// The ramp down starts at the next call to [`KeepaliveSignal::fill`].
    pub fn start_ramp_down(&self) {
        self.shared.stop_requested.store(true, Ordering::Release);
    }

    /// Tells whether the ramp down is complete.
    ///
    /// It is true after the signal wrote its first sample of silence at the
    /// end of the ramp down. From then on, every sample is 0.0.
    #[must_use]
    pub fn is_ramp_down_complete(&self) -> bool {
        self.shared.ramp_down_complete.load(Ordering::Acquire)
    }
}

/// Gives the number of whole frames in `duration` at `rate`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a cast from f64 to u32 saturates, and a ramp longer than u32::MAX frames is not a real case"
)]
fn frames_in(duration: Duration, rate: SampleRate) -> u32 {
    (duration.as_secs_f64() * rate.hz()).round() as u32
}

#[cfg(test)]
mod tests {
    use super::{KeepaliveSignal, SampleRate, LEVEL, RAMP_DURATION};

    /// The sample rate of the tests, in hertz.
    const RATE_HZ: u32 = 48_000;

    /// The number of microseconds in one second.
    const MICROS_PER_SECOND: u128 = 1_000_000;

    fn rate() -> SampleRate {
        SampleRate::new(f64::from(RATE_HZ)).expect("48000 Hz is a valid sample rate")
    }

    /// The number of frames in one ramp at `RATE_HZ`.
    fn ramp_frames() -> usize {
        usize::try_from(RAMP_DURATION.as_micros() * u128::from(RATE_HZ) / MICROS_PER_SECOND)
            .expect("one ramp fits in usize")
    }

    /// The largest change from one frame to the next that a linear ramp makes.
    ///
    /// The margin covers the rounding of `f32`. A larger change is a step.
    fn ramp_step() -> f32 {
        const ROUNDING_MARGIN: f32 = 1.001;
        LEVEL / ramp_frames() as f32 * ROUNDING_MARGIN
    }

    /// Fills a new buffer of `frames` frames and gives its samples.
    ///
    /// The buffer starts as NaN, so a sample that `fill` does not write shows.
    fn fill_frames(signal: &mut KeepaliveSignal, frames: usize, channels: usize) -> Vec<f32> {
        let mut buffer = vec![f32::NAN; frames * channels];
        signal.fill(&mut buffer, channels);
        buffer
    }

    #[test]
    fn every_sample_after_the_ramp_up_equals_the_level() {
        let (mut signal, _stop) = KeepaliveSignal::new(rate());
        let ramp = ramp_frames();

        let first = fill_frames(&mut signal, ramp * 2, 2);
        let later = fill_frames(&mut signal, ramp, 2);
        let after_ramp: Vec<f32> = first[ramp * 2..].iter().chain(&later).copied().collect();

        assert_eq!(after_ramp.len(), ramp * 4);
        for (index, sample) in after_ramp.iter().enumerate() {
            assert_eq!(
                *sample, LEVEL,
                "sample {index} after the ramp up is {sample}, not the level"
            );
        }
    }

    #[test]
    fn the_ramp_up_rises_linearly_from_zero_to_the_level_and_never_decreases() {
        let (mut signal, _stop) = KeepaliveSignal::new(rate());
        let ramp = ramp_frames();

        let samples = fill_frames(&mut signal, ramp + 1, 1);

        assert_eq!(samples[0], 0.0, "the first sample is exactly zero");
        assert_eq!(
            samples[ramp], LEVEL,
            "the ramp reaches the level after RAMP_DURATION"
        );
        for (index, sample) in samples[..ramp].iter().enumerate() {
            assert!(
                *sample < LEVEL,
                "sample {index} is {sample}, so the level came before the end of the ramp"
            );
        }
        let step = ramp_step();
        for (index, pair) in samples.windows(2).enumerate() {
            let rise = pair[1] - pair[0];
            assert!(rise >= 0.0, "sample {} decreases by {}", index + 1, -rise);
            assert!(
                rise <= step,
                "sample {} rises by {rise}, which is more than one step of the ramp",
                index + 1
            );
        }
    }

    #[test]
    fn a_ramp_down_falls_to_exactly_zero_stays_there_and_reports_complete() {
        let (mut signal, stop) = KeepaliveSignal::new(rate());
        let ramp = ramp_frames();

        let before = fill_frames(&mut signal, ramp * 2, 1);
        assert_eq!(before.last(), Some(&LEVEL), "the signal is at the level");
        assert!(
            !stop.is_ramp_down_complete(),
            "the ramp down is not complete before the stop"
        );

        stop.start_ramp_down();
        assert!(
            !stop.is_ramp_down_complete(),
            "the ramp down is not complete before one sample of it plays"
        );

        // One frame for each fill, so the test reads the report after each sample.
        let step = ramp_step();
        let mut previous = LEVEL;
        let mut first_zero = None;
        for frame in 0..ramp * 3 {
            let sample = fill_frames(&mut signal, 1, 1)[0];
            assert!(
                (0.0..=previous).contains(&sample),
                "frame {frame} of the ramp down is {sample}, after {previous}"
            );
            assert!(
                previous - sample <= step,
                "frame {frame} of the ramp down falls by more than one step of the ramp"
            );
            if sample == 0.0 {
                first_zero.get_or_insert(frame);
            }
            assert_eq!(
                stop.is_ramp_down_complete(),
                first_zero.is_some(),
                "the report after frame {frame} ({sample}) is wrong"
            );
            previous = sample;
        }
        assert_eq!(
            first_zero,
            Some(ramp - 1),
            "the ramp down lasts RAMP_DURATION, then the signal is silent"
        );
    }

    #[test]
    fn a_stop_during_the_ramp_up_makes_no_step() {
        let (mut signal, stop) = KeepaliveSignal::new(rate());
        let ramp = ramp_frames();

        let before = fill_frames(&mut signal, ramp / 3, 1);
        let last_before = *before.last().expect("the ramp up played");
        assert!(
            last_before > 0.0 && last_before < LEVEL,
            "the stop comes during the ramp up, at {last_before}"
        );

        stop.start_ramp_down();
        let after = fill_frames(&mut signal, ramp * 2, 1);

        assert!(
            after[0] <= last_before,
            "the first sample of the ramp down ({}) is larger than the last sample before the stop ({last_before})",
            after[0]
        );
        assert!(
            last_before - after[0] <= ramp_step(),
            "the ramp down starts with a step from {last_before} to {}",
            after[0]
        );
        for (index, pair) in after.windows(2).enumerate() {
            assert!(
                pair[1] <= pair[0],
                "sample {} after the stop increases from {} to {}",
                index + 1,
                pair[0],
                pair[1]
            );
        }
        assert_eq!(after.last(), Some(&0.0), "the ramp down reaches silence");
        assert!(
            stop.is_ramp_down_complete(),
            "the ramp down reports complete"
        );
    }

    /// Plays `total` frames in buffers of at most `buffer_frames` frames, and
    /// starts the ramp down after `stop_at` frames. Gives all the samples.
    ///
    /// No buffer crosses the stop, so every buffer size sees the stop at the
    /// same frame.
    fn play_in_buffers(
        total: usize,
        stop_at: usize,
        buffer_frames: usize,
        channels: usize,
    ) -> Vec<f32> {
        let (mut signal, stop) = KeepaliveSignal::new(rate());
        let mut samples = Vec::with_capacity(total * channels);
        for (start, end) in [(0, stop_at), (stop_at, total)] {
            if start == stop_at {
                stop.start_ramp_down();
            }
            for buffer_start in (start..end).step_by(buffer_frames) {
                let frames = buffer_frames.min(end - buffer_start);
                samples.extend(fill_frames(&mut signal, frames, channels));
            }
        }
        samples
    }

    #[test]
    fn the_ramp_continues_across_buffers_of_any_size() {
        let ramp = ramp_frames();
        let total = ramp * 4 + 123;
        // One stop during the ramp up and one stop at the level.
        let stops = [ramp / 3, ramp + 777];

        for channels in [1, 2] {
            for stop_at in stops {
                let one_buffer = play_in_buffers(total, stop_at, total, channels);
                assert_eq!(one_buffer.len(), total * channels);
                assert_eq!(
                    one_buffer.last(),
                    Some(&0.0),
                    "the run ends in the silence after the ramp down"
                );
                for (index, frame) in one_buffer.chunks_exact(channels).enumerate() {
                    assert!(
                        frame.iter().all(|sample| *sample == frame[0]),
                        "the channels of frame {index} differ: {frame:?}"
                    );
                }

                for buffer_frames in [1, 7, 333, 4096] {
                    let many_buffers = play_in_buffers(total, stop_at, buffer_frames, channels);
                    assert!(
                        many_buffers == one_buffer,
                        "buffers of {buffer_frames} frames, {channels} channels, stop at \
                         frame {stop_at}: the samples differ from one large buffer, first at \
                         sample {:?}",
                        many_buffers
                            .iter()
                            .zip(&one_buffer)
                            .position(|(many, one)| many != one)
                    );
                }
            }
        }
    }

    #[test]
    fn a_ragged_buffer_fills_its_whole_frames_and_zeroes_the_remainder() {
        const CHANNELS: usize = 3;
        const WHOLE_FRAMES: usize = 3;
        const REMAINDER: usize = 2;
        let ramp = ramp_frames();

        // Two signals at the level, so that a whole frame is not 0.0.
        let (mut reference, _reference_stop) = KeepaliveSignal::new(rate());
        let (mut ragged, _ragged_stop) = KeepaliveSignal::new(rate());
        fill_frames(&mut reference, ramp, CHANNELS);
        fill_frames(&mut ragged, ramp, CHANNELS);

        let whole = fill_frames(&mut reference, WHOLE_FRAMES, CHANNELS);
        let mut buffer = vec![f32::NAN; WHOLE_FRAMES * CHANNELS + REMAINDER];
        ragged.fill(&mut buffer, CHANNELS);

        let (frames, remainder) = buffer.split_at(WHOLE_FRAMES * CHANNELS);
        assert_eq!(frames, whole.as_slice(), "the whole frames hold the signal");
        assert_eq!(remainder, [0.0; REMAINDER], "the remainder is silent");
        assert_eq!(
            fill_frames(&mut ragged, 1, CHANNELS),
            fill_frames(&mut reference, 1, CHANNELS),
            "the remainder does not move the ramp"
        );
    }

    #[test]
    fn zero_channels_do_not_panic_and_give_silence() {
        let (mut signal, _stop) = KeepaliveSignal::new(rate());
        fill_frames(&mut signal, ramp_frames(), 2);

        let mut buffer = [f32::NAN; 16];
        signal.fill(&mut buffer, 0);
        assert_eq!(buffer, [0.0; 16], "a buffer with no channels is silent");

        signal.fill(&mut [], 0);
        signal.fill(&mut [], 2);
    }
}
