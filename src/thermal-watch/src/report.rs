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
pub fn judge(samples: &[Sample], table: &DvfsTable) -> Verdict {
    // An unknown level is the absence of a reading, not a level above
    // `Critical`, so it never wins the comparison for the worst one.
    let worst_pressure = samples
        .iter()
        .map(|sample| sample.pressure)
        .filter(|level| *level != PressureLevel::Unknown)
        .max()
        .unwrap_or(PressureLevel::Unknown);
    let peak_power_mw = samples
        .iter()
        .filter_map(|sample| sample.cpu_power_mw)
        .max()
        .unwrap_or(0);

    // An idle P-cluster reports a low clock. Counting it would report every
    // machine that spent a moment idle as a machine that throttled.
    let busy: Vec<&Sample> = samples
        .iter()
        .filter(|sample| sample.p_cluster_is_busy(BUSY_THRESHOLD_PCT))
        .collect();

    let empty = Verdict {
        outcome: Outcome::NotEnoughData {
            busy_samples: busy.len(),
        },
        peak: Mhz::new(0),
        early_mean: Mhz::new(0),
        late_mean: Mhz::new(0),
        late_ratio_of_max: 0.0,
        peak_power_mw,
        worst_pressure,
    };

    if busy.len() < MINIMUM_BUSY_SAMPLES {
        return empty;
    }

    let clock = |sample: &&Sample| sample.p_freq.map(Mhz::megahertz);
    let Some(peak) = busy.iter().filter_map(clock).max().map(Mhz::new) else {
        return empty;
    };
    let Some(last_at) = busy.last().map(|sample| sample.at) else {
        return empty;
    };
    let late_from = last_at.saturating_sub(LATE_WINDOW);

    let early_mean = mean_clock(
        busy.iter()
            .filter(|sample| sample.at < EARLY_WINDOW)
            .copied(),
    )
    .unwrap_or(peak);
    let Some(late_mean) = mean_clock(busy.iter().filter(|sample| sample.at >= late_from).copied())
    else {
        return empty;
    };

    let late_ratio_of_max = late_mean.ratio_of(table.p_max());
    let decay = 1.0 - late_mean.ratio_of(early_mean);

    let outcome = if decay >= DECAY_TOLERANCE {
        Outcome::Throttled { decay }
    } else if late_ratio_of_max >= HOLD_RATIO {
        Outcome::HeldClock
    } else {
        Outcome::NeverReachedPeak
    };

    Verdict {
        outcome,
        peak,
        early_mean,
        late_mean,
        late_ratio_of_max,
        peak_power_mw,
        worst_pressure,
    }
}

/// The mean clock of the samples that carry one.
fn mean_clock<'a>(samples: impl Iterator<Item = &'a Sample>) -> Option<Mhz> {
    let clocks: Vec<u32> = samples
        .filter_map(|sample| sample.p_freq.map(Mhz::megahertz))
        .collect();
    if clocks.is_empty() {
        return None;
    }
    let total: u64 = clocks.iter().map(|&mhz| u64::from(mhz)).sum();
    let count = clocks.len() as u64;
    // Round to the nearest megahertz rather than toward zero, so a run at a
    // steady clock reports exactly that clock.
    let rounded = (total + count / 2) / count;
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the mean of a list of u32 clocks cannot exceed u32::MAX"
    )]
    Some(Mhz::new(rounded as u32))
}
