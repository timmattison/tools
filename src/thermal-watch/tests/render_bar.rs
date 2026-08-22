//! Tests for the bar of the live display.
//!
//! Parallel safety: every test here is pure.

use thermal_watch::render::bar;

#[test]
fn a_full_ratio_fills_every_cell() {
    assert_eq!(bar(1.0, 8), "████████");
}

#[test]
fn an_empty_ratio_fills_no_cell() {
    assert_eq!(bar(0.0, 8), "        ");
}

#[test]
fn a_half_ratio_fills_half_the_cells() {
    assert_eq!(bar(0.5, 8), "████    ");
}

#[test]
fn a_partial_cell_uses_an_eighth_block() {
    // Five and a half cells of eight.
    assert_eq!(bar(0.687_5, 8), "█████▌  ");
}

#[test]
fn a_ratio_outside_the_range_is_clamped_rather_than_refused() {
    assert_eq!(bar(2.0, 4), "████");
    assert_eq!(bar(-1.0, 4), "    ");
    assert_eq!(bar(f64::NAN, 4), "    ", "a strange sample must still draw");
}

#[test]
fn every_bar_occupies_the_same_number_of_cells() {
    for ratio in [0.0_f64, 0.01, 0.13, 0.5, 0.77, 0.99, 1.0] {
        assert_eq!(
            bar(ratio, 10).chars().count(),
            10,
            "a column of bars must stay aligned, ratio {ratio}"
        );
    }
}

#[test]
fn a_width_of_zero_draws_nothing() {
    assert_eq!(bar(0.5, 0), "");
}
