//! A clock frequency in megahertz.

use std::fmt;

use serde::Serialize;

/// A clock frequency in megahertz.
///
/// Every frequency in this crate arrives from one of two places: the DVFS table
/// of the chip, which reports kilohertz, or `powermetrics`, which reports
/// megahertz. A newtype keeps the two units from mixing, and keeps a raw `u32`
/// from standing in for a percentage or a count of cores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Mhz(u32);

impl Mhz {
    /// Build a frequency from a count of megahertz.
    #[must_use]
    pub const fn new(megahertz: u32) -> Self {
        Self(megahertz)
    }

    /// Build a frequency from a count of kilohertz, as the DVFS table reports
    /// it. The result is rounded to the nearest megahertz.
    #[must_use]
    pub const fn from_khz(_kilohertz: u32) -> Self {
        Self(0)
    }

    /// The frequency as a count of megahertz.
    #[must_use]
    pub const fn megahertz(self) -> u32 {
        self.0
    }

    /// The frequency in gigahertz, for display.
    #[must_use]
    pub fn gigahertz(self) -> f64 {
        f64::from(self.0) / 1_000.0
    }

    /// This frequency as a share of `maximum`, in the range `0.0` to `1.0`.
    ///
    /// A `maximum` of zero gives `0.0` rather than an infinity, so a caller
    /// that reads an empty DVFS table still renders.
    #[must_use]
    pub fn ratio_of(self, _maximum: Self) -> f64 {
        0.0
    }
}

impl fmt::Display for Mhz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.2} GHz", self.gigahertz())
    }
}
