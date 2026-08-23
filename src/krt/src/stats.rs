//! The aggregate fold of a run: every round of a recorded file, in one table.
//!
//! A round record names the TTLs that the round probed, and it holds one hop
//! for each TTL that answered. This module folds those rounds into one row for
//! each TTL, and one entry for each address that answered at a TTL. Every row
//! carries the count of the probes, the count of the answers, and the
//! statistics of the round trip times. The fold reads the records of the run
//! and nothing else, so a test drives it without a network and without a
//! privilege.

use crate::record;
use std::collections::{BTreeMap, VecDeque};
use std::net::IpAddr;

/// The whole of a percentage.
///
/// The loss and the share are both a part of a whole, and both of them report
/// that part as a percentage.
const PERCENT: f64 = 100.0;

/// The number of round-trip times that one key keeps.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
    )
)]
const RECENT_CAPACITY: usize = 60;

/// Reads a count as the number that an arithmetic of this module takes.
///
/// The mean, the loss, and the share each divide by a count, so each of them
/// needs the count as a number with a fraction.
#[expect(
    clippy::cast_precision_loss,
    reason = "an `f64` holds every whole number below 2^53, and a probe run counts one answer for one TTL of one round, so no count of a run reaches that point"
)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
    )
)]
fn count_as_f64(count: u64) -> f64 {
    count as f64
}

/// The statistics of one key, over the round-trip times it observed.
///
/// A key is one TTL of the path, or one address that answered at a TTL.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
    )
)]
#[derive(Debug, Clone, Default)]
pub(crate) struct HopStats {
    /// The number of round-trip times that the key observed.
    recv: u64,
    /// The most recent round-trip time.
    last: Option<f64>,
    /// The round-trip time before the most recent one.
    previous: Option<f64>,
    /// The smallest round-trip time.
    min: Option<f64>,
    /// The largest round-trip time.
    max: Option<f64>,
    /// The arithmetic mean of every round-trip time.
    mean: f64,
    /// The sum of the squared distances from the mean, as Welford keeps it.
    m2: f64,
    /// The last `RECENT_CAPACITY` round-trip times, oldest first.
    recent: VecDeque<f64>,
}

impl HopStats {
    /// Folds one more round-trip time into the statistics.
    ///
    /// The mean and the standard deviation come from Welford's online
    /// algorithm. The algorithm holds the mean and the sum of the squared
    /// distances in constant memory, and it stays stable over millions of
    /// samples. A naive sum of the squares loses precision on a long run,
    /// because that sum grows large while the distances between the samples
    /// stay small.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    fn observe(&mut self, rtt_ms: f64) {
        self.recv += 1;
        self.previous = self.last;
        self.last = Some(rtt_ms);
        self.min = Some(self.min.map_or(rtt_ms, |min| min.min(rtt_ms)));
        self.max = Some(self.max.map_or(rtt_ms, |max| max.max(rtt_ms)));

        // Welford's online algorithm. The mean moves toward the new sample by
        // one part in the count, and the sum of the squared distances takes the
        // product of the distance from the old mean and the distance from the
        // new one.
        let count = count_as_f64(self.recv);
        let from_old_mean = rtt_ms - self.mean;
        self.mean += from_old_mean / count;
        self.m2 += from_old_mean * (rtt_ms - self.mean);

        // The buffer holds a bounded amount of memory, so a run of any length
        // holds the same amount.
        if self.recent.len() == RECENT_CAPACITY {
            self.recent.pop_front();
        }
        self.recent.push_back(rtt_ms);
    }

    /// The number of round-trip times that the key observed.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn recv(&self) -> u64 {
        self.recv
    }

    /// The most recent round-trip time. A key with no sample holds none.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn last(&self) -> Option<f64> {
        self.last
    }

    /// The smallest round-trip time. A key with no sample holds none.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn min(&self) -> Option<f64> {
        self.min
    }

    /// The arithmetic mean of the round-trip times. A key with no sample holds
    /// none.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn avg(&self) -> Option<f64> {
        (self.recv > 0).then_some(self.mean)
    }

    /// The largest round-trip time. A key with no sample holds none.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn max(&self) -> Option<f64> {
        self.max
    }

    /// The population standard deviation of the round-trip times. A key with no
    /// sample holds none.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn stddev(&self) -> Option<f64> {
        (self.recv > 0).then(|| (self.m2 / count_as_f64(self.recv)).sqrt())
    }

    /// The absolute difference between the last two round-trip times. A key
    /// with one sample or none holds none.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn jitter(&self) -> Option<f64> {
        let last = self.last?;
        let previous = self.previous?;
        Some((last - previous).abs())
    }

    /// The last `RECENT_CAPACITY` round-trip times, oldest first.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn recent(&self) -> impl ExactSizeIterator<Item = f64> + '_ {
        self.recent.iter().copied()
    }
}

/// The aggregate view of every hop seen so far, keyed and ordered by TTL.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
    )
)]
#[derive(Debug, Clone, Default)]
pub(crate) struct HopTable {
    /// One row for each TTL that a round probed, and for each TTL that a hop
    /// answered at. The map holds the rows in TTL order.
    rows: BTreeMap<u8, TtlRow>,
}

impl HopTable {
    /// Builds a table that holds no row.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Folds one more round into the table.
    ///
    /// Every TTL that the round probed takes one more probe, whether it
    /// answered or not, so the loss of a TTL counts the rounds that reached it.
    /// Every hop of the round then folds its round-trip time twice: once into
    /// the statistics of the TTL, and once into the statistics of the address
    /// that answered.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn observe(&mut self, round: &record::RoundRecord) {
        for ttl in round.ttl_range.first()..=round.ttl_range.last() {
            self.row_mut(ttl).sent += 1;
        }
        for hop in &round.hops {
            let row = self.row_mut(hop.ttl);
            row.stats.observe(hop.rtt_ms);
            row.address_mut(hop.addr).observe(hop.rtt_ms);
        }
    }

    /// The row of one TTL. The row of a TTL that the table never saw is made
    /// here.
    fn row_mut(&mut self, ttl: u8) -> &mut TtlRow {
        self.rows.entry(ttl).or_insert_with(|| TtlRow::new(ttl))
    }

    /// Every TTL row, in TTL order.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn rows(&self) -> impl ExactSizeIterator<Item = &TtlRow> {
        self.rows.values()
    }

    /// The row of one TTL. A TTL the table never saw gives `None`.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn row(&self, ttl: u8) -> Option<&TtlRow> {
        self.rows.get(&ttl)
    }
}

/// One hop position on the path, over every answer it gave.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
    )
)]
#[derive(Debug, Clone)]
pub(crate) struct TtlRow {
    /// The TTL of the row.
    ttl: u8,
    /// The number of rounds whose range covered this TTL.
    sent: u64,
    /// The statistics over every answer at this TTL.
    stats: HopStats,
    /// Every address that answered at this TTL, in the order the TTL first saw
    /// them, each with the statistics of its own answers.
    ///
    /// One TTL sees a handful of routers, so a scan of the whole list costs
    /// less than a map that keeps the order beside the keys.
    addresses: Vec<(IpAddr, HopStats)>,
}

impl TtlRow {
    /// Builds the row of one TTL, before any round reaches it.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    fn new(ttl: u8) -> Self {
        Self {
            ttl,
            sent: 0,
            stats: HopStats::default(),
            addresses: Vec::new(),
        }
    }

    /// The statistics of one address of this row. The entry of an address that
    /// this row never saw is made here.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    fn address_mut(&mut self, addr: IpAddr) -> &mut HopStats {
        let found = self.addresses.iter().position(|(held, _)| *held == addr);
        let index = if let Some(index) = found {
            index
        } else {
            // The address is new to this TTL, so its entry goes at the end.
            // The order of the list then stays the order of the first answers.
            self.addresses.push((addr, HopStats::default()));
            self.addresses.len() - 1
        };
        &mut self.addresses[index].1
    }

    /// The TTL of the row.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn ttl(&self) -> u8 {
        self.ttl
    }

    /// The rounds whose `ttl_range` covered this TTL.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn sent(&self) -> u64 {
        self.sent
    }

    /// The statistics over every answer at this TTL, whichever address gave it.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn stats(&self) -> &HopStats {
        &self.stats
    }

    /// The loss of this position, as a percentage. A TTL that no round probed
    /// gives `None`.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn loss(&self) -> Option<f64> {
        if self.sent == 0 {
            return None;
        }
        // A TTL that answered more times than it was probed floors at no loss.
        // No fold of a well formed file reaches that state, and a percentage
        // below zero would read as a defect of the tool.
        let lost = self.sent.saturating_sub(self.stats.recv());
        Some(count_as_f64(lost) / count_as_f64(self.sent) * PERCENT)
    }

    /// Every address that answered at this TTL, in the order the TTL first saw
    /// them, each with the share of the answers it took.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn addresses(&self) -> impl ExactSizeIterator<Item = Address<'_>> {
        let answers = self.stats.recv();
        self.addresses.iter().map(move |(addr, stats)| Address {
            addr: *addr,
            stats,
            answers,
        })
    }
}

/// One router that answered at a TTL, and the share of that TTL's answers it
/// took.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
    )
)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Address<'a> {
    /// The address that answered.
    addr: IpAddr,
    /// The statistics over the answers of this address.
    stats: &'a HopStats,
    /// The number of answers of the whole TTL, from every address of it.
    answers: u64,
}

impl Address<'_> {
    /// The address that answered.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn addr(&self) -> IpAddr {
        self.addr
    }

    /// The statistics over the answers of this address.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn stats(&self) -> &HopStats {
        self.stats
    }

    /// The share of the answers of the TTL that this address took, as a
    /// percentage.
    ///
    /// The divisor is never zero. This entry exists because the address
    /// answered at the TTL, so the TTL holds one answer at least.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    pub(crate) fn share(&self) -> f64 {
        count_as_f64(self.stats.recv()) / count_as_f64(self.answers) * PERCENT
    }
}

#[cfg(test)]
mod tests {
    use super::{HopStats, HopTable, TtlRow, RECENT_CAPACITY};
    use crate::record::{Hop, RoundRecord, RunId, TtlRange};
    use chrono::{DateTime, Utc};
    use std::net::IpAddr;

    /// The largest difference that a comparison of two round-trip times admits.
    ///
    /// Every number of these tests is a small decimal, and the fold adds and
    /// divides them, so the answer lands within a few units of the last place.
    const TOLERANCE: f64 = 1e-9;

    /// A sample set whose hand-computed statistics the tests state.
    ///
    /// The minimum is 10.0, the maximum is 40.0, and the sum is
    /// 10 + 20 + 30 + 40 = 100. The mean is therefore 100 / 4 = 25.0.
    const FOUR_SAMPLES: [f64; 4] = [10.0, 20.0, 30.0, 40.0];

    /// A sample set whose population standard deviation is exactly 2.0.
    ///
    /// The sum is 2 + 4 + 4 + 4 + 5 + 5 + 7 + 9 = 40, so the mean is
    /// 40 / 8 = 5.0. The distances from the mean are -3, -1, -1, -1, 0, 0, 2,
    /// and 4. The squares of them are 9, 1, 1, 1, 0, 0, 4, and 16, and they
    /// sum to 32. The population variance is 32 / 8 = 4.0, so the standard
    /// deviation is the square root of 4.0, which is 2.0.
    const EIGHT_SAMPLES: [f64; 8] = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];

    /// The number of samples that the ring buffer test folds.
    const MANY: u32 = 100;

    /// Folds every sample into one set of statistics.
    fn stats_of(samples: &[f64]) -> HopStats {
        let mut stats = HopStats::default();
        for sample in samples {
            stats.observe(*sample);
        }
        stats
    }

    /// Asserts that a statistic holds a value, and that the value is the one
    /// the test computed by hand.
    fn holds(actual: Option<f64>, expected: f64, name: &str) {
        let actual = actual.unwrap_or_else(|| panic!("the {name} must hold a value"));
        assert!(
            (actual - expected).abs() < TOLERANCE,
            "the {name} is {expected}, and the fold gives {actual}"
        );
    }

    #[test]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    fn a_key_with_no_sample_holds_no_statistic() {
        let stats = HopStats::default();
        assert_eq!(stats.recv(), 0, "no sample is no answer");
        assert_eq!(stats.last(), None, "no sample is no last time");
        assert_eq!(stats.min(), None, "no sample is no smallest time");
        assert_eq!(stats.avg(), None, "no sample is no mean");
        assert_eq!(stats.max(), None, "no sample is no largest time");
        assert_eq!(stats.stddev(), None, "no sample is no deviation");
        assert_eq!(stats.jitter(), None, "no sample is no jitter");
        assert_eq!(stats.recent().len(), 0, "no sample is no history");
    }

    #[test]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    fn the_statistics_of_a_known_sample_set_are_the_hand_computed_ones() {
        let stats = stats_of(&FOUR_SAMPLES);
        assert_eq!(stats.recv(), 4, "the fold took four samples");
        holds(stats.min(), 10.0, "smallest time");
        holds(stats.avg(), 25.0, "mean");
        holds(stats.max(), 40.0, "largest time");
        holds(stats.last(), 40.0, "last time");
    }

    #[test]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    fn the_deviation_of_a_known_sample_set_is_the_population_one() {
        let stats = stats_of(&EIGHT_SAMPLES);
        assert_eq!(stats.recv(), 8, "the fold took eight samples");
        holds(stats.avg(), 5.0, "mean");
        holds(stats.stddev(), 2.0, "population standard deviation");
    }

    #[test]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    fn one_sample_holds_a_deviation_of_zero() {
        let stats = stats_of(&[12.5]);
        assert_eq!(stats.recv(), 1, "the fold took one sample");
        holds(stats.stddev(), 0.0, "standard deviation of one sample");
    }

    #[test]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    fn the_jitter_is_the_absolute_difference_of_the_last_two_samples() {
        holds(stats_of(&[10.0, 15.0]).jitter(), 5.0, "jitter of a rise");
        holds(stats_of(&[15.0, 10.0]).jitter(), 5.0, "jitter of a fall");
        holds(
            stats_of(&[10.0, 40.0, 42.0]).jitter(),
            2.0,
            "jitter of the last two of three samples",
        );
    }

    #[test]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    fn one_sample_holds_no_jitter() {
        let stats = stats_of(&[12.5]);
        assert_eq!(
            stats.jitter(),
            None,
            "one sample names no last two round-trip times"
        );
    }

    #[test]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the replay slice of issue #369 wires the fold into `krt replay`, so the tests of this module are the one reader today"
        )
    )]
    fn the_history_holds_the_last_sixty_samples_in_order() {
        let samples: Vec<f64> = (1..=MANY).map(f64::from).collect();
        let stats = stats_of(&samples);
        assert_eq!(stats.recv(), u64::from(MANY), "the fold took every sample");
        let history: Vec<f64> = stats.recent().collect();
        assert_eq!(
            history.len(),
            RECENT_CAPACITY,
            "the history keeps {RECENT_CAPACITY} samples"
        );
        let expected: Vec<f64> = samples[samples.len() - RECENT_CAPACITY..].to_vec();
        assert_eq!(history, expected, "the history keeps the last samples");
    }

    /// The identifier of the run that every test round belongs to.
    const RUN: &str = "2026-08-18T12:00:00.000Z";

    /// The moment of every test round.
    const MOMENT: &str = "2026-08-18T12:00:01.000Z";

    /// The time that every test round took, in milliseconds.
    const ROUND_DURATION: u64 = 1000;

    /// The name of the ICMP message of every test hop.
    const TIME_EXCEEDED: &str = "time_exceeded";

    /// The address of the first router of the test path.
    const FIRST_HOP: &str = "192.168.1.1";

    /// The address of one of the two routers that answer at one TTL.
    const LEFT_ROUTER: &str = "10.0.0.1";

    /// The address of the other of the two routers that answer at one TTL.
    const RIGHT_ROUTER: &str = "10.0.0.2";

    /// The address of the target of the test path.
    const TARGET: &str = "93.184.216.34";

    /// The round-trip time of a hop that a test does not read.
    const ANY_RTT: f64 = 1.5;

    /// Reads an address that a test names.
    fn address(text: &str) -> IpAddr {
        text.parse().expect("the test address must parse")
    }

    /// One round that probed the TTLs of the range, and that the named hops
    /// answered.
    ///
    /// Each hop is a TTL, the address that answered at it, and the round-trip
    /// time of that answer.
    fn round(first: u8, last: u8, hops: &[(u8, &str, f64)]) -> RoundRecord {
        RoundRecord {
            run: RunId::from(RUN),
            seq: 1,
            ts: DateTime::parse_from_rfc3339(MOMENT)
                .expect("the test moment must parse")
                .with_timezone(&Utc),
            dur_ms: ROUND_DURATION,
            ttl_range: TtlRange::new(first, last).expect("the test range must hold"),
            reached: false,
            hops: hops
                .iter()
                .map(|(ttl, addr, rtt_ms)| Hop {
                    ttl: *ttl,
                    addr: address(addr),
                    rtt_ms: *rtt_ms,
                    icmp: TIME_EXCEEDED.to_owned(),
                })
                .collect(),
        }
    }

    /// Folds every round into one table.
    fn table_of(rounds: &[RoundRecord]) -> HopTable {
        let mut table = HopTable::new();
        for round in rounds {
            table.observe(round);
        }
        table
    }

    /// The row of one TTL that the table must hold.
    fn row_of(table: &HopTable, ttl: u8) -> &TtlRow {
        table
            .row(ttl)
            .unwrap_or_else(|| panic!("the table must hold the row of ttl {ttl}"))
    }

    /// The TTL of every row of the table, in the order the table gives them.
    fn ttls_of(table: &HopTable) -> Vec<u8> {
        table.rows().map(TtlRow::ttl).collect()
    }

    #[test]
    fn the_probes_of_a_ttl_count_the_rounds_that_covered_it() {
        // Round one covers TTL 1 to 3, round two covers TTL 1 to 5, and round
        // three covers TTL 2 to 4. TTL 1 is therefore in two rounds, TTL 2 and
        // TTL 3 are in three, TTL 4 is in two, and TTL 5 is in one.
        let table = table_of(&[round(1, 3, &[]), round(1, 5, &[]), round(2, 4, &[])]);
        for (ttl, sent) in [(1, 2), (2, 3), (3, 3), (4, 2), (5, 1)] {
            assert_eq!(row_of(&table, ttl).sent(), sent, "the probes of ttl {ttl}");
        }
        assert!(
            table.row(6).is_none(),
            "no round probed ttl 6, so the table holds no row of it"
        );
    }

    /// A run whose target moves closer shrinks the range of its rounds. A TTL
    /// that falls outside the new range takes no probe, so its loss stays where
    /// the earlier rounds left it.
    #[test]
    fn a_ttl_outside_the_range_of_a_round_takes_no_probe_from_it() {
        let table = table_of(&[
            round(1, 5, &[(5, TARGET, ANY_RTT)]),
            round(1, 3, &[(3, FIRST_HOP, ANY_RTT)]),
        ]);
        let five = row_of(&table, 5);
        assert_eq!(five.sent(), 1, "one round of the two covered ttl 5");
        assert_eq!(five.stats().recv(), 1, "ttl 5 answered its one probe");
        holds(five.loss(), 0.0, "loss of ttl 5");
    }

    #[test]
    fn a_ttl_that_never_answers_reaches_a_loss_of_one_hundred_percent() {
        let table = table_of(&[
            round(1, 3, &[(1, FIRST_HOP, ANY_RTT)]),
            round(1, 3, &[(1, FIRST_HOP, ANY_RTT)]),
        ]);
        let two = row_of(&table, 2);
        assert_eq!(two.sent(), 2, "both rounds probed ttl 2");
        assert_eq!(two.stats().recv(), 0, "ttl 2 answered no probe");
        holds(two.loss(), 100.0, "loss of ttl 2");
    }

    #[test]
    fn a_ttl_that_answers_nine_rounds_of_ten_reaches_a_loss_of_ten_percent() {
        let mut rounds: Vec<RoundRecord> = (0..9)
            .map(|_| round(1, 1, &[(1, FIRST_HOP, ANY_RTT)]))
            .collect();
        rounds.push(round(1, 1, &[]));
        let table = table_of(&rounds);
        let one = row_of(&table, 1);
        assert_eq!(one.sent(), 10, "ten rounds probed ttl 1");
        assert_eq!(one.stats().recv(), 9, "nine rounds of the ten answered");
        // One round of the ten answered nothing, so the loss is
        // 1 / 10 * 100 = 10.0 percent.
        holds(one.loss(), 10.0, "loss of ttl 1");
    }

    #[test]
    fn two_addresses_at_one_ttl_share_the_answers_of_that_ttl() {
        // The left router answers three of the four rounds, and the right
        // router answers the other one. The shares are therefore
        // 3 / 4 * 100 = 75.0 and 1 / 4 * 100 = 25.0.
        let table = table_of(&[
            round(1, 2, &[(2, LEFT_ROUTER, 10.0)]),
            round(1, 2, &[(2, RIGHT_ROUTER, 20.0)]),
            round(1, 2, &[(2, LEFT_ROUTER, 30.0)]),
            round(1, 2, &[(2, LEFT_ROUTER, 50.0)]),
        ]);
        let two = row_of(&table, 2);
        assert_eq!(
            two.stats().recv(),
            4,
            "the row of the ttl holds every answer of it"
        );
        holds(two.stats().avg(), 27.5, "mean of ttl 2");

        let addresses: Vec<_> = two.addresses().collect();
        assert_eq!(addresses.len(), 2, "two routers answered at ttl 2");
        assert_eq!(
            addresses[0].addr(),
            address(LEFT_ROUTER),
            "the left router answered first"
        );
        assert_eq!(
            addresses[1].addr(),
            address(RIGHT_ROUTER),
            "the right router answered second"
        );
        assert_eq!(addresses[0].stats().recv(), 3, "the left router answered 3");
        assert_eq!(
            addresses[1].stats().recv(),
            1,
            "the right router answered 1"
        );
        assert_eq!(
            addresses[0].stats().recv() + addresses[1].stats().recv(),
            two.stats().recv(),
            "every answer of the ttl belongs to one address of it"
        );
        holds(Some(addresses[0].share()), 75.0, "share of the left router");
        holds(
            Some(addresses[1].share()),
            25.0,
            "share of the right router",
        );
        let total: f64 = two.addresses().map(|address| address.share()).sum();
        holds(Some(total), 100.0, "sum of the shares of ttl 2");
    }

    #[test]
    fn a_ttl_that_stops_answering_keeps_its_history_and_loses_the_later_rounds() {
        // Three rounds answer with 10, 20, and 30, and two more rounds probe
        // the TTL and get nothing. The sum is 60, so the mean is 60 / 3 = 20.0.
        // Two probes of the five got no answer, so the loss is
        // 2 / 5 * 100 = 40.0 percent.
        let table = table_of(&[
            round(1, 1, &[(1, FIRST_HOP, 10.0)]),
            round(1, 1, &[(1, FIRST_HOP, 20.0)]),
            round(1, 1, &[(1, FIRST_HOP, 30.0)]),
            round(1, 1, &[]),
            round(1, 1, &[]),
        ]);
        let one = row_of(&table, 1);
        assert_eq!(one.sent(), 5, "five rounds probed ttl 1");
        assert_eq!(one.stats().recv(), 3, "three rounds of the five answered");
        holds(one.stats().min(), 10.0, "smallest time of ttl 1");
        holds(one.stats().avg(), 20.0, "mean of ttl 1");
        holds(one.stats().max(), 30.0, "largest time of ttl 1");
        holds(one.stats().last(), 30.0, "last time of ttl 1");
        holds(one.loss(), 40.0, "loss of ttl 1");
    }

    #[test]
    fn the_rows_come_back_in_ttl_order() {
        let table = table_of(&[
            round(3, 3, &[(3, TARGET, ANY_RTT)]),
            round(1, 2, &[(2, FIRST_HOP, ANY_RTT)]),
        ]);
        assert_eq!(table.rows().len(), 3, "the table holds three rows");
        assert_eq!(ttls_of(&table), [1, 2, 3], "the rows run in ttl order");
    }
}
