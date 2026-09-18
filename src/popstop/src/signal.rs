//! The keepalive signal.
//!
//! The signal is a constant offset that is too small to hear. A short linear
//! ramp starts it and a short linear ramp stops it, because a step is a click.

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

/// The keepalive signal. The audio render thread owns it.
///
/// The signal keeps its position in the ramp across calls to
/// [`KeepaliveSignal::fill`], so the size of the buffers does not change the
/// samples.
#[derive(Debug)]
pub struct KeepaliveSignal {
    /// The number of frames from silence to the level.
    ramp_frames: u32,
    /// The position of the next frame in the ramp, from 0 to `ramp_frames`.
    /// The value of a frame is `LEVEL * position / ramp_frames`.
    position: u32,
}

/// The handle that stops the signal. The main thread holds it.
#[derive(Debug, Clone)]
pub struct StopHandle {}

impl KeepaliveSignal {
    /// Makes a signal for the given sample rate, and the handle that stops it.
    #[must_use]
    pub fn new(rate: SampleRate) -> (Self, StopHandle) {
        let signal = Self {
            ramp_frames: frames_in(RAMP_DURATION, rate),
            position: 0,
        };
        (signal, StopHandle {})
    }

    /// Writes the next samples of the signal into an interleaved buffer.
    ///
    /// The buffer holds `buffer.len() / channels` frames. Every channel of a
    /// frame gets the same value.
    pub fn fill(&mut self, buffer: &mut [f32], channels: usize) {
        for frame in buffer.chunks_exact_mut(channels) {
            frame.fill(self.next_sample());
        }
    }

    /// Gives the value of the next frame and moves the ramp on by one frame.
    fn next_sample(&mut self) -> f32 {
        let sample = LEVEL * (self.position as f32 / self.ramp_frames as f32);
        if self.position < self.ramp_frames {
            self.position += 1;
        }
        sample
    }
}

impl StopHandle {
    /// Asks the signal to ramp down to silence.
    ///
    /// The ramp down starts at the next call to [`KeepaliveSignal::fill`].
    pub fn start_ramp_down(&self) {}

    /// Tells whether the ramp down is complete.
    ///
    /// It is true after the signal wrote its first sample of silence at the
    /// end of the ramp down. From then on, every sample is 0.0.
    #[must_use]
    pub fn is_ramp_down_complete(&self) -> bool {
        false
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
}
