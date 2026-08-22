//! Tests for the JSON mode of the tool.
//!
//! The JSON mode prints one object for each sample and one final object that
//! carries the verdict. A consumer reads the stream line by line, so the two
//! kinds of line must stay apart: a sample line carries `at_seconds`, and the
//! verdict line carries `verdict`.
//!
//! Parallel safety: every test here is pure. The samples are built in memory,
//! and nothing is keyed on a fixed path, port, or name.

use std::time::Duration;

use serde_json::Value;
use thermal_watch::dvfs::DvfsTable;
use thermal_watch::mhz::Mhz;
use thermal_watch::powermetrics::{PressureLevel, Sample};
use thermal_watch::report::{judge, verdict_line, Verdict};

/// The DVFS table of an M4 Pro, trimmed to its two ends.
fn m4_pro() -> DvfsTable {
    DvfsTable::from_steps(
        vec![Mhz::new(1_260), Mhz::new(4_510)],
        vec![Mhz::new(1_020), Mhz::new(2_592)],
    )
}

/// One busy sample at a given second and clock.
fn busy(second: u64, mhz: u32) -> Sample {
    Sample {
        at: Duration::from_secs(second),
        p_freq: Some(Mhz::new(mhz)),
        p_active_pct: Some(99.9),
        e_freq: Some(Mhz::new(1_020)),
        cpu_power_mw: Some(48_500),
        gpu_power_mw: Some(0),
        pressure: PressureLevel::Nominal,
    }
}

/// A run of `count` busy samples, one each second, all at the same clock.
fn steady(count: u64, mhz: u32) -> Vec<Sample> {
    (0..count).map(|second| busy(second, mhz)).collect()
}

/// A run that holds its clock and then loses it. This is throttling.
fn decaying_run() -> Vec<Sample> {
    let mut samples = steady(20, 4_500);
    samples.extend((20..180).map(|second| busy(second, 3_400)));
    samples
}

/// The verdict of a run, as the JSON mode prints it, parsed back.
fn verdict_json(samples: &[Sample]) -> Value {
    parse(&judge(samples, &m4_pro()))
}

/// Serialize one verdict the way the tool does, then parse it back.
fn parse(verdict: &Verdict) -> Value {
    let line = serde_json::to_string(&verdict_line(verdict)).expect("the verdict serializes");
    serde_json::from_str(&line).expect("the verdict line is JSON")
}

#[test]
fn the_verdict_line_carries_the_outcome_and_the_numbers_of_the_run() {
    let line = verdict_json(&decaying_run());
    let verdict = &line["verdict"];

    assert_eq!(
        verdict["outcome"], "throttled",
        "the outcome names itself in lower snake case, got {line}"
    );
    let decay = verdict["decay"]
        .as_f64()
        .unwrap_or_else(|| panic!("a throttled verdict carries a decay, got {line}"));
    assert!(
        (decay - 0.244_444).abs() < 1e-3_f64,
        "3400 MHz is about 24% below 4500 MHz, got {decay}"
    );
    assert_eq!(
        verdict["peak"], 4_500,
        "the peak is a bare megahertz number"
    );
    assert_eq!(verdict["early_mean"], 4_500);
    assert_eq!(verdict["late_mean"], 3_400);
    assert_eq!(verdict["peak_power_mw"], 48_500);
    assert_eq!(verdict["worst_pressure"], "nominal");
    assert!(
        verdict["late_ratio_of_max"].is_f64(),
        "the ratio is a number, got {line}"
    );
}

#[test]
fn a_run_that_holds_its_clock_says_so_and_carries_no_variant_data() {
    let verdict = verdict_json(&steady(180, 4_500))["verdict"].clone();
    assert_eq!(verdict["outcome"], "held_clock");
    assert!(verdict["decay"].is_null(), "a held clock lost nothing");
    assert!(
        verdict["busy_samples"].is_null(),
        "a held clock counts no missing samples"
    );
}

#[test]
fn a_run_that_decayed_carries_its_decay_beside_the_outcome() {
    let verdict = verdict_json(&decaying_run())["verdict"].clone();
    assert_eq!(verdict["outcome"], "throttled");
    assert!(
        verdict["decay"].is_f64(),
        "the decay sits beside the outcome, not under it"
    );
    assert!(verdict["busy_samples"].is_null());
}

#[test]
fn a_run_that_never_sped_up_says_so_and_carries_no_variant_data() {
    let verdict = verdict_json(&steady(180, 2_600))["verdict"].clone();
    assert_eq!(verdict["outcome"], "never_reached_peak");
    assert!(verdict["decay"].is_null());
    assert!(verdict["busy_samples"].is_null());
}

#[test]
fn a_run_with_too_few_busy_samples_carries_the_count_beside_the_outcome() {
    let verdict = verdict_json(&steady(3, 4_500))["verdict"].clone();
    assert_eq!(verdict["outcome"], "not_enough_data");
    assert_eq!(
        verdict["busy_samples"], 3,
        "the count sits beside the outcome, not under it"
    );
    assert!(verdict["decay"].is_null());
}

/// The four clock numbers of an unjudged run are placeholders, not readings.
/// A consumer cannot tell a placeholder zero from a measured zero, so the line
/// must leave the four keys out.
#[test]
fn a_verdict_that_judged_nothing_carries_no_clock_measurement() {
    let line = verdict_json(&steady(3, 4_500));
    let verdict = &line["verdict"];
    for key in ["peak", "early_mean", "late_mean", "late_ratio_of_max"] {
        assert!(
            verdict[key].is_null(),
            "an unjudged verdict leaves {key} out, got {line}"
        );
    }
}

/// The power and the pressure are real readings of an unjudged run, and the
/// outcome and its count describe the run. All four stay on the line.
#[test]
fn a_verdict_that_judged_nothing_keeps_the_measurements_it_made() {
    let line = verdict_json(&steady(3, 4_500));
    let verdict = &line["verdict"];
    assert_eq!(verdict["outcome"], "not_enough_data");
    assert_eq!(verdict["busy_samples"], 3);
    assert_eq!(
        verdict["peak_power_mw"], 48_500,
        "the power is a reading of the run, got {line}"
    );
    assert_eq!(
        verdict["worst_pressure"], "nominal",
        "the pressure is a reading of the run, got {line}"
    );
}

/// A judged outcome keeps every clock number. This holds the line against a
/// fix that leaves the four keys out of each outcome.
#[test]
fn a_verdict_that_judged_the_run_carries_every_clock_measurement() {
    let throttled = verdict_json(&decaying_run());
    let verdict = &throttled["verdict"];
    assert_eq!(verdict["outcome"], "throttled");
    assert_eq!(verdict["peak"], 4_500, "got {throttled}");
    assert_eq!(verdict["early_mean"], 4_500, "got {throttled}");
    assert_eq!(verdict["late_mean"], 3_400, "got {throttled}");
    assert!(verdict["late_ratio_of_max"].is_f64(), "got {throttled}");

    let held = verdict_json(&steady(180, 4_500));
    let verdict = &held["verdict"];
    assert_eq!(verdict["outcome"], "held_clock");
    assert_eq!(verdict["peak"], 4_500, "got {held}");
    assert_eq!(verdict["early_mean"], 4_500, "got {held}");
    assert_eq!(verdict["late_mean"], 4_500, "got {held}");
    assert!(verdict["late_ratio_of_max"].is_f64(), "got {held}");
}

#[test]
fn a_sample_line_and_a_verdict_line_stay_apart_in_the_stream() {
    let sample_line = serde_json::to_string(&busy(7, 4_500)).expect("the sample serializes");
    let sample: Value = serde_json::from_str(&sample_line).expect("the sample line is JSON");
    assert!(
        !sample["at_seconds"].is_null(),
        "a sample line carries its time, got {sample_line}"
    );
    assert!(
        sample["verdict"].is_null(),
        "a sample line carries no verdict, got {sample_line}"
    );

    let verdict = verdict_json(&decaying_run());
    assert!(
        !verdict["verdict"].is_null(),
        "a verdict line carries the verdict key, got {verdict}"
    );
    assert!(
        verdict["at_seconds"].is_null(),
        "a verdict line carries no time, got {verdict}"
    );
}
