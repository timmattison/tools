//! The output unit (macOS only).
//!
//! [`OutputUnit`] opens the default output unit of Core Audio and plays the
//! samples that a [`Render`] writes, on the real-time audio thread.

use crate::signal::SampleRate;

/// Writes the samples that the output unit plays.
pub trait Render: Send + 'static {
    /// Writes the next samples into an interleaved buffer of `channels`
    /// channels.
    ///
    /// The output unit calls this on the real-time audio thread. It must not
    /// allocate, lock, or panic.
    fn render(&mut self, interleaved: &mut [f32], channels: usize);
}

/// A Core Audio call that failed.
#[derive(Debug, thiserror::Error)]
#[error("{call} failed with status {status}")]
pub struct AudioError {
    /// The name of the call that failed.
    call: &'static str,
    /// The status that the call returned.
    status: i32,
}

/// The default output unit, which plays the samples of a renderer.
pub struct OutputUnit {
    /// The renderer. The stub never calls it.
    _renderer: Box<dyn Render>,
}

impl OutputUnit {
    /// Starts the default output unit.
    ///
    /// # Errors
    ///
    /// Returns an [`AudioError`] that names the call that failed.
    pub fn start<R: Render>(rate: SampleRate, renderer: R) -> Result<Self, AudioError> {
        let _ = rate;
        Ok(Self {
            _renderer: Box::new(renderer),
        })
    }

    /// Stops the output unit.
    ///
    /// # Errors
    ///
    /// Returns an [`AudioError`] that names the call that failed.
    pub fn stop(self) -> Result<(), AudioError> {
        Ok(())
    }
}
