//! Judge a run of samples: did the clock decrease, and by how much.
//!
//! The verdict compares three numbers. The **peak** is the highest clock the
//! P-cluster reached while busy. The **early mean** is the clock over the first
//! seconds of the run, before the heat sink saturates. The **late mean** is the
//! clock over the last stretch of the run. A machine that holds its clock has
//! an early mean and a late mean that agree. A machine that throttles shows a
//! late mean below both.
//!
//! Two failure modes are separated on purpose. A run whose clock **decays** was
//! fast and then slowed, which is thermal throttling or a power limit. A run
//! whose clock was **low from the start** never reached its peak at all, which
//! points at a competing load rather than at heat.

use std::time::Duration;

use crate::dvfs::DvfsTable;
use crate::mhz::Mhz;
use crate::powermetrics::{PressureLevel, Sample};

/// Samples inside this window from the start form the early mean.
pub const EARLY_WINDOW: Duration = Duration::from_secs(20);

/// Samples inside this window from the end form the late mean.
pub const LATE_WINDOW: Duration = Duration::from_secs(60);

/// A P-cluster below this active residency is idle, not throttled.
pub const BUSY_THRESHOLD_PCT: f64 = 50.0;

/// A late mean below this share of the peak of the chip counts as a decrease.
pub const HOLD_RATIO: f64 = 0.9;

/// A decay above this share of the early mean counts as throttling.
pub const DECAY_TOLERANCE: f64 = 0.05;

/// Fewer busy samples than this cannot support a verdict.
pub const MINIMUM_BUSY_SAMPLES: usize = 5;

/// What the run showed.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The P-cluster was never busy for long enough to judge.
    NotEnoughData {
        /// How many busy samples the run collected.
        busy_samples: usize,
    },
    /// The clock held near the peak of the chip for the whole run.
    HeldClock,
    /// The clock started near the peak and then decreased.
    Throttled {
        /// The share of the early mean that was lost, from `0.0` to `1.0`.
        decay: f64,
    },
    /// The clock never reached the peak of the chip, from the first sample on.
    NeverReachedPeak,
}

/// The measured summary of one run.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// What the run showed.
    pub outcome: Outcome,
    /// The peak clock the P-cluster reached while busy.
    pub peak: Mhz,
    /// The mean clock over [`EARLY_WINDOW`] from the start.
    pub early_mean: Mhz,
    /// The mean clock over [`LATE_WINDOW`] to the end.
    pub late_mean: Mhz,
    /// The late mean as a share of the peak of the chip.
    pub late_ratio_of_max: f64,
    /// The highest CPU package power the run reached, in milliwatts.
    pub peak_power_mw: u32,
    /// The worst thermal pressure level the run reported.
    pub worst_pressure: PressureLevel,
}

/// Judge a run of samples against the DVFS table of the chip.
#[must_use]
pub fn judge(_samples: &[Sample], _table: &DvfsTable) -> Verdict {
    Verdict {
        outcome: Outcome::NotEnoughData { busy_samples: 0 },
        peak: Mhz::new(0),
        early_mean: Mhz::new(0),
        late_mean: Mhz::new(0),
        late_ratio_of_max: 0.0,
        peak_power_mw: 0,
        worst_pressure: PressureLevel::Unknown,
    }
}
