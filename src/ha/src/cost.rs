//! The list-price cost of one job and of one run (issue #639).
//!
//! GitHub gives no cost for one job. The `timing` endpoint is closing down,
//! and its `billable` data is empty. Thus the cost here is an estimate at list
//! price, from data that `ha` already reads: the runner labels of the job, its
//! time on a runner, and the visibility of its repository.
//!
//! The estimate is not the bill. The free minutes that an account includes can
//! make the bill $0.

use std::time::Duration;

use crate::money::Usd;

/// The SKU of a runner that `ha` can price.
///
/// A SKU is the unit that GitHub uses to price runner minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Sku {
    /// The 1-core Linux runner, label `ubuntu-slim`.
    LinuxSlim,
    /// The standard x64 Linux runner.
    Linux,
    /// The standard Arm Linux runner.
    LinuxArm,
    /// The standard x64 Windows runner.
    Windows,
    /// The standard Arm Windows runner.
    WindowsArm,
    /// The standard macOS runner, 3 cores or 4 cores.
    Macos,
    /// The larger macOS runner, label ending in `-large`.
    MacosLarge,
    /// The extra large macOS runner, label ending in `-xlarge`.
    MacosXlarge,
}

/// The labels of the GitHub-hosted runners that `ha` can price.
///
/// Sources: <https://docs.github.com/en/actions/reference/runners/github-hosted-runners>
/// and <https://docs.github.com/en/actions/reference/runners/larger-runners>,
/// read on 2026-11-07. This is the one table of labels. `Sku::of_label` reads
/// it.
const LABELS: &[(&str, Sku)] = &[
    ("ubuntu-slim", Sku::LinuxSlim),
    ("ubuntu-latest", Sku::Linux),
    ("ubuntu-26.04", Sku::Linux),
    ("ubuntu-24.04", Sku::Linux),
    ("ubuntu-22.04", Sku::Linux),
    ("ubuntu-26.04-arm", Sku::LinuxArm),
    ("ubuntu-24.04-arm", Sku::LinuxArm),
    ("ubuntu-22.04-arm", Sku::LinuxArm),
    ("windows-latest", Sku::Windows),
    ("windows-2025", Sku::Windows),
    ("windows-2025-vs2026", Sku::Windows),
    ("windows-2022", Sku::Windows),
    ("windows-11-arm", Sku::WindowsArm),
    ("windows-11-vs2026-arm", Sku::WindowsArm),
    ("macos-latest", Sku::Macos),
    ("macos-26", Sku::Macos),
    ("macos-15", Sku::Macos),
    ("xcode-27", Sku::Macos),
    ("macos-26-intel", Sku::Macos),
    ("macos-15-intel", Sku::Macos),
    ("macos-latest-large", Sku::MacosLarge),
    ("macos-26-large", Sku::MacosLarge),
    ("macos-15-large", Sku::MacosLarge),
    ("macos-latest-xlarge", Sku::MacosXlarge),
    ("macos-26-xlarge", Sku::MacosXlarge),
    ("macos-15-xlarge", Sku::MacosXlarge),
    ("xcode-27-xlarge", Sku::MacosXlarge),
];

/// Billing names that the rule of `Sku::of_billing_name` cannot reach.
///
/// The usage endpoint named the macOS SKU `Actions macOS 3-core` on 2026-11-07.
const BILLING_NAME_EXTRAS: &[(&str, Sku)] = &[("Actions macOS 3-core", Sku::Macos)];

impl Sku {
    /// Every SKU, in the order of the price table.
    pub const ALL: [Sku; 8] = [
        Sku::LinuxSlim,
        Sku::Linux,
        Sku::LinuxArm,
        Sku::Windows,
        Sku::WindowsArm,
        Sku::Macos,
        Sku::MacosLarge,
        Sku::MacosXlarge,
    ];

    /// The name of the SKU in the pricing table of GitHub, for example
    /// `actions_linux`.
    #[must_use]
    pub fn table_name(self) -> &'static str {
        match self {
            Sku::LinuxSlim => "actions_linux_slim",
            Sku::Linux => "actions_linux",
            Sku::LinuxArm => "actions_linux_arm",
            Sku::Windows => "actions_windows",
            Sku::WindowsArm => "actions_windows_arm",
            Sku::Macos => "actions_macos",
            Sku::MacosLarge => "macos_l",
            Sku::MacosXlarge => "macos_xl",
        }
    }

    /// The list price of one minute on this SKU.
    ///
    /// Source: <https://docs.github.com/en/billing/reference/actions-runner-pricing>,
    /// read on 2026-11-07. The billing API gave the same $0.006 for
    /// `Actions Linux`, $0.01 for `Actions Windows`, and $0.062 for
    /// `Actions macOS 3-core` on 2026-11-07.
    #[must_use]
    pub fn list_price(self) -> Usd {
        match self {
            Sku::LinuxSlim => Usd::from_mills(2),
            Sku::Linux => Usd::from_mills(6),
            Sku::LinuxArm => Usd::from_mills(5),
            Sku::Windows | Sku::WindowsArm => Usd::from_mills(10),
            Sku::Macos => Usd::from_mills(62),
            Sku::MacosLarge => Usd::from_mills(77),
            Sku::MacosXlarge => Usd::from_mills(102),
        }
    }

    /// Whether this is a standard runner.
    ///
    /// A standard runner is free on a public repository. A larger runner
    /// (`macos_l`, `macos_xl`) is billed also on a public repository. The
    /// pricing page says: "Larger runners are not free for public
    /// repositories".
    #[must_use]
    pub fn is_standard(self) -> bool {
        !matches!(self, Sku::MacosLarge | Sku::MacosXlarge)
    }

    /// The SKU of a runner label, or `None` when the label is not in the table.
    ///
    /// The match ignores ASCII case. A label that `ha` does not know can name a
    /// larger runner, a custom label, or a label newer than the table.
    ///
    /// Sources: <https://docs.github.com/en/actions/reference/runners/github-hosted-runners>
    /// and <https://docs.github.com/en/actions/reference/runners/larger-runners>,
    /// read on 2026-11-07.
    #[must_use]
    pub fn of_label(label: &str) -> Option<Sku> {
        LABELS
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(label))
            .map(|(_, sku)| *sku)
    }

    /// The SKU that the `sku` text of a billing usage item names.
    ///
    /// This is the one place where the two naming schemes meet. The text
    /// matches when it holds a table name after two changes: the text becomes
    /// lowercase, and each space and `-` becomes `_`. Thus `Actions Linux`,
    /// `actions_linux` and `actions-linux` name `Sku::Linux`. A short list of
    /// extra names covers the rest. The usage endpoint named the macOS SKU
    /// `Actions macOS 3-core` on 2026-11-07, and the first rule cannot reach it.
    ///
    /// Any other text gives `None`, for example `Actions storage`.
    #[must_use]
    pub fn of_billing_name(name: &str) -> Option<Sku> {
        let table_form: String = name
            .chars()
            .map(|c| match c {
                ' ' | '-' => '_',
                other => other.to_ascii_lowercase(),
            })
            .collect();
        Sku::ALL
            .into_iter()
            .find(|sku| sku.table_name() == table_form)
            .or_else(|| {
                BILLING_NAME_EXTRAS
                    .iter()
                    .find(|(known, _)| known.eq_ignore_ascii_case(name))
                    .map(|(_, sku)| *sku)
            })
    }
}

/// The number of seconds in one billed minute.
const SECONDS_PER_MINUTE: u64 = 60;

/// The whole minutes that GitHub bills for a time on a runner.
///
/// GitHub rounds each job up to a whole minute. Zero seconds give zero minutes.
#[must_use]
pub fn billed_minutes(time: Duration) -> u64 {
    time.as_secs().div_ceil(SECONDS_PER_MINUTE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn billed_minutes_round_up_to_a_whole_minute() {
        let cases = [
            (0, 0),
            (1, 1),
            (59, 1),
            (60, 1),
            (61, 2),
            (120, 2),
            (121, 3),
        ];
        for (seconds, minutes) in cases {
            assert_eq!(
                billed_minutes(Duration::from_secs(seconds)),
                minutes,
                "{seconds} s"
            );
        }
    }

    #[test]
    fn every_label_of_the_table_maps_to_its_sku() {
        for (label, sku) in LABELS {
            assert_eq!(Sku::of_label(label), Some(*sku), "{label}");
        }
    }

    #[test]
    fn one_label_of_each_sku_has_a_hand_written_expectation() {
        let cases = [
            ("ubuntu-slim", Sku::LinuxSlim),
            ("ubuntu-latest", Sku::Linux),
            ("ubuntu-24.04-arm", Sku::LinuxArm),
            ("windows-2022", Sku::Windows),
            ("windows-11-arm", Sku::WindowsArm),
            ("macos-15", Sku::Macos),
            ("macos-15-large", Sku::MacosLarge),
            ("macos-latest-xlarge", Sku::MacosXlarge),
        ];
        for (label, sku) in cases {
            assert_eq!(Sku::of_label(label), Some(sku), "{label}");
        }
    }

    #[test]
    fn the_label_table_covers_every_sku() {
        for sku in Sku::ALL {
            assert!(LABELS.iter().any(|(_, known)| *known == sku), "{sku:?}");
        }
    }

    #[test]
    fn a_label_matches_without_regard_to_case() {
        assert_eq!(Sku::of_label("Ubuntu-Latest"), Some(Sku::Linux));
        assert_eq!(Sku::of_label("MACOS-15-XLARGE"), Some(Sku::MacosXlarge));
    }

    #[test]
    fn an_unknown_label_has_no_sku() {
        for label in [
            "ubuntu-latest-8-cores",
            "linux-arm64-gpu",
            "self-hosted",
            "",
        ] {
            assert_eq!(Sku::of_label(label), None, "{label}");
        }
    }

    #[test]
    fn list_prices_follow_the_pricing_page() {
        let cases = [
            (Sku::LinuxSlim, 2),
            (Sku::Linux, 6),
            (Sku::LinuxArm, 5),
            (Sku::Windows, 10),
            (Sku::WindowsArm, 10),
            (Sku::Macos, 62),
            (Sku::MacosLarge, 77),
            (Sku::MacosXlarge, 102),
        ];
        for (sku, mills) in cases {
            assert_eq!(sku.list_price(), Usd::from_mills(mills), "{sku:?}");
        }
    }

    #[test]
    fn only_the_larger_macos_runners_are_not_standard() {
        for sku in Sku::ALL {
            let larger = matches!(sku, Sku::MacosLarge | Sku::MacosXlarge);
            assert_eq!(sku.is_standard(), !larger, "{sku:?}");
        }
    }

    #[test]
    fn table_names_follow_the_pricing_table() {
        let cases = [
            (Sku::LinuxSlim, "actions_linux_slim"),
            (Sku::Linux, "actions_linux"),
            (Sku::LinuxArm, "actions_linux_arm"),
            (Sku::Windows, "actions_windows"),
            (Sku::WindowsArm, "actions_windows_arm"),
            (Sku::Macos, "actions_macos"),
            (Sku::MacosLarge, "macos_l"),
            (Sku::MacosXlarge, "macos_xl"),
        ];
        for (sku, name) in cases {
            assert_eq!(sku.table_name(), name);
        }
    }

    #[test]
    fn a_billing_name_maps_to_a_sku() {
        let cases = [
            ("Actions Linux", Sku::Linux),
            ("Actions Windows", Sku::Windows),
            ("Actions macOS 3-core", Sku::Macos),
            ("actions macos 3-core", Sku::Macos),
            ("actions_linux", Sku::Linux),
            ("actions-linux", Sku::Linux),
            ("Actions Linux Arm", Sku::LinuxArm),
        ];
        for (name, sku) in cases {
            assert_eq!(Sku::of_billing_name(name), Some(sku), "{name}");
        }
    }

    #[test]
    fn a_billing_name_that_is_not_a_runner_has_no_sku() {
        for name in ["Actions storage", "Git LFS storage", "", "Actions"] {
            assert_eq!(Sku::of_billing_name(name), None, "{name:?}");
        }
    }

    #[test]
    fn every_table_name_maps_back_to_its_sku() {
        for sku in Sku::ALL {
            assert_eq!(Sku::of_billing_name(sku.table_name()), Some(sku));
        }
    }
}
