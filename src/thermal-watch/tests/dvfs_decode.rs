//! Tests for decoding a DVFS property into frequency steps.
//!
//! Parallel safety: every test here is pure. Nothing touches a file, a port, or
//! any other shared resource, so two copies of this binary can run at once.

use thermal_watch::dvfs::{decode_voltage_states, DvfsTable};
use thermal_watch::mhz::Mhz;

/// Build the raw bytes of a DVFS property from frequency and voltage pairs.
fn encode(pairs: &[(u32, u32)]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(pairs.len() * 8);
    for &(khz, volts) in pairs {
        raw.extend_from_slice(&khz.to_le_bytes());
        raw.extend_from_slice(&volts.to_le_bytes());
    }
    raw
}

#[test]
fn decodes_little_endian_kilohertz_pairs() {
    let raw = encode(&[(1_260_000, 790), (4_510_000, 938)]);
    assert_eq!(
        decode_voltage_states(&raw),
        vec![Mhz::new(1_260), Mhz::new(4_510)]
    );
}

#[test]
fn keeps_the_order_the_soc_lists() {
    let raw = encode(&[(2_000_000, 700), (1_000_000, 600), (3_000_000, 800)]);
    assert_eq!(
        decode_voltage_states(&raw),
        vec![Mhz::new(2_000), Mhz::new(1_000), Mhz::new(3_000)],
        "the decoder must not sort; the caller asks for the maximum itself"
    );
}

#[test]
fn drops_padding_entries_whose_frequency_is_zero() {
    let raw = encode(&[(1_260_000, 790), (0, 0), (0, 0)]);
    assert_eq!(decode_voltage_states(&raw), vec![Mhz::new(1_260)]);
}

#[test]
fn ignores_a_trailing_partial_entry() {
    let mut raw = encode(&[(1_260_000, 790)]);
    raw.extend_from_slice(&[0x11, 0x22, 0x33]);
    assert_eq!(
        decode_voltage_states(&raw),
        vec![Mhz::new(1_260)],
        "three stray bytes are not a step"
    );
}

#[test]
fn decodes_an_empty_property_to_no_steps() {
    assert_eq!(decode_voltage_states(&[]), Vec::new());
}

#[test]
fn rounds_kilohertz_to_the_nearest_megahertz() {
    // A real M4 Pro lists 4509104 kHz for its top step, which is 4509 MHz.
    let raw = encode(&[(4_509_104, 938), (2_499_500, 800)]);
    assert_eq!(
        decode_voltage_states(&raw),
        vec![Mhz::new(4_509), Mhz::new(2_500)]
    );
}

#[test]
fn reports_the_maximum_step_of_each_cluster() {
    let table = DvfsTable::from_steps(
        vec![Mhz::new(1_260), Mhz::new(4_510), Mhz::new(3_300)],
        vec![Mhz::new(1_020), Mhz::new(2_592)],
    );
    assert_eq!(table.p_max(), Mhz::new(4_510));
    assert_eq!(table.e_max(), Mhz::new(2_592));
    assert_eq!(table.p_steps().len(), 3);
    assert_eq!(table.e_steps().len(), 2);
}

#[test]
fn an_empty_table_reports_a_maximum_of_zero_rather_than_panicking() {
    let table = DvfsTable::from_steps(Vec::new(), Vec::new());
    assert_eq!(table.p_max(), Mhz::new(0));
    assert_eq!(table.e_max(), Mhz::new(0));
}
