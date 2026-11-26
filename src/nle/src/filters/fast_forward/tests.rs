//! Tests for the fast-forward filter.

use super::*;

#[test]
fn acceleration_accepts_finite_values_above_one() {
    for good in [1.5, 20.0, 50.0] {
        assert_eq!(Acceleration::new(good).unwrap().get(), good);
    }
}

#[test]
fn acceleration_refuses_bad_values() {
    for bad in [
        1.0,
        0.5,
        0.0,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        assert!(
            matches!(
                Acceleration::new(bad),
                Err(NleError::InvalidAcceleration(_))
            ),
            "{bad} must be refused"
        );
    }
}
