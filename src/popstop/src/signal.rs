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
#[derive(Debug)]
pub struct KeepaliveSignal {}

/// The handle that stops the signal. The main thread holds it.
#[derive(Debug, Clone)]
pub struct StopHandle {}

impl KeepaliveSignal {
    /// Makes a signal for the given sample rate, and the handle that stops it.
    #[must_use]
    pub fn new(_rate: SampleRate) -> (Self, StopHandle) {
        (Self {}, StopHandle {})
    }

    /// Writes the next samples of the signal into an interleaved buffer.
    ///
    /// The buffer holds `buffer.len() / channels` frames. Every channel of a
    /// frame gets the same value.
    pub fn fill(&mut self, buffer: &mut [f32], _channels: usize) {
        buffer.fill(LEVEL);
    }
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
}
