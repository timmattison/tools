//! The keepalive: the output unit that plays the signal (macOS only).
//!
//! [`Keepalive`] holds the three things that belong together while popstop
//! runs: the output unit, the handle that stops the signal, and the name of
//! the device that the unit plays to. A start makes all three, and a stop
//! ramps the signal down to silence before it stops the unit, so the stop is
//! as quiet as the run.

use std::thread;
use std::time::{Duration, Instant};

use crate::output::{self, AudioError, OutputUnit};
use crate::signal::{KeepaliveSignal, StopHandle};

/// The longest time that a stop waits for the ramp down to reach silence.
///
/// The ramp lasts [`crate::signal::RAMP_DURATION`], and the audio thread
/// renders a few buffers in front of the device, so a stop waits a little
/// longer than the ramp. A device that stopped its stream never reaches
/// silence, and this bound is what ends the wait then.
const RAMP_DOWN_BOUND: Duration = Duration::from_secs(1);

/// The time between two looks at the ramp down.
const RAMP_DOWN_POLL: Duration = Duration::from_millis(5);

/// The signal that plays on the default output device.
///
/// The unit plays until [`Keepalive::stop`]. A drop without a stop stops the
/// unit at once, with no ramp down.
pub struct Keepalive {
    /// The output unit that plays the signal.
    unit: OutputUnit,
    /// The handle that ramps the signal down.
    stop: StopHandle,
    /// The name of the device that the unit plays to.
    device_name: String,
}

impl Keepalive {
    /// Starts the keepalive signal on the default output device.
    ///
    /// # Errors
    ///
    /// Returns an [`AudioError`] that names the Core Audio call that failed.
    pub fn start() -> Result<Self, AudioError> {
        let rate = output::default_output_sample_rate()?;
        let device_name = output::default_output_device_name()?;
        let (signal, stop) = KeepaliveSignal::new(rate);
        let unit = OutputUnit::start(rate, signal)?;
        Ok(Self {
            unit,
            stop,
            device_name,
        })
    }

    /// Gives the name of the device that the signal keeps awake.
    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Ramps the signal down to silence, then stops the output unit.
    ///
    /// # Errors
    ///
    /// Returns an [`AudioError`] that names the Core Audio call that failed.
    pub fn stop(self) -> Result<(), AudioError> {
        self.unit.stop()
    }
}

#[cfg(test)]
mod tests {
    use super::Keepalive;

    #[test]
    fn a_stop_reaches_silence_before_it_stops_the_unit() {
        let keepalive = Keepalive::start().unwrap_or_else(|error| {
            panic!(
                "the keepalive did not start: {error}. This test plays on the default output \
                 device, so the machine must have one"
            )
        });
        assert!(
            !keepalive.device_name().trim().is_empty(),
            "the keepalive names the device that it keeps awake"
        );
        let ramp = keepalive.stop.clone();

        keepalive.stop().expect("the keepalive stops");

        assert!(
            ramp.is_ramp_down_complete(),
            "the unit stopped while the signal still played, and that step is a click"
        );
    }
}
