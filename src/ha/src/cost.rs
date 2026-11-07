//! The list-price cost of one job and of one run (issue #639).
//!
//! GitHub gives no cost for one job. The `timing` endpoint is closing down,
//! and its `billable` data is empty. Thus the cost here is an estimate at list
//! price, from data that `ha` already reads: the runner labels of the job, its
//! time on a runner, and the visibility of its repository.
//!
//! The estimate is not the bill. The free minutes that an account includes can
//! make the bill $0.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::job::Job;
use crate::money::Usd;
use crate::run::Visibility;
use crate::target::OwnerName;

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

/// The label of a runner that the user hosts.
const SELF_HOSTED_LABEL: &str = "self-hosted";

/// The `pricePerUnit` of each SKU that the billing data of one account names.
///
/// A price here wins over the list price of the table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Prices(BTreeMap<Sku, Usd>);

impl Prices {
    /// Sets the price of one minute on a SKU. A new price replaces an older
    /// price of the same SKU.
    pub fn set(&mut self, sku: Sku, price: Usd) {
        self.0.insert(sku, price);
    }

    /// The price of one minute on a SKU, or `None` when the billing data does
    /// not name the SKU.
    #[must_use]
    pub fn get(&self, sku: Sku) -> Option<Usd> {
        self.0.get(&sku).copied()
    }
}

/// The billing prices of each account that `ha` could read.
///
/// An account that is not in the book uses the price table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PriceBook(BTreeMap<OwnerName, Prices>);

impl PriceBook {
    /// Stores the prices of one account. New prices replace older prices of
    /// the same account.
    pub fn insert(&mut self, owner: OwnerName, prices: Prices) {
        self.0.insert(owner, prices);
    }

    /// The prices of one account, or `None` when the book does not hold them.
    /// The match ignores the case of the name.
    #[must_use]
    pub fn of(&self, owner: &OwnerName) -> Option<&Prices> {
        self.0.get(owner)
    }
}

/// The estimated cost of one job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCost {
    /// The cost is known. It can be zero.
    Known(Usd),
    /// The job ran on a runner that `ha` cannot price.
    Unknown,
}

/// The list-price cost of one job.
///
/// The rules apply in this order:
///
/// 1. A `self-hosted` label gives zero. GitHub announced a fee of $0.002 per
///    minute for self-hosted runners on 2025-12-16 and then postponed it. The
///    pricing page gives no rate.
/// 2. A job with no runner gives zero.
/// 3. A job with no runner time gives zero. A skipped job and a waiting job
///    are examples.
/// 4. The SKU is the first label that `Sku::of_label` knows. A job with no
///    such label gives `Unknown`. The runner can be a larger runner, or its
///    label can be newer than the table. `ha` never guesses a price.
/// 5. A standard runner on a public repository gives zero.
/// 6. Else the cost is the price of one minute times the billed minutes. The
///    price comes from `prices` when it names the SKU, else from the table.
///
/// A job that runs uses the time until `now`. Its cost thus grows at the
/// start of each minute.
#[must_use]
pub fn job_cost(
    job: &Job,
    visibility: Visibility,
    prices: Option<&Prices>,
    now: DateTime<Utc>,
) -> JobCost {
    let zero = JobCost::Known(Usd::ZERO);
    if job
        .labels
        .iter()
        .any(|label| label.eq_ignore_ascii_case(SELF_HOSTED_LABEL))
    {
        return zero;
    }
    if job.runner.is_none() {
        return zero;
    }
    let Some(time) = job.runner_time(now) else {
        return zero;
    };
    let Some(sku) = job.labels.iter().find_map(|label| Sku::of_label(label)) else {
        return JobCost::Unknown;
    };
    if visibility == Visibility::Public && sku.is_standard() {
        return zero;
    }
    let price = prices
        .and_then(|known| known.get(sku))
        .unwrap_or_else(|| sku.list_price());
    JobCost::Known(price * billed_minutes(time))
}

/// The estimated cost of one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunCost {
    /// The sum of the known costs of the jobs.
    pub amount: Usd,
    /// Whether at least one job has an unknown cost. Then `amount` is a lower
    /// bound.
    pub partial: bool,
}

/// The list-price cost of one run: the sum of the costs of its jobs.
///
/// An empty slice gives `None`. A run whose jobs did not load shows an empty
/// cell. A job with an unknown cost adds nothing to the amount and sets
/// `partial`.
#[must_use]
pub fn run_cost(
    jobs: &[Job],
    visibility: Visibility,
    prices: Option<&Prices>,
    now: DateTime<Utc>,
) -> Option<RunCost> {
    let _ = (jobs, visibility, prices, now);
    None
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

    use chrono::TimeZone;

    use crate::job::{JobId, RunnerName};
    use crate::run::RunState;

    fn at(hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 5, hour, minute, second)
            .unwrap()
    }

    fn job(labels: &[&str], state: RunState, seconds: Option<i64>) -> Job {
        let start = at(12, 30, 14);
        Job {
            id: JobId(1),
            name: "build".to_owned(),
            state,
            runner: RunnerName::new("GitHub Actions 1000001133".to_owned()),
            labels: labels.iter().map(|l| (*l).to_owned()).collect(),
            created_at: Some(start),
            started_at: seconds.map(|_| start),
            completed_at: seconds.map(|s| start + chrono::Duration::seconds(s)),
            url: String::new(),
        }
    }

    /// The sample job of issue 639: 26 minutes 17 seconds, 27 billed minutes.
    fn sample_job() -> Job {
        let mut sample = job(&["ubuntu-latest"], RunState::Passed, None);
        sample.started_at = Some(Utc.with_ymd_and_hms(2026, 11, 5, 12, 30, 14).unwrap());
        sample.completed_at = Some(Utc.with_ymd_and_hms(2026, 11, 5, 12, 56, 31).unwrap());
        sample
    }

    fn owner(name: &str) -> OwnerName {
        OwnerName::parse(name).unwrap()
    }

    fn known(mills: u32, minutes: u64) -> JobCost {
        JobCost::Known(Usd::from_mills(mills) * minutes)
    }

    #[test]
    fn prices_hold_one_price_per_sku_and_the_newest_wins() {
        let mut prices = Prices::default();
        assert_eq!(prices.get(Sku::Linux), None);
        prices.set(Sku::Linux, Usd::from_mills(8));
        assert_eq!(prices.get(Sku::Linux), Some(Usd::from_mills(8)));
        prices.set(Sku::Linux, Usd::from_mills(9));
        assert_eq!(prices.get(Sku::Linux), Some(Usd::from_mills(9)));
        assert_eq!(prices.get(Sku::Windows), None);
    }

    #[test]
    fn a_price_book_finds_an_account_without_regard_to_case() {
        let mut prices = Prices::default();
        prices.set(Sku::Linux, Usd::from_mills(8));
        let mut book = PriceBook::default();
        assert_eq!(book.of(&owner("Octo")), None);
        book.insert(owner("Octo"), prices.clone());
        assert_eq!(book.of(&owner("octo")), Some(&prices));
        assert_eq!(book.of(&owner("other")), None);
    }

    #[test]
    fn the_sample_job_costs_27_minutes_at_the_linux_price() {
        let cost = job_cost(&sample_job(), Visibility::Private, None, at(13, 0, 0));
        assert_eq!(cost, known(6, 27));
        assert_eq!(cost, JobCost::Known(Usd::from_mills(162)));
    }

    #[test]
    fn a_self_hosted_job_is_free() {
        let hosted = job(&["self-hosted", "linux"], RunState::Passed, Some(600));
        let cost = job_cost(&hosted, Visibility::Private, None, at(13, 0, 0));
        assert_eq!(cost, JobCost::Known(Usd::ZERO));
        let shouting = job(&["Self-Hosted"], RunState::Passed, Some(600));
        let cost = job_cost(&shouting, Visibility::Private, None, at(13, 0, 0));
        assert_eq!(cost, JobCost::Known(Usd::ZERO));
    }

    #[test]
    fn a_job_with_no_runner_is_free() {
        let mut unassigned = job(&["ubuntu-latest"], RunState::Passed, Some(600));
        unassigned.runner = None;
        let cost = job_cost(&unassigned, Visibility::Private, None, at(13, 0, 0));
        assert_eq!(cost, JobCost::Known(Usd::ZERO));
    }

    #[test]
    fn a_job_with_no_runner_time_is_free() {
        let skipped = job(&["ubuntu-latest"], RunState::Skipped, None);
        let cost = job_cost(&skipped, Visibility::Private, None, at(13, 0, 0));
        assert_eq!(cost, JobCost::Known(Usd::ZERO));
        let waiting = job(&["ubuntu-latest"], RunState::Queued, Some(0));
        let cost = job_cost(&waiting, Visibility::Private, None, at(13, 0, 0));
        assert_eq!(cost, JobCost::Known(Usd::ZERO));
    }

    #[test]
    fn an_unknown_label_on_a_private_repository_is_unknown() {
        let big = job(&["ubuntu-latest-8-cores"], RunState::Passed, Some(600));
        let cost = job_cost(&big, Visibility::Private, None, at(13, 0, 0));
        assert_eq!(cost, JobCost::Unknown);
        let none = job(&[], RunState::Passed, Some(600));
        assert_eq!(
            job_cost(&none, Visibility::Private, None, at(13, 0, 0)),
            JobCost::Unknown
        );
    }

    #[test]
    fn the_first_known_label_decides_the_sku() {
        let mixed = job(
            &["custom", "windows-latest", "ubuntu-latest"],
            RunState::Passed,
            Some(60),
        );
        let cost = job_cost(&mixed, Visibility::Private, None, at(13, 0, 0));
        assert_eq!(cost, known(10, 1));
    }

    #[test]
    fn a_standard_runner_on_a_public_repository_is_free() {
        let standard = job(&["ubuntu-latest"], RunState::Passed, Some(600));
        let cost = job_cost(&standard, Visibility::Public, None, at(13, 0, 0));
        assert_eq!(cost, JobCost::Known(Usd::ZERO));
    }

    #[test]
    fn a_larger_runner_on_a_public_repository_is_billed() {
        let large = job(&["macos-latest-xlarge"], RunState::Passed, Some(600));
        let cost = job_cost(&large, Visibility::Public, None, at(13, 0, 0));
        assert_eq!(cost, known(102, 10));
    }

    #[test]
    fn an_unknown_label_on_a_public_repository_is_unknown() {
        let big = job(&["linux-arm64-gpu"], RunState::Passed, Some(600));
        let cost = job_cost(&big, Visibility::Public, None, at(13, 0, 0));
        assert_eq!(cost, JobCost::Unknown);
    }

    #[test]
    fn a_billing_price_wins_over_the_table() {
        let mut prices = Prices::default();
        prices.set(Sku::Linux, Usd::from_mills(8));
        let cost = job_cost(
            &sample_job(),
            Visibility::Private,
            Some(&prices),
            at(13, 0, 0),
        );
        assert_eq!(cost, known(8, 27));
    }

    #[test]
    fn the_table_prices_a_sku_that_the_billing_data_does_not_name() {
        let mut prices = Prices::default();
        prices.set(Sku::Windows, Usd::from_mills(20));
        let cost = job_cost(
            &sample_job(),
            Visibility::Private,
            Some(&prices),
            at(13, 0, 0),
        );
        assert_eq!(cost, known(6, 27));
        let cost = job_cost(&sample_job(), Visibility::Private, None, at(13, 0, 0));
        assert_eq!(cost, known(6, 27));
    }

    #[test]
    fn a_running_job_costs_one_more_minute_after_each_minute() {
        let running = job(&["ubuntu-latest"], RunState::Running, None);
        let mut running = running;
        running.started_at = Some(at(12, 30, 14));
        let cost = |seconds: i64| {
            let now = at(12, 30, 14) + chrono::Duration::seconds(seconds);
            job_cost(&running, Visibility::Private, None, now)
        };
        assert_eq!(cost(0), known(6, 0));
        assert_eq!(cost(1), known(6, 1));
        assert_eq!(cost(60), known(6, 1));
        assert_eq!(cost(61), known(6, 2));
    }

    fn run_of(jobs: &[Job], visibility: Visibility) -> Option<RunCost> {
        run_cost(jobs, visibility, None, at(13, 0, 0))
    }

    #[test]
    fn a_run_costs_the_sum_of_its_known_jobs() {
        let jobs = [
            sample_job(),
            job(&["windows-latest"], RunState::Passed, Some(120)),
        ];
        let cost = run_of(&jobs, Visibility::Private).unwrap();
        assert_eq!(
            cost,
            RunCost {
                amount: Usd::from_mills(162) + Usd::from_mills(20),
                partial: false,
            }
        );
    }

    #[test]
    fn a_run_with_an_unknown_job_is_a_lower_bound() {
        let jobs = [
            sample_job(),
            job(&["ubuntu-latest-8-cores"], RunState::Passed, Some(600)),
        ];
        let cost = run_of(&jobs, Visibility::Private).unwrap();
        assert_eq!(
            cost,
            RunCost {
                amount: Usd::from_mills(162),
                partial: true,
            }
        );
    }

    #[test]
    fn a_run_with_no_jobs_has_no_cost() {
        assert_eq!(run_of(&[], Visibility::Private), None);
    }

    #[test]
    fn a_run_of_only_unknown_jobs_is_a_zero_lower_bound() {
        let jobs = [job(&["linux-arm64-gpu"], RunState::Passed, Some(60))];
        let cost = run_of(&jobs, Visibility::Private).unwrap();
        assert_eq!(
            cost,
            RunCost {
                amount: Usd::ZERO,
                partial: true,
            }
        );
    }

    #[test]
    fn a_run_uses_the_billing_prices_and_the_visibility() {
        let mut prices = Prices::default();
        prices.set(Sku::Linux, Usd::from_mills(8));
        let jobs = [sample_job()];
        let billed = run_cost(&jobs, Visibility::Private, Some(&prices), at(13, 0, 0));
        assert_eq!(billed.unwrap().amount, Usd::from_mills(8) * 27);
        let free = run_cost(&jobs, Visibility::Public, Some(&prices), at(13, 0, 0));
        assert_eq!(free.unwrap().amount, Usd::ZERO);
    }
}
