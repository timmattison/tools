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

use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde::{Serialize, Serializer};

use crate::mhz::Mhz;

/// The line that opens each sample of `powermetrics` output.
pub const SAMPLE_HEADER: &str = "*** Sampled system activity";

/// The samplers this tool asks `powermetrics` for.
pub const SAMPLERS: &str = "cpu_power,thermal";

/// The part of a cluster line that carries the achieved clock.
const FREQUENCY_LABEL: &str = "HW active frequency:";

/// The part of a cluster line that carries how busy the cluster was.
const RESIDENCY_LABEL: &str = "HW active residency:";

/// The line that carries CPU package power.
const CPU_POWER_LABEL: &str = "CPU Power:";

/// The line that carries GPU power.
const GPU_POWER_LABEL: &str = "GPU Power:";

/// The part of the thermal line that comes before the level.
const PRESSURE_LABEL: &str = "pressure level:";

/// The suffix every cluster name carries.
const CLUSTER_SUFFIX: &str = "-Cluster";

/// Which cluster a `powermetrics` line describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cluster {
    /// The efficiency cores.
    Efficiency,
    /// The performance cores. A chip with more than one P-cluster names them
    /// `P0`, `P1`, and so on, and every one of them is this kind.
    Performance,
}

/// Read which cluster a line names from the text before its label.
///
/// The names in service are `E-Cluster`, `P-Cluster`, and the numbered
/// `P0-Cluster` form of a chip with more than one performance cluster. A name
/// this function does not recognize gives `None`, so an unfamiliar line is
/// dropped rather than counted as a P-cluster.
fn cluster_of(head: &str) -> Option<Cluster> {
    let name = head.trim().strip_suffix(CLUSTER_SUFFIX)?;
    let mut characters = name.chars();
    let letter = characters.next()?;
    if !characters.all(|character| character.is_ascii_digit()) {
        return None;
    }
    match letter {
        'E' | 'e' => Some(Cluster::Efficiency),
        'P' | 'p' => Some(Cluster::Performance),
        _ => None,
    }
}

/// Read the number at the start of `text`, ignoring the unit after it.
fn leading_number(text: &str) -> Option<f64> {
    let trimmed = text.trim_start();
    let digits: String = trimmed
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    digits.parse().ok()
}

/// Split on `needle` without regard to case, giving the text after it.
fn split_once_ignoring_case<'a>(line: &'a str, needle: &str) -> Option<(&'a str, &'a str)> {
    let position = line
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())?;
    let (head, rest) = line.split_at(position);
    let tail = rest.get(needle.len()..)?;
    Some((head, tail))
}

/// The mean of a list of numbers, or `None` when the list is empty.
fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let total: f64 = values.iter().sum();
    #[allow(
        clippy::cast_precision_loss,
        reason = "a sample carries a handful of clusters, far below the precision of f64"
    )]
    Some(total / values.len() as f64)
}

/// Round a measured frequency onto the nearest whole megahertz.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "powermetrics reports a non-negative clock below 100 GHz, which fits in u32"
)]
fn round_to_mhz(value: f64) -> Mhz {
    Mhz::new(value.round().clamp(0.0, f64::from(u32::MAX)) as u32)
}

/// Round a measured power onto the nearest whole milliwatt.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "powermetrics reports a non-negative power far below u32::MAX milliwatts"
)]
fn round_to_milliwatts(value: f64) -> u32 {
    value.round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// Write a duration as a count of seconds, so a JSON sample stays readable.
fn as_seconds<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64(value.as_secs_f64())
}

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
    ///
    /// A word this tool does not know reads as [`Self::Unknown`] rather than as
    /// [`Self::Nominal`]. Reading an unfamiliar level as the calmest one would
    /// report a hot machine as a cool one.
    #[must_use]
    pub fn parse(word: &str) -> Self {
        match word.trim().to_ascii_lowercase().as_str() {
            "nominal" => Self::Nominal,
            "fair" => Self::Fair,
            "serious" => Self::Serious,
            "critical" => Self::Critical,
            _ => Self::Unknown,
        }
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
    #[serde(rename = "at_seconds", serialize_with = "as_seconds")]
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
    pub fn parse_block(block: &str, at: Duration) -> Self {
        let mut p_freqs: Vec<f64> = Vec::new();
        let mut p_residencies: Vec<f64> = Vec::new();
        let mut e_freq = None;
        let mut cpu_power_mw = None;
        let mut gpu_power_mw = None;
        let mut pressure = PressureLevel::Unknown;

        for line in block.lines() {
            let line = line.trim();

            if let Some((head, tail)) = line.split_once(FREQUENCY_LABEL) {
                match cluster_of(head) {
                    Some(Cluster::Performance) => {
                        if let Some(mhz) = leading_number(tail) {
                            p_freqs.push(mhz);
                        }
                    }
                    Some(Cluster::Efficiency) => {
                        e_freq = leading_number(tail).map(round_to_mhz);
                    }
                    None => {}
                }
                continue;
            }

            if let Some((head, tail)) = line.split_once(RESIDENCY_LABEL) {
                if matches!(cluster_of(head), Some(Cluster::Performance)) {
                    if let Some(percent) = leading_number(tail) {
                        p_residencies.push(percent);
                    }
                }
                continue;
            }

            if let Some(tail) = line.strip_prefix(CPU_POWER_LABEL) {
                cpu_power_mw = leading_number(tail).map(round_to_milliwatts);
                continue;
            }

            if let Some(tail) = line.strip_prefix(GPU_POWER_LABEL) {
                gpu_power_mw = leading_number(tail).map(round_to_milliwatts);
                continue;
            }

            if let Some((_, tail)) = split_once_ignoring_case(line, PRESSURE_LABEL) {
                pressure = PressureLevel::parse(tail);
            }
        }

        Self {
            at,
            p_freq: mean(&p_freqs).map(round_to_mhz),
            p_active_pct: mean(&p_residencies),
            e_freq,
            cpu_power_mw,
            gpu_power_mw,
            pressure,
        }
    }

    /// True when the P-cluster was busy enough for its frequency to mean
    /// something. An idle cluster reports a low clock, and that is not
    /// throttling.
    #[must_use]
    pub fn p_cluster_is_busy(&self, threshold_pct: f64) -> bool {
        self.p_freq.is_some() && self.p_active_pct.is_some_and(|pct| pct >= threshold_pct)
    }
}

/// A running `powermetrics` process, read one sample at a time.
///
/// The process is stopped when this value is dropped, so an early return or a
/// panic cannot leave it behind.
///
/// `powermetrics` needs root. This type does not ask for it and never runs
/// `sudo`: a caller without the privilege gets the refusal of the command
/// itself, which says so plainly.
#[derive(Debug)]
pub struct SampleStream {
    /// The running process, killed on drop.
    child: Child,
    /// Buffered standard output of the process.
    output: BufReader<ChildStdout>,
    /// When the run started, which every sample time is measured from.
    started: Instant,
    /// Lines of the sample being read, not yet complete.
    block: Vec<String>,
    /// True once the process closed its output.
    ended: bool,
    /// How the run ended, once it has been waited for.
    status: Option<ExitStatus>,
}

impl SampleStream {
    /// Start `powermetrics`, asking for `count` samples `interval` apart.
    ///
    /// # Errors
    ///
    /// Returns the error of the spawn when `powermetrics` cannot be started,
    /// which is what a caller without root sees.
    pub fn spawn(interval: Duration, count: u32) -> std::io::Result<Self> {
        let mut command = Command::new("powermetrics");
        command
            .arg("--samplers")
            .arg(SAMPLERS)
            .arg("-i")
            .arg(interval.as_millis().to_string())
            .arg("-n")
            .arg(count.to_string());
        Self::from_command(command)
    }

    /// Start an already-built command and read its output as samples.
    ///
    /// [`Self::spawn`] builds the `powermetrics` invocation and hands it here.
    /// A test hands a stand-in instead, which is how the lifecycle of a run is
    /// covered without root.
    ///
    /// # Errors
    ///
    /// Returns the error of the spawn when the command cannot be started.
    pub fn from_command(mut command: Command) -> std::io::Result<Self> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("the command started with no standard output"))?;

        Ok(Self {
            child,
            output: BufReader::new(stdout),
            started: Instant::now(),
            block: Vec::new(),
            ended: false,
            status: None,
        })
    }

    /// How the run ended.
    ///
    /// A run that measured nothing and a run that refused to start are the same
    /// shape at the iterator — both give no sample — and they need different
    /// answers. `powermetrics` without root is the second: it writes a refusal
    /// and exits non-zero. This tells the two apart.
    ///
    /// The status is kept, so asking more than once gives the same answer.
    pub fn exit_status(&mut self) -> Option<ExitStatus> {
        if self.status.is_none() {
            self.status = self.child.wait().ok();
        }
        self.status
    }

    /// Take the lines collected so far as one sample.
    fn take_block(&mut self) -> Option<Sample> {
        if self.block.is_empty() {
            return None;
        }
        let text = self.block.join("\n");
        self.block.clear();
        Some(Sample::parse_block(&text, self.started.elapsed()))
    }
}

impl Iterator for SampleStream {
    type Item = Sample;

    /// Read lines until the header of the next sample, then give the sample
    /// that just ended. The last sample is given when the output closes, so no
    /// sample is dropped at the end of a run.
    fn next(&mut self) -> Option<Sample> {
        if self.ended {
            return None;
        }

        loop {
            let mut line = String::new();
            match self.output.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    self.ended = true;
                    return self.take_block();
                }
                Ok(_) => {}
            }

            let line = line.trim_end().to_owned();
            if line.starts_with(SAMPLE_HEADER) {
                let finished = self.take_block();
                self.block.push(line);
                if finished.is_some() {
                    return finished;
                }
                continue;
            }

            // Anything before the first header is the banner `powermetrics`
            // prints at the start of a run. It carries no measurement, so it is
            // dropped rather than turned into a sample.
            if !self.block.is_empty() {
                self.block.push(line);
            }
        }
    }
}

impl Drop for SampleStream {
    /// Stop `powermetrics` rather than leave it sampling after this tool ends.
    fn drop(&mut self) {
        if self.status.is_some() {
            // Already waited for, so there is no process left to stop and no
            // zombie left to reap.
            return;
        }
        drop(self.child.kill());
        drop(self.child.wait());
    }
}
