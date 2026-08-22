//! Tests for the verdict of a run.
//!
//! Parallel safety: every test here is pure. The samples are built in memory.

use std::time::Duration;

use thermal_watch::dvfs::DvfsTable;
use thermal_watch::mhz::Mhz;
use thermal_watch::powermetrics::{PressureLevel, Sample};
use thermal_watch::report::{judge, windows, Outcome};

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
        cpu_power_mw: Some(30_000),
        gpu_power_mw: Some(0),
        pressure: PressureLevel::Nominal,
    }
}

/// One idle sample at a given second.
fn idle(second: u64) -> Sample {
    Sample {
        at: Duration::from_secs(second),
        p_freq: None,
        p_active_pct: None,
        e_freq: Some(Mhz::new(1_020)),
        cpu_power_mw: Some(120),
        gpu_power_mw: Some(0),
        pressure: PressureLevel::Nominal,
    }
}

/// A run of `count` busy samples, one each second, all at the same clock.
fn steady(count: u64, mhz: u32) -> Vec<Sample> {
    (0..count).map(|second| busy(second, mhz)).collect()
}

#[test]
fn a_clock_that_holds_at_the_peak_is_not_throttling() {
    let verdict = judge(&steady(180, 4_500), &m4_pro());
    assert_eq!(verdict.outcome, Outcome::HeldClock);
    assert_eq!(verdict.peak, Mhz::new(4_500));
    assert!(verdict.late_ratio_of_max > 0.99);
}

#[test]
fn a_clock_that_decays_from_its_early_peak_is_throttling() {
    let mut samples = steady(20, 4_500);
    samples.extend((20..180).map(|second| busy(second, 3_400)));

    let verdict = judge(&samples, &m4_pro());
    match verdict.outcome {
        Outcome::Throttled { decay } => {
            assert!(
                (decay - 0.244_444).abs() < 1e-3_f64,
                "3400 MHz is about 24% below 4500 MHz, got {decay}"
            );
        }
        other => panic!("expected throttling, got {other:?}"),
    }
    assert_eq!(verdict.peak, Mhz::new(4_500));
    assert_eq!(verdict.early_mean, Mhz::new(4_500));
    assert_eq!(verdict.late_mean, Mhz::new(3_400));
}

#[test]
fn a_clock_that_was_low_from_the_start_is_not_reported_as_decay() {
    let verdict = judge(&steady(180, 2_600), &m4_pro());
    assert_eq!(
        verdict.outcome,
        Outcome::NeverReachedPeak,
        "a machine that never sped up did not slow down"
    );
}

#[test]
fn too_few_busy_samples_give_no_verdict() {
    let verdict = judge(&steady(3, 4_500), &m4_pro());
    assert_eq!(verdict.outcome, Outcome::NotEnoughData { busy_samples: 3 });
}

#[test]
fn idle_samples_never_count_toward_the_verdict() {
    let idle_run: Vec<Sample> = (0..180).map(idle).collect();
    let verdict = judge(&idle_run, &m4_pro());
    assert_eq!(
        verdict.outcome,
        Outcome::NotEnoughData { busy_samples: 0 },
        "an idle machine reports a low clock, and that is not throttling"
    );
}

#[test]
fn an_idle_sample_among_busy_ones_does_not_drag_the_mean_down() {
    let mut samples = steady(180, 4_500);
    samples[90] = idle(90);

    let verdict = judge(&samples, &m4_pro());
    assert_eq!(verdict.outcome, Outcome::HeldClock);
    assert_eq!(
        verdict.late_mean,
        Mhz::new(4_500),
        "the idle sample must be dropped, not read as 0 MHz"
    );
}

#[test]
fn reports_the_worst_pressure_level_the_run_reached() {
    let mut samples = steady(180, 4_500);
    samples[40].pressure = PressureLevel::Fair;
    samples[41].pressure = PressureLevel::Serious;
    samples[42].pressure = PressureLevel::Fair;

    let verdict = judge(&samples, &m4_pro());
    assert_eq!(verdict.worst_pressure, PressureLevel::Serious);
}

#[test]
fn reports_the_highest_power_the_run_reached() {
    let mut samples = steady(180, 4_500);
    samples[10].cpu_power_mw = Some(48_500);

    let verdict = judge(&samples, &m4_pro());
    assert_eq!(verdict.peak_power_mw, 48_500);
}

#[test]
fn throttling_is_found_even_when_the_pressure_level_never_leaves_nominal() {
    let mut samples = steady(20, 4_500);
    samples.extend((20..180).map(|second| busy(second, 3_000)));
    for sample in &mut samples {
        sample.pressure = PressureLevel::Nominal;
    }

    let verdict = judge(&samples, &m4_pro());
    assert!(
        matches!(verdict.outcome, Outcome::Throttled { .. }),
        "the clock is the evidence; the pressure level is not"
    );
    assert_eq!(verdict.worst_pressure, PressureLevel::Nominal);
}

#[test]
fn a_short_run_still_reports_a_verdict() {
    // Thirty seconds is shorter than the early window plus the late window.
    // Each window becomes one third of the run, so the two windows stay
    // apart. The verdict must still be produced.
    let verdict = judge(&steady(30, 4_500), &m4_pro());
    assert_eq!(verdict.outcome, Outcome::HeldClock);
    assert_eq!(verdict.early_mean, Mhz::new(4_500));
    assert_eq!(verdict.late_mean, Mhz::new(4_500));
}

#[test]
fn a_twenty_second_collapse_is_throttling() {
    // The clock holds for fifteen seconds and then falls to 2000 MHz. A run
    // this short must still show the decay.
    let mut samples = steady(15, 4_500);
    samples.extend((15..20).map(|second| busy(second, 2_000)));

    let verdict = judge(&samples, &m4_pro());
    match verdict.outcome {
        Outcome::Throttled { decay } => {
            assert!(
                decay > 0.30_f64,
                "the clock fell from 4500 MHz to 2000 MHz, got a decay of {decay}"
            );
        }
        other => panic!("expected throttling, got {other:?}"),
    }
}

#[test]
fn a_forty_five_second_run_reports_the_decay_it_measured() {
    // The true decay is 44.4%. Windows that share samples dilute it to 24.7%.
    let mut samples = steady(20, 4_500);
    samples.extend((20..45).map(|second| busy(second, 2_500)));

    let verdict = judge(&samples, &m4_pro());
    match verdict.outcome {
        Outcome::Throttled { decay } => {
            assert!(
                decay > 0.40_f64,
                "2500 MHz is 44.4% below 4500 MHz, got a decay of {decay}"
            );
        }
        other => panic!("expected throttling, got {other:?}"),
    }
    assert_eq!(verdict.early_mean, Mhz::new(4_500));
    assert_eq!(verdict.late_mean, Mhz::new(2_500));
}

#[test]
fn the_early_window_and_the_late_window_never_share_a_sample() {
    // Each run starts at second zero and holds one sample each second.
    for count in [5_u64, 10, 20, 30, 45, 60, 90, 180, 300] {
        let (early_until, late_from) = windows(Duration::ZERO, Duration::from_secs(count - 1));
        assert!(
            early_until <= late_from,
            "a run of {count} samples ends the early window at {early_until:?} \
             and starts the late window at {late_from:?}"
        );
    }
}

#[test]
fn a_load_that_starts_late_takes_its_early_mean_from_the_load() {
    // The machine is idle for a minute, then the user starts a build. The
    // early mean must come from the start of the load.
    let mut samples: Vec<Sample> = (0..60).map(idle).collect();
    samples.extend((60..=300).map(|second| busy(second, 4_000)));
    // One spike lifts the peak above the clock of the load.
    samples[180] = busy(180, 4_500);

    let verdict = judge(&samples, &m4_pro());
    assert_eq!(verdict.peak, Mhz::new(4_500));
    assert_eq!(
        verdict.early_mean,
        Mhz::new(4_000),
        "the early mean comes from the start of the load, not from the peak"
    );
    assert_eq!(
        verdict.outcome,
        Outcome::NeverReachedPeak,
        "the clock never decayed, so this is not throttling"
    );
}
