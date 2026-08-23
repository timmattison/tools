//! The aggregate fold of a run: every round of a recorded file, in one table.
//!
//! A round record names the TTLs that the round probed, and it holds one hop
//! for each TTL that answered. This module folds those rounds into one row for
//! each TTL, and one entry for each address that answered at a TTL. Every row
//! carries the count of the probes, the count of the answers, and the
//! statistics of the round trip times. The fold reads the records of the run
//! and nothing else, so a test drives it without a network and without a
//! privilege.

use std::collections::VecDeque;

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

#[cfg(test)]
mod tests {
    use super::{HopStats, RECENT_CAPACITY};

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
}
