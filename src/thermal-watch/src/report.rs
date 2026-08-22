//! Judge a run of samples: did the clock decrease, and by how much.
//!
//! The verdict compares three numbers. The **peak** is the highest clock the
//! P-cluster reached while busy. The **early mean** is the clock over the first
//! seconds of the load, before the heat sink saturates. The **late mean** is
//! the clock over the last stretch of the load. A machine that holds its clock
//! has an early mean and a late mean that agree. A machine that throttles
//! shows a late mean below both.
//!
//! Both windows are measured from the busy samples, and [`windows`] keeps each
//! one to one third of the busy span or less. Thus the two windows never share
//! a sample, and a short run shows its decay at full size.
//!
//! Two failure modes are separated on purpose. A run whose clock **decays** was
//! fast and then slowed, which is thermal throttling or a power limit. A run
//! whose clock was **low from the start** never reached its peak at all, which
//! points at a competing load rather than at heat.

use std::time::Duration;

use serde::Serialize;

use crate::dvfs::DvfsTable;
use crate::mhz::Mhz;
use crate::powermetrics::{PressureLevel, Sample};

/// The longest early window. It starts at the first busy sample.
///
/// A run shorter than 60 seconds gets a smaller window. See [`windows`].
pub const EARLY_WINDOW: Duration = Duration::from_secs(20);

/// The longest late window. It ends at the last busy sample.
///
/// A run shorter than 180 seconds gets a smaller window. See [`windows`].
pub const LATE_WINDOW: Duration = Duration::from_secs(60);

/// The end of the early window and the start of the late window.
///
/// The early window holds every busy sample before the first value. The late
/// window holds every busy sample from the second value on. Give the time of
/// the first busy sample and the time of the last busy sample.
///
/// Each window is not longer than one third of the span between the two
/// samples. Thus the two windows never share a sample. The two windows reach
/// their full length at different spans. One third of the span becomes 20
/// seconds at a span of 60 seconds, thus the early window is the full
/// [`EARLY_WINDOW`] on a span of 60 seconds or more. One third of the span
/// becomes 60 seconds at a span of 180 seconds, thus the late window is the
/// full [`LATE_WINDOW`] on a span of 180 seconds or more. Only a span of 180
/// seconds or more gives both windows their full length.
#[must_use]
pub fn windows(first_at: Duration, last_at: Duration) -> (Duration, Duration) {
    let third = last_at.saturating_sub(first_at) / 3;
    let early_until = first_at.saturating_add(EARLY_WINDOW.min(third));
    let late_from = last_at.saturating_sub(LATE_WINDOW.min(third));
    (early_until, late_from)
}

/// A P-cluster below this active residency is idle, not throttled.
pub const BUSY_THRESHOLD_PCT: f64 = 50.0;

/// A late mean below this share of the peak of the chip counts as a decrease.
pub const HOLD_RATIO: f64 = 0.9;

/// A decay above this share of the early mean counts as throttling.
pub const DECAY_TOLERANCE: f64 = 0.05;

/// Fewer busy samples than this cannot support a verdict.
pub const MINIMUM_BUSY_SAMPLES: usize = 5;

/// What the run showed.
///
/// In JSON, each variant becomes an `outcome` key that names it in lower snake
/// case, and the data of the variant sits beside that key rather than under it.
/// [`Outcome::Throttled`] adds `decay`, [`Outcome::NotEnoughData`] adds
/// `busy_samples`, and the other two add nothing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
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
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    /// What the run showed. In JSON it becomes an `outcome` key beside the
    /// measurements, together with the data the outcome carries. See
    /// [`Outcome`].
    #[serde(flatten)]
    pub outcome: Outcome,
    /// The peak clock the P-cluster reached while busy.
    pub peak: Mhz,
    /// The mean clock over the early window, which starts at the first busy
    /// sample. See [`windows`].
    pub early_mean: Mhz,
    /// The mean clock over the late window, which ends at the last busy
    /// sample. See [`windows`].
    pub late_mean: Mhz,
    /// The late mean as a share of the peak of the chip.
    pub late_ratio_of_max: f64,
    /// The highest CPU package power the run reached, in milliwatts.
    pub peak_power_mw: u32,
    /// The worst thermal pressure level the run reported.
    pub worst_pressure: PressureLevel,
}

/// One [`Verdict`] in the shape the JSON mode prints it.
///
/// The whole verdict sits under a single `verdict` key. A reader of the
/// line-delimited stream tells a verdict line from a sample line by that key
/// alone, because a sample line carries `at_seconds` and no `verdict`.
///
/// Build one with [`verdict_line`]. The field is private, so the shape has one
/// entrance and the printed stream cannot drift from what the tests hold.
///
/// ```text
/// {"verdict":{"outcome":"throttled","decay":0.244,"peak":4500,"early_mean":4500,
///  "late_mean":3400,"late_ratio_of_max":0.754,"peak_power_mw":48500,
///  "worst_pressure":"nominal"}}
/// ```
///
/// The tool prints it on one line. It is broken here to fit the page.
#[derive(Debug, Serialize)]
pub struct VerdictLine<'a> {
    /// The verdict this line carries.
    verdict: &'a Verdict,
}

/// Wrap a verdict in the shape the JSON mode prints it. See [`VerdictLine`].
#[must_use]
pub const fn verdict_line(verdict: &Verdict) -> VerdictLine<'_> {
    VerdictLine { verdict }
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
    // The windows are anchored on the busy samples, not on time zero. A user
    // who starts a build one minute into the run gets an early mean from the
    // start of the load.
    let (Some(first_at), Some(last_at)) = (
        busy.first().map(|sample| sample.at),
        busy.last().map(|sample| sample.at),
    ) else {
        return empty;
    };
    let (early_until, late_from) = windows(first_at, last_at);

    let early_mean = mean_clock(
        busy.iter()
            .filter(|sample| sample.at < early_until)
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
