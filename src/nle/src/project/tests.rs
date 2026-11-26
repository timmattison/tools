//! Tests for the rules of the edit project.

use super::*;
use crate::error::NleError;
use crate::types::Seconds;

#[test]
fn seconds_refuses_a_time_that_is_not_finite_or_is_below_zero() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.5] {
        assert!(
            matches!(Seconds::new(bad), Err(NleError::InvalidTime(_))),
            "{bad} was accepted"
        );
    }
}

#[test]
fn seconds_accepts_zero_and_a_positive_time() {
    assert_eq!(Seconds::new(0.0).unwrap().get(), 0.0);
    assert_eq!(Seconds::new(1.5).unwrap().get(), 1.5);
}

#[test]
fn seconds_is_a_plain_json_number_and_refuses_a_negative_one() {
    assert_eq!(
        serde_json::to_string(&Seconds::new(1.5).unwrap()).unwrap(),
        "1.5"
    );
    assert_eq!(serde_json::from_str::<Seconds>("2.5").unwrap().get(), 2.5);
    assert!(serde_json::from_str::<Seconds>("-1.0").is_err());
}
