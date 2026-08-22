//! Tests for reading one `powermetrics` sample.
//!
//! Every fixture is a real shape of `powermetrics` output: the two-P-cluster
//! form of an M-series Pro or Max chip, the two-die form of an Ultra, the
//! single-P-cluster form of a base chip, and a block whose P-cluster is
//! offline.
//!
//! Parallel safety: every test here is pure. Nothing runs `powermetrics`.

use std::time::Duration;

use thermal_watch::mhz::Mhz;
use thermal_watch::powermetrics::{PressureLevel, Sample};

/// An M4 Pro divides its ten P-cores into two clusters, and prints both.
const TWO_P_CLUSTERS: &str = "\
*** Sampled system activity (Fri Aug 22 09:00:00 2026 -0400) (1002.31ms elapsed) ***

**** Processor usage ****

E-Cluster Online: 0-3
E-Cluster HW active frequency: 1332 MHz
E-Cluster HW active residency:  85.06% (1020 MHz:  12% 1400 MHz: 30%)
E-Cluster idle residency:  14.94%

P0-Cluster Online: 4-8
P0-Cluster HW active frequency: 4010 MHz
P0-Cluster HW active residency:  99.91% (4510 MHz: 60%)
P0-Cluster idle residency:   0.09%

P1-Cluster Online: 9-13
P1-Cluster HW active frequency: 3990 MHz
P1-Cluster HW active residency:  99.71% (4510 MHz: 58%)
P1-Cluster idle residency:   0.29%

CPU Power: 32104 mW
GPU Power: 21 mW
Combined Power (CPU + GPU + ANE): 32125 mW

**** Thermal pressure ****

Current pressure level: Nominal
";

/// An M-series Ultra joins two dies, and prints the E-cluster and the
/// P-cluster of each die. The numbers here make the two means easy to read:
/// the E-clusters give 1500 MHz, and the P-clusters give 3500 MHz.
const TWO_E_CLUSTERS: &str = "\
*** Sampled system activity (Fri Aug 22 09:00:00 2026 -0400) (1001.55ms elapsed) ***

**** Processor usage ****

E0-Cluster Online: 0-3
E0-Cluster HW active frequency: 1000 MHz
E0-Cluster HW active residency:  70.00% (1020 MHz:  40%)
E0-Cluster idle residency:  30.00%

P0-Cluster Online: 4-11
P0-Cluster HW active frequency: 4000 MHz
P0-Cluster HW active residency:  99.00% (4510 MHz: 55%)
P0-Cluster idle residency:   1.00%

E1-Cluster Online: 12-15
E1-Cluster HW active frequency: 2000 MHz
E1-Cluster HW active residency:  60.00% (2020 MHz:  30%)
E1-Cluster idle residency:  40.00%

P1-Cluster Online: 16-23
P1-Cluster HW active frequency: 3000 MHz
P1-Cluster HW active residency:  99.00% (4510 MHz: 45%)
P1-Cluster idle residency:   1.00%

CPU Power: 60000 mW
GPU Power: 40 mW
Combined Power (CPU + GPU + ANE): 60040 mW

**** Thermal pressure ****

Current pressure level: Nominal
";

/// A block whose second E-cluster line carries no number. `powermetrics`
/// prints a dash in place of a measurement it did not get.
const UNREADABLE_SECOND_E_CLUSTER: &str = "\
*** Sampled system activity ***

E0-Cluster HW active frequency: 1000 MHz
E0-Cluster HW active residency:  70.00%
E1-Cluster HW active frequency: -- MHz
P0-Cluster HW active frequency: 3000 MHz
P0-Cluster HW active residency:  99.00%
CPU Power: 30000 mW
";

/// A base M-series chip prints one P-cluster.
const ONE_P_CLUSTER: &str = "\
*** Sampled system activity ***

E-Cluster HW active frequency: 972 MHz
E-Cluster HW active residency:  40.00%
P-Cluster HW active frequency: 2880 MHz
P-Cluster HW active residency:  61.25%
CPU Power: 9000 mW
GPU Power: 0 mW

**** Thermal pressure ****
Current pressure level: Serious
";

/// An idle machine prints no P-cluster lines at all.
const NO_P_CLUSTER: &str = "\
*** Sampled system activity ***

E-Cluster HW active frequency: 1020 MHz
E-Cluster HW active residency:  5.00%
CPU Power: 120 mW
";

#[test]
fn averages_every_p_cluster_that_reports() {
    let sample = Sample::parse_block(TWO_P_CLUSTERS, Duration::from_secs(7));
    assert_eq!(
        sample.p_freq,
        Some(Mhz::new(4_000)),
        "the mean of 4010 MHz and 3990 MHz"
    );
    let residency = sample.p_active_pct.expect("a residency");
    assert!((residency - 99.81).abs() < 1e-9_f64);
    assert_eq!(sample.at, Duration::from_secs(7));
}

#[test]
fn averages_every_e_cluster_that_reports() {
    let sample = Sample::parse_block(TWO_E_CLUSTERS, Duration::from_secs(5));
    assert_eq!(
        sample.e_freq,
        Some(Mhz::new(1_500)),
        "the mean of 1000 MHz and 2000 MHz, not the last cluster of the block"
    );
    assert_eq!(
        sample.p_freq,
        Some(Mhz::new(3_500)),
        "the mean of 4000 MHz and 3000 MHz, which the two branches agree on"
    );
    assert_eq!(sample.at, Duration::from_secs(5));
}

#[test]
fn an_unreadable_e_cluster_line_keeps_the_frequency_already_read() {
    let sample = Sample::parse_block(UNREADABLE_SECOND_E_CLUSTER, Duration::ZERO);
    assert_eq!(
        sample.e_freq,
        Some(Mhz::new(1_000)),
        "a line with no number adds nothing, and erases nothing"
    );
    assert_eq!(sample.p_freq, Some(Mhz::new(3_000)));
}

#[test]
fn reads_the_remaining_fields_of_a_full_sample() {
    let sample = Sample::parse_block(TWO_P_CLUSTERS, Duration::ZERO);
    assert_eq!(sample.e_freq, Some(Mhz::new(1_332)));
    assert_eq!(sample.cpu_power_mw, Some(32_104));
    assert_eq!(sample.gpu_power_mw, Some(21));
    assert_eq!(sample.pressure, PressureLevel::Nominal);
}

#[test]
fn reads_a_single_p_cluster_chip() {
    let sample = Sample::parse_block(ONE_P_CLUSTER, Duration::ZERO);
    assert_eq!(sample.p_freq, Some(Mhz::new(2_880)));
    let residency = sample.p_active_pct.expect("a residency");
    assert!((residency - 61.25).abs() < 1e-9_f64);
    assert_eq!(sample.pressure, PressureLevel::Serious);
}

#[test]
fn never_reads_an_e_cluster_line_as_a_p_cluster_line() {
    let sample = Sample::parse_block(ONE_P_CLUSTER, Duration::ZERO);
    assert_eq!(sample.e_freq, Some(Mhz::new(972)));
    assert_ne!(
        sample.p_freq,
        Some(Mhz::new(972)),
        "the E-cluster frequency must never reach the P-cluster field"
    );
}

#[test]
fn an_offline_p_cluster_reads_as_absent_rather_than_as_zero() {
    let sample = Sample::parse_block(NO_P_CLUSTER, Duration::from_secs(3));
    assert_eq!(
        sample.p_freq, None,
        "an absent measurement is not a measurement of zero"
    );
    assert_eq!(sample.p_active_pct, None);
    assert_eq!(sample.cpu_power_mw, Some(120));
}

#[test]
fn a_block_with_no_pressure_line_reads_as_unknown() {
    let sample = Sample::parse_block(NO_P_CLUSTER, Duration::ZERO);
    assert_eq!(sample.pressure, PressureLevel::Unknown);
}

#[test]
fn reads_every_pressure_level_powermetrics_prints() {
    assert_eq!(PressureLevel::parse("Nominal"), PressureLevel::Nominal);
    assert_eq!(PressureLevel::parse("Fair"), PressureLevel::Fair);
    assert_eq!(PressureLevel::parse("Serious"), PressureLevel::Serious);
    assert_eq!(PressureLevel::parse("Critical"), PressureLevel::Critical);
}

#[test]
fn reads_a_pressure_level_whatever_its_case() {
    assert_eq!(PressureLevel::parse("nominal"), PressureLevel::Nominal);
    assert_eq!(PressureLevel::parse("SERIOUS"), PressureLevel::Serious);
}

#[test]
fn an_unfamiliar_pressure_level_reads_as_unknown_rather_than_as_nominal() {
    assert_eq!(
        PressureLevel::parse("Sleeping"),
        PressureLevel::Unknown,
        "a level this tool does not know must never read as the safest one"
    );
}

#[test]
fn a_busy_p_cluster_is_told_apart_from_an_idle_one() {
    let busy = Sample::parse_block(TWO_P_CLUSTERS, Duration::ZERO);
    assert!(busy.p_cluster_is_busy(50.0));

    let idle = Sample::parse_block(NO_P_CLUSTER, Duration::ZERO);
    assert!(
        !idle.p_cluster_is_busy(50.0),
        "a sample with no P-cluster measurement is never busy"
    );

    let light = Sample::parse_block(ONE_P_CLUSTER, Duration::ZERO);
    assert!(light.p_cluster_is_busy(50.0), "61.25% is above 50%");
    assert!(!light.p_cluster_is_busy(80.0), "61.25% is below 80%");
}

#[test]
fn ranks_the_pressure_levels_from_calm_to_severe() {
    assert!(PressureLevel::Nominal < PressureLevel::Fair);
    assert!(PressureLevel::Fair < PressureLevel::Serious);
    assert!(PressureLevel::Serious < PressureLevel::Critical);
}
