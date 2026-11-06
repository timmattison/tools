//! The domain of the Actions billing box: what a usage report holds, how its
//! items fall into UTC days, and which GitHub reads give the numbers.
//!
//! This module does no I/O and parses no JSON. `github.rs` turns the answers
//! of GitHub into [`UsageItem`] values, and the code here sums them.
//!
//! All windows count whole UTC days, because the report of GitHub has a date
//! and no time of day. No window is shorter than one day.

use std::collections::BTreeMap;
use std::iter::Sum;
use std::ops::{Add, AddAssign};

use chrono::{DateTime, Duration, NaiveDate, Utc};

use crate::money::Usd;
use crate::target::OwnerName;

/// The product name that the billing box counts. Copilot and the other
/// products of the report never count. The match ignores ASCII case.
pub const BILLED_PRODUCT: &str = "actions";

/// One line of a billing usage report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageItem {
    /// The moment of the usage. Its UTC date picks the day.
    pub date: DateTime<Utc>,
    /// The product, for example `actions` or `copilot`.
    pub product: String,
    /// The amount at list price, before any discount.
    pub gross: Usd,
    /// The amount that GitHub bills, after discounts.
    pub net: Usd,
    /// The organization that used the product. An enterprise report names it.
    pub organization: Option<String>,
}

impl UsageItem {
    /// The money of this one item.
    fn amounts(&self) -> Amounts {
        Amounts {
            billed: self.net,
            list_price: self.gross,
        }
    }
}

/// Whether the box counts the item: its product is [`BILLED_PRODUCT`].
fn is_billed(item: &UsageItem) -> bool {
    item.product.eq_ignore_ascii_case(BILLED_PRODUCT)
}

/// The money of a span of days, at two prices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Amounts {
    /// The sum of the `net` amounts: what GitHub bills.
    pub billed: Usd,
    /// The sum of the `gross` amounts: what the usage costs at list price.
    pub list_price: Usd,
}

/// The sums of the four windows of the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Windows {
    /// The usage of the UTC day `today`.
    pub today: Amounts,
    /// The usage of the day before `today`, and of no other day.
    pub yesterday: Amounts,
    /// The usage of `today` and the 6 days before it.
    pub last_7_days: Amounts,
    /// The usage of `today` and the 29 days before it.
    pub last_30_days: Amounts,
}

/// The reason why an enterprise report cannot give the usage of one
/// organization.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("the enterprise report has an {BILLED_PRODUCT} item with no organizationName.")]
pub struct MissingOrganization;

/// The billed and list-price sums of each UTC day.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DailyUsage(BTreeMap<NaiveDate, Amounts>);

impl Add for Amounts {
    type Output = Amounts;

    fn add(self, other: Amounts) -> Amounts {
        Amounts {
            billed: self.billed + other.billed,
            list_price: self.list_price + other.list_price,
        }
    }
}

impl AddAssign for Amounts {
    fn add_assign(&mut self, other: Amounts) {
        *self = *self + other;
    }
}

impl Sum for Amounts {
    fn sum<I: Iterator<Item = Amounts>>(iter: I) -> Amounts {
        iter.fold(Amounts::default(), Add::add)
    }
}

impl Add for Windows {
    type Output = Windows;

    fn add(self, other: Windows) -> Windows {
        Windows {
            today: self.today + other.today,
            yesterday: self.yesterday + other.yesterday,
            last_7_days: self.last_7_days + other.last_7_days,
            last_30_days: self.last_30_days + other.last_30_days,
        }
    }
}

impl AddAssign for Windows {
    fn add_assign(&mut self, other: Windows) {
        *self = *self + other;
    }
}

impl Sum for Windows {
    fn sum<I: Iterator<Item = Windows>>(iter: I) -> Windows {
        iter.fold(Windows::default(), Add::add)
    }
}

impl DailyUsage {
    /// Sums the billed items of an organization read or a user read by UTC
    /// day. An item of another product is left out.
    #[must_use]
    pub fn from_items(items: &[UsageItem]) -> DailyUsage {
        let mut days: BTreeMap<NaiveDate, Amounts> = BTreeMap::new();
        for item in items.iter().filter(|item| is_billed(item)) {
            *days.entry(item.date.date_naive()).or_default() += item.amounts();
        }
        DailyUsage(days)
    }

    /// Sums the billed items of one organization from an enterprise report.
    ///
    /// # Errors
    ///
    /// Returns [`MissingOrganization`] when a billed item names no
    /// organization, because the item could belong to the organization asked
    /// for, and a silent omission would show a sum that is too low.
    pub fn for_organization(
        items: &[UsageItem],
        organization: &OwnerName,
    ) -> Result<DailyUsage, MissingOrganization> {
        let mut own = Vec::new();
        for item in items.iter().filter(|item| is_billed(item)) {
            match item.organization.as_deref() {
                None => return Err(MissingOrganization),
                Some(name) if name.eq_ignore_ascii_case(organization.as_str()) => {
                    own.push(item.clone());
                }
                Some(_) => {}
            }
        }
        Ok(DailyUsage::from_items(&own))
    }

    /// Sums the days into the four windows of the box.
    #[must_use]
    pub fn windows(&self, today: NaiveDate) -> Windows {
        let sum_of = |days_back: i64| -> Amounts {
            let first = today
                .checked_sub_signed(Duration::days(days_back))
                .unwrap_or(NaiveDate::MIN);
            self.0.range(first..=today).map(|(_, amounts)| *amounts).sum()
        };
        Windows {
            today: sum_of(0),
            yesterday: today
                .pred_opt()
                .and_then(|day| self.0.get(&day).copied())
                .unwrap_or_default(),
            last_7_days: sum_of(6),
            last_30_days: sum_of(29),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> NaiveDate {
        text.parse().expect("a date")
    }

    fn today() -> NaiveDate {
        day("2026-11-06")
    }

    fn amount(value: f64) -> Usd {
        Usd::from_f64(value).expect("a finite amount")
    }

    fn item_at(moment: &str, product: &str, gross: f64, net: f64) -> UsageItem {
        UsageItem {
            date: moment.parse().expect("a moment"),
            product: product.to_owned(),
            gross: amount(gross),
            net: amount(net),
            organization: None,
        }
    }

    fn actions_on(date: &str, net: f64) -> UsageItem {
        item_at(&format!("{date}T00:00:00Z"), "actions", net, net)
    }

    fn windows_of(items: &[UsageItem]) -> Windows {
        DailyUsage::from_items(items).windows(today())
    }

    fn owner(text: &str) -> OwnerName {
        OwnerName::parse(text).expect("a valid owner")
    }

    #[test]
    fn an_item_29_days_back_is_in_30_days_and_30_days_back_is_not() {
        let inside = windows_of(&[actions_on("2026-10-08", 1.0)]);
        assert_eq!(inside.last_30_days.billed, amount(1.0));
        let outside = windows_of(&[actions_on("2026-10-07", 1.0)]);
        assert_eq!(outside.last_30_days.billed, Usd::ZERO);
    }

    #[test]
    fn an_item_6_days_back_is_in_7_days_and_7_days_back_is_not() {
        let inside = windows_of(&[actions_on("2026-10-31", 1.0)]);
        assert_eq!(inside.last_7_days.billed, amount(1.0));
        assert_eq!(inside.last_30_days.billed, amount(1.0));
        let outside = windows_of(&[actions_on("2026-10-30", 1.0)]);
        assert_eq!(outside.last_7_days.billed, Usd::ZERO);
        assert_eq!(outside.last_30_days.billed, amount(1.0));
    }

    #[test]
    fn today_is_in_every_window_but_yesterday() {
        let windows = windows_of(&[actions_on("2026-11-06", 2.0)]);
        assert_eq!(windows.today.billed, amount(2.0));
        assert_eq!(windows.yesterday.billed, Usd::ZERO);
        assert_eq!(windows.last_7_days.billed, amount(2.0));
        assert_eq!(windows.last_30_days.billed, amount(2.0));
    }

    #[test]
    fn yesterday_holds_only_the_day_before_today() {
        let windows = windows_of(&[
            actions_on("2026-11-06", 1.0),
            actions_on("2026-11-05", 10.0),
            actions_on("2026-11-04", 100.0),
        ]);
        assert_eq!(windows.yesterday.billed, amount(10.0));
        assert_eq!(windows.today.billed, amount(1.0));
        assert_eq!(windows.last_7_days.billed, amount(111.0));
    }

    #[test]
    fn a_day_after_today_is_in_no_window() {
        let windows = windows_of(&[actions_on("2026-11-07", 5.0)]);
        assert_eq!(windows, Windows::default());
    }

    #[test]
    fn the_last_second_of_a_utc_day_stays_in_that_day() {
        let windows = windows_of(&[item_at("2026-11-05T23:59:59Z", "actions", 3.0, 3.0)]);
        assert_eq!(windows.yesterday.billed, amount(3.0));
        assert_eq!(windows.today.billed, Usd::ZERO);
    }

    #[test]
    fn a_discounted_item_adds_zero_billed_and_its_gross_to_list_price() {
        let windows = windows_of(&[item_at("2026-11-06T10:00:00Z", "actions", 0.126, 0.0)]);
        assert_eq!(windows.today.billed, Usd::ZERO);
        assert_eq!(windows.today.list_price, amount(0.126));
    }

    #[test]
    fn only_the_actions_product_counts_whatever_its_case() {
        let windows = windows_of(&[
            item_at("2026-11-06T10:00:00Z", "copilot", 9.0, 9.0),
            item_at("2026-11-06T10:00:00Z", "Actions", 1.0, 1.0),
            item_at("2026-11-06T10:00:00Z", "packages", 7.0, 7.0),
        ]);
        assert_eq!(windows.today.billed, amount(1.0));
        assert_eq!(windows.today.list_price, amount(1.0));
    }

    #[test]
    fn no_items_give_zero_in_every_window() {
        assert_eq!(windows_of(&[]), Windows::default());
    }

    #[test]
    fn items_of_one_day_add_up_exactly() {
        let items: Vec<UsageItem> = (0..1000)
            .map(|_| item_at("2026-11-06T10:00:00Z", "actions", 0.006, 0.006))
            .collect();
        let windows = windows_of(&items);
        assert_eq!(windows.today.billed, amount(6.0));
        assert_eq!(windows.today.list_price, amount(6.0));
    }

    #[test]
    fn amounts_and_windows_add_field_by_field() {
        let one = Amounts {
            billed: amount(1.0),
            list_price: amount(2.0),
        };
        let two = Amounts {
            billed: amount(0.5),
            list_price: amount(0.25),
        };
        let expected = Amounts {
            billed: amount(1.5),
            list_price: amount(2.25),
        };
        assert_eq!(one + two, expected);
        assert_eq!([one, two].into_iter().sum::<Amounts>(), expected);
        let mut running = one;
        running += two;
        assert_eq!(running, expected);

        let first = Windows {
            today: one,
            yesterday: two,
            last_7_days: one,
            last_30_days: two,
        };
        let second = Windows {
            today: two,
            yesterday: one,
            last_7_days: two,
            last_30_days: one,
        };
        let both = Windows {
            today: expected,
            yesterday: expected,
            last_7_days: expected,
            last_30_days: expected,
        };
        assert_eq!(first + second, both);
        assert_eq!([first, second].into_iter().sum::<Windows>(), both);
        let mut running = first;
        running += second;
        assert_eq!(running, both);
    }

    fn enterprise_item(organization: Option<&str>, product: &str, net: f64) -> UsageItem {
        UsageItem {
            organization: organization.map(str::to_owned),
            ..item_at("2026-11-06T10:00:00Z", product, net, net)
        }
    }

    #[test]
    fn for_organization_keeps_only_its_own_items_ignoring_case() {
        let items = [
            enterprise_item(Some("Lock-Free-Labs"), "actions", 1.0),
            enterprise_item(Some("lock-free-labs"), "actions", 2.0),
            enterprise_item(Some("other-org"), "actions", 100.0),
        ];
        let usage = DailyUsage::for_organization(&items, &owner("LOCK-FREE-LABS"))
            .expect("every actions item names an organization");
        assert_eq!(usage.windows(today()).today.billed, amount(3.0));
    }

    #[test]
    fn for_organization_refuses_an_actions_item_with_no_organization() {
        let items = [
            enterprise_item(Some("acme"), "actions", 1.0),
            enterprise_item(None, "actions", 2.0),
        ];
        let error = DailyUsage::for_organization(&items, &owner("acme"))
            .expect_err("an item with no organization");
        assert_eq!(error, MissingOrganization);
        assert_eq!(
            error.to_string(),
            "the enterprise report has an actions item with no organizationName."
        );
    }

    #[test]
    fn for_organization_ignores_other_products_with_no_organization() {
        let items = [
            enterprise_item(None, "copilot", 50.0),
            enterprise_item(Some("acme"), "actions", 1.0),
        ];
        let usage = DailyUsage::for_organization(&items, &owner("acme"))
            .expect("a copilot item needs no organization");
        assert_eq!(usage.windows(today()).today.billed, amount(1.0));
    }
}
