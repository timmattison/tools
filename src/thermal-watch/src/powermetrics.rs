//! Run `powermetrics` and turn its output into samples.
//!
//! `powermetrics` is the only interface that reports the achieved clock of each
//! CPU cluster, and it prints plain text with no machine-readable mode that
//! carries the same fields. Its output is line-oriented, and each field this
//! module wants is a labelled line, so a line scan is the correct instrument
//! here rather than a parser.
//!
//! The command needs root. Nothing in this module asks for it, and nothing in
//! this module runs `sudo`. The caller decides.

use std::time::Duration;

use serde::Serialize;

use crate::mhz::Mhz;

/// The line that opens each sample of `powermetrics` output.
pub const SAMPLE_HEADER: &str = "*** Sampled system activity";

/// The samplers this tool asks `powermetrics` for.
pub const SAMPLERS: &str = "cpu_power,thermal";

/// How much work the OS believes the thermal budget can still absorb.
///
/// macOS raises this level to tell applications to do less work. It is not a
/// report of the clock: Apple Silicon decreases its clock long before the level
/// leaves [`PressureLevel::Nominal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PressureLevel {
    /// No pressure reported.
    Nominal,
    /// Light pressure.
    Fair,
    /// Heavy pressure.
    Serious,
    /// The OS is about to stop work to protect the hardware.
    Critical,
    /// The sample carried no pressure line, or one this tool does not know.
    Unknown,
}

impl PressureLevel {
    /// Read a level from the word `powermetrics` prints after `pressure level:`.
    #[must_use]
    pub fn parse(_word: &str) -> Self {
        Self::Unknown
    }
}

/// One sample of `powermetrics` output.
///
/// Every measured field is optional because `powermetrics` omits the lines of a
/// cluster that is offline. An absent P-cluster frequency means "not measured",
/// which is a different fact from "measured, and it was low" — reporting the
/// first as a zero would make an idle machine look fully throttled.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sample {
    /// Time from the start of the run to this sample.
    pub at: Duration,
    /// Mean active frequency across every P-cluster that reported.
    pub p_freq: Option<Mhz>,
    /// Mean active residency across every P-cluster that reported, as a
    /// percentage.
    pub p_active_pct: Option<f64>,
    /// Active frequency of the E-cluster.
    pub e_freq: Option<Mhz>,
    /// CPU package power, in milliwatts.
    pub cpu_power_mw: Option<u32>,
    /// GPU power, in milliwatts.
    pub gpu_power_mw: Option<u32>,
    /// The thermal pressure level of this sample.
    pub pressure: PressureLevel,
}

impl Sample {
    /// Read one sample from the block of text between two sample headers.
    ///
    /// A chip with more than one P-cluster, such as an M4 Pro, prints
    /// `P0-Cluster` and `P1-Cluster`. Both are read, and the frequencies are
    /// averaged, so the caller sees one number for the P-cores.
    #[must_use]
    pub fn parse_block(_block: &str, at: Duration) -> Self {
        Self {
            at,
            p_freq: None,
            p_active_pct: None,
            e_freq: None,
            cpu_power_mw: None,
            gpu_power_mw: None,
            pressure: PressureLevel::Unknown,
        }
    }

    /// True when the P-cluster was busy enough for its frequency to mean
    /// something. An idle cluster reports a low clock, and that is not
    /// throttling.
    #[must_use]
    pub fn p_cluster_is_busy(&self, threshold_pct: f64) -> bool {
        let _ = threshold_pct;
        false
    }
}

/// A running `powermetrics` process, read one sample at a time.
///
/// The process is stopped when this value is dropped, so an early return or a
/// panic cannot leave it behind.
#[derive(Debug)]
pub struct SampleStream {
    /// Placeholder until the process is wired up.
    _private: (),
}
