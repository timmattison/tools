//! Money in US dollars, exact to the digits that GitHub wrote.
//!
//! The billing report of GitHub gives amounts as JSON numbers such as
//! `0.126`. A binary `f64` cannot hold that value, and a sum of 1000 such
//! amounts drifts from the true total. `Usd` holds a decimal number, so sums
//! stay exact. Only [`fmt::Display`] rounds, and it rounds one time, to cents.

use std::fmt;
use std::iter::Sum;
use std::ops::{Add, AddAssign, Mul};
use std::str::FromStr;

use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Deserializer};

/// An amount of US dollars with full decimal precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Usd(Decimal);

/// The reasons why an `f64` is not an amount of money.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MoneyError {
    /// The number is NaN or infinite.
    #[error("{value} is not a finite amount of money.")]
    NotFinite {
        /// The number as text.
        value: String,
    },
    /// The number does not fit in a decimal value.
    #[error("{value} is too large or too exact for an amount of money.")]
    OutOfRange {
        /// The number as text.
        value: String,
    },
}

impl Usd {
    /// No money.
    pub const ZERO: Usd = Usd(Decimal::ZERO);

    /// One cent, which is $0.01.
    pub const CENT: Usd = Usd::from_mills(10);

    /// Makes an amount from mills. A mill is one thousandth of a dollar.
    ///
    /// `Usd::from_mills(6)` is $0.006. The function is `const`, thus a price
    /// table can be a constant.
    #[must_use]
    pub const fn from_mills(mills: u32) -> Usd {
        Usd(Decimal::from_parts(mills, 0, 0, false, 3))
    }

    /// Whether the amount is exactly zero.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.0.is_zero()
    }

    /// Rounds the amount toward negative infinity to whole cents.
    ///
    /// The result is never above the amount. A caller uses it to make a claim
    /// such as "at least $0.01" that stays true.
    #[must_use]
    pub fn floor_to_cents(self) -> Usd {
        Usd(self
            .0
            .round_dp_with_strategy(2, RoundingStrategy::ToNegativeInfinity))
    }

    /// Makes an amount from the `f64` that a JSON reader gave.
    ///
    /// The conversion goes through the shortest text that reads back as the
    /// same `f64`. That text is the text GitHub wrote, thus `0.126` stays
    /// `0.126` and not the binary expansion of the nearest `f64`.
    ///
    /// # Errors
    ///
    /// Returns a [`MoneyError`] when the number is NaN or infinite, or when
    /// it does not fit in a decimal value.
    pub fn from_f64(value: f64) -> Result<Usd, MoneyError> {
        if !value.is_finite() {
            return Err(MoneyError::NotFinite {
                value: value.to_string(),
            });
        }
        // The `Display` text of an `f64` is the shortest text that reads back
        // as the same number, and it never uses an exponent.
        let text = value.to_string();
        Decimal::from_str(&text)
            .map(Usd)
            .map_err(|_| MoneyError::OutOfRange { value: text })
    }
}

impl Add for Usd {
    type Output = Usd;

    fn add(self, other: Usd) -> Usd {
        Usd(self.0 + other.0)
    }
}

impl Mul<u64> for Usd {
    type Output = Usd;

    fn mul(self, count: u64) -> Usd {
        Usd(self.0 * Decimal::from(count))
    }
}

impl AddAssign for Usd {
    fn add_assign(&mut self, other: Usd) {
        *self = *self + other;
    }
}

impl Sum<Usd> for Usd {
    fn sum<I: Iterator<Item = Usd>>(iter: I) -> Usd {
        iter.fold(Usd::ZERO, Add::add)
    }
}

impl<'a> Sum<&'a Usd> for Usd {
    fn sum<I: Iterator<Item = &'a Usd>>(iter: I) -> Usd {
        iter.fold(Usd::ZERO, |total, item| total + *item)
    }
}

impl fmt::Display for Usd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cents = self
            .0
            .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
        // A negative amount that rounds to zero shows no sign.
        let sign = if cents.is_sign_negative() && !cents.is_zero() {
            "-"
        } else {
            ""
        };
        write!(f, "{sign}${:.2}", cents.abs())
    }
}

impl<'de> Deserialize<'de> for Usd {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Usd, D::Error> {
        let value = f64::deserialize(deserializer)?;
        Usd::from_f64(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usd(value: f64) -> Usd {
        Usd::from_f64(value).expect("a finite amount")
    }

    #[test]
    fn point_one_plus_point_two_is_exactly_point_three() {
        let sum = usd(0.1) + usd(0.2);
        assert_eq!(sum, usd(0.3));
        assert_eq!(sum.to_string(), "$0.30");
    }

    #[test]
    fn from_f64_keeps_the_text_of_the_json_number() {
        assert_eq!(usd(0.126).to_string(), "$0.13");
        assert_eq!((usd(2.106) + usd(1.16)).to_string(), "$3.27");
    }

    #[test]
    fn a_thousand_small_amounts_sum_to_exactly_six() {
        let items = vec![usd(0.006); 1000];
        let by_value: Usd = items.iter().copied().sum();
        let by_reference: Usd = items.iter().sum();
        assert_eq!(by_value, usd(6.0));
        assert_eq!(by_reference, usd(6.0));
    }

    #[test]
    fn add_assign_adds() {
        let mut total = usd(1.5);
        total += usd(0.25);
        assert_eq!(total, usd(1.75));
    }

    #[test]
    fn display_rounds_to_cents_one_time_away_from_zero() {
        let cases = [
            (0.005, "$0.01"),
            (0.004, "$0.00"),
            (1234.5, "$1234.50"),
            (-0.5, "-$0.50"),
            (-0.005, "-$0.01"),
            (5.805, "$5.81"),
        ];
        for (value, shown) in cases {
            assert_eq!(usd(value).to_string(), shown, "value {value}");
        }
        assert_eq!(Usd::ZERO.to_string(), "$0.00");
    }

    #[test]
    fn a_sum_keeps_full_precision_until_display() {
        // Each part shows as $0.00, and the sum of 0.004 three times is 0.012.
        let total = usd(0.004) + usd(0.004) + usd(0.004);
        assert_eq!(usd(0.004).to_string(), "$0.00");
        assert_eq!(total.to_string(), "$0.01");
    }

    #[test]
    fn nan_and_infinity_are_errors() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let error = Usd::from_f64(value).expect_err("not finite");
            assert!(matches!(error, MoneyError::NotFinite { .. }), "{value}");
        }
    }

    #[test]
    fn a_json_number_deserializes_without_loss() {
        let parsed: Usd = serde_json::from_str("0.126").expect("a number");
        assert_eq!(parsed, usd(0.126));
        assert_eq!(parsed.to_string(), "$0.13");
        let whole: Usd = serde_json::from_str("12").expect("an integer");
        assert_eq!(whole.to_string(), "$12.00");
    }

    #[test]
    fn from_mills_is_thousandths_of_a_dollar() {
        assert_eq!(Usd::from_mills(6), usd(0.006));
        assert_eq!(Usd::from_mills(102), usd(0.102));
        assert_eq!(Usd::from_mills(1000), usd(1.0));
        assert_eq!(Usd::from_mills(0), Usd::ZERO);
    }

    #[test]
    fn from_mills_is_usable_in_a_const() {
        const PRICE: Usd = Usd::from_mills(62);
        assert_eq!(PRICE, usd(0.062));
    }

    #[test]
    fn cent_is_one_hundredth_of_a_dollar() {
        assert_eq!(Usd::CENT, usd(0.01));
        assert_eq!(Usd::CENT.to_string(), "$0.01");
    }

    #[test]
    fn multiplying_by_minutes_is_exact() {
        assert_eq!(Usd::from_mills(6) * 27, usd(0.162));
        #[expect(
            clippy::erasing_op,
            reason = "the test pins that zero minutes cost nothing"
        )]
        let no_minutes = Usd::from_mills(6) * 0;
        assert_eq!(no_minutes, Usd::ZERO);
        assert_eq!(Usd::from_mills(6) * 1000, usd(6.0));
    }

    #[test]
    fn is_zero_tells_zero_from_a_small_amount() {
        assert!(Usd::ZERO.is_zero());
        assert!(Usd::from_mills(0).is_zero());
        assert!(!Usd::from_mills(1).is_zero());
    }

    #[test]
    fn floor_to_cents_rounds_toward_negative_infinity() {
        assert_eq!(Usd::from_mills(16).floor_to_cents(), usd(0.01));
        assert_eq!(Usd::from_mills(19).floor_to_cents(), usd(0.01));
        assert_eq!(Usd::from_mills(6).floor_to_cents(), Usd::ZERO);
        assert_eq!(Usd::from_mills(160).floor_to_cents(), usd(0.16));
        assert_eq!(usd(-0.006).floor_to_cents(), usd(-0.01));
    }

    #[test]
    fn a_json_text_is_refused() {
        assert!(serde_json::from_str::<Usd>("\"0.1\"").is_err());
    }
}
