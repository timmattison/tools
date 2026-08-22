//! Tests for the frequency newtype.
//!
//! Parallel safety: every test here is pure.

use thermal_watch::mhz::Mhz;

#[test]
fn converts_kilohertz_to_megahertz() {
    assert_eq!(Mhz::from_khz(4_510_000), Mhz::new(4_510));
    assert_eq!(Mhz::from_khz(1_020_000), Mhz::new(1_020));
}

#[test]
fn rounds_to_the_nearest_megahertz() {
    assert_eq!(Mhz::from_khz(4_509_104), Mhz::new(4_509));
    assert_eq!(Mhz::from_khz(2_499_500), Mhz::new(2_500));
    assert_eq!(Mhz::from_khz(2_499_499), Mhz::new(2_499));
}

#[test]
fn reports_gigahertz_for_display() {
    assert!((Mhz::new(4_510).gigahertz() - 4.51).abs() < 1e-9_f64);
    assert_eq!(Mhz::new(4_510).to_string(), "4.51 GHz");
}

#[test]
fn reports_a_share_of_a_maximum() {
    assert!((Mhz::new(2_255).ratio_of(Mhz::new(4_510)) - 0.5).abs() < 1e-9_f64);
    assert!((Mhz::new(4_510).ratio_of(Mhz::new(4_510)) - 1.0).abs() < 1e-9_f64);
}

#[test]
fn a_maximum_of_zero_gives_a_share_of_zero_rather_than_an_infinity() {
    let ratio = Mhz::new(4_510).ratio_of(Mhz::new(0));
    assert!(ratio.is_finite(), "a zero maximum must not produce infinity");
    assert!((ratio - 0.0).abs() < f64::EPSILON);
}
