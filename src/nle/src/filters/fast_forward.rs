//! The fast-forward filter: it compresses a region that holds a long burst of
//! command output, for example a build, an install, or a test run.
//!
//! Issue #162 names three modes. Skip to end jumps straight to the final state
//! of the region. Accelerate plays the region at 10 to 50 times its speed.
//! Summary shows the first lines, then "...", then the last lines. This module
//! gives skip to end and accelerate.

use crate::error::NleError;
use crate::filters::time_dilation::Speed;

/// An acceleration factor. It is finite and above 1. A fast-forward never
/// slows a region, because the time dilation filter does slow-downs.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Acceleration(Speed);

impl Acceleration {
    /// Make an acceleration from a number.
    ///
    /// # Errors
    /// Returns `NleError::InvalidAcceleration` when `value` is NaN, infinite,
    /// or not above 1.
    pub fn new(value: f64) -> Result<Self, NleError> {
        Speed::new(value)
            .map(Self)
            .map_err(|_| NleError::InvalidAcceleration(value))
    }

    /// The acceleration as a number.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0.get()
    }

    /// The acceleration as a `Speed`.
    #[must_use]
    pub fn speed(self) -> Speed {
        self.0
    }
}

#[cfg(test)]
mod tests;
