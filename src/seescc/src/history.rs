//! The timestamped ring buffer that backs the watch view's sparklines.
//!
//! sccache exposes no historical series — every poll is a fresh snapshot — so the
//! only history seescc can ever draw is what *this* process has observed since
//! launch. [`History`] is that observation log: a window-bounded ring of
//! `(Instant, Stats)` samples captured at each poll. It is deliberately
//! **time-based, not count-based**: a sample is retained because it falls inside
//! the configured `window`, never because it is one of the last *N* samples. That
//! is what lets the sparkline decouple poll cadence from column count — a 1 s poll
//! over a 15 m window keeps ~900 samples, which [`History::bucket_last`] then
//! aggregates down into however many columns the terminal can spare.
//!
//! Everything here is driven by a monotonic [`Instant`] supplied by the caller,
//! never the wall clock: monotonic time can't jump backwards across an NTP step
//! or a DST change, and injecting the timestamp keeps the whole module pure and
//! unit-testable without sleeping or reading a real clock.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::stats::Stats;

/// A window-bounded ring of timestamped sccache samples.
///
/// Holds every `(Instant, Stats)` poll observed within the last `window` of
/// wall-time, oldest first. The buffer is the single source of truth the
/// sparkline layer reads from: [`History::push`] feeds a fresh poll in and prunes
/// anything that has aged out, and [`History::bucket_last`] collapses the retained
/// samples into a fixed number of time columns for rendering. Both the window and
/// the samples are private so the only ways to mutate or observe history are the
/// two methods below — callers can never desync the ring from its retention rule.
pub(crate) struct History {
    /// How far back samples are retained. A sample at time `t` survives a later
    /// `push(at, …)` iff `t >= at - window`; older samples are dropped.
    window: Duration,
    /// The retained samples, oldest at the front. Kept ordered by push time
    /// (which is monotonic, so insertion order *is* time order), so pruning is a
    /// cheap pop-from-front of the aged-out prefix and bucketing is a single
    /// forward scan.
    samples: VecDeque<(Instant, Stats)>,
}

impl History {
    /// Create an empty history that retains samples observed within `window`.
    ///
    /// Nothing is sampled yet — the watch view starts with a blank sparkline that
    /// fills in over the first `window` of runtime, matching the design's
    /// "in-memory only, empty at launch" rule.
    pub(crate) fn new(window: Duration) -> Self {
        let _ = window;
        todo!("implemented in the green commit")
    }

    /// Append a sample observed at `at`, then prune everything older than the
    /// window.
    ///
    /// `at` is the monotonic instant the poll completed; because polls happen in
    /// real time `at` is non-decreasing across calls, so the new sample always
    /// belongs at the back. After appending, any sample whose timestamp is
    /// strictly before `at - window` has aged out of the retention window and is
    /// dropped from the front. Pruning relative to the *just-pushed* `at` (rather
    /// than a separate "now") keeps the ring self-maintaining: it never grows
    /// without bound even if `bucket_last` is never called.
    pub(crate) fn push(&mut self, at: Instant, stats: Stats) {
        let _ = (at, stats);
        todo!("implemented in the green commit")
    }

    /// Bucket the retained samples into `columns` equal time slices spanning
    /// `[now - window, now]`, newest slice last.
    ///
    /// Each slice covers `window / columns` of time. Slot `i` holds the **most
    /// recent** sample whose timestamp lands in slice `i`, or `None` when no
    /// sample fell there (a gap the sparkline draws at baseline). The boundary
    /// rules are pinned by tests and must stay exact:
    ///
    /// - a sample exactly at `now` lands in the **last** bucket (`columns - 1`),
    /// - a sample exactly at `now - window` lands in the **first** bucket (`0`),
    /// - samples strictly older than `now - window`, or (defensively) newer than
    ///   `now`, are excluded entirely.
    ///
    /// The slice index is chosen by integer duration arithmetic —
    /// `elapsed_since_window_start / slice_width`, clamped to `columns - 1` — so a
    /// sample sitting exactly on the trailing edge can't round into a phantom
    /// `columns`-th bucket. `columns == 0` yields an empty `Vec` (no room to draw
    /// anything); an empty ring yields a `Vec` of `columns` `None`s.
    pub(crate) fn bucket_last(&self, now: Instant, columns: usize) -> Vec<Option<&Stats>> {
        let _ = (now, columns);
        todo!("implemented in the green commit")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Stats` tagged by `cache_size` so a test can assert *which* sample
    /// landed in a given bucket. The field is otherwise meaningless here — it is
    /// purely a recognizable marker — which keeps the tests independent of the
    /// real sccache payload shape.
    fn tagged(tag: u64) -> Stats {
        Stats {
            cache_size: tag,
            ..Default::default()
        }
    }

    /// The `cache_size` tag of the sample in slot `slot`, or `None` for a gap.
    /// Collapses `Option<&Stats>` to `Option<u64>` so bucket assertions read as
    /// plain "slot N holds tag T" without dereferencing in every line.
    fn tags(buckets: &[Option<&Stats>]) -> Vec<Option<u64>> {
        buckets.iter().map(|s| s.map(|st| st.cache_size)).collect()
    }

    #[test]
    fn push_prunes_samples_older_than_the_window() {
        // A sample that has aged past `at - window` must be dropped, while one
        // still inside the window survives. Observed through bucketing: a single
        // wide bucket over the window shows only the survivor, never the evicted
        // sample. window = 10s; the first sample lands 20s before the final push,
        // so it is 10s past the trailing edge and must be gone.
        let base = Instant::now();
        let window = Duration::from_secs(10);
        let mut history = History::new(window);

        history.push(base, tagged(1)); // t = 0s — will age out
        history.push(base + Duration::from_secs(20), tagged(2)); // t = 20s — current

        // Bucket over [now - window, now] = [10s, 20s] with a single column: only
        // the t=20s sample is in range; the t=0s sample was pruned on push.
        let now = base + Duration::from_secs(20);
        let buckets = history.bucket_last(now, 1);
        assert_eq!(
            tags(&buckets),
            vec![Some(2)],
            "the aged-out sample must be pruned, leaving only the in-window one",
        );
    }

    #[test]
    fn bucket_last_places_known_samples_in_known_slots_newest_wins() {
        // Three samples across a 4-column window must land in their time slices,
        // and when two share a slice the MOST RECENT one wins. window = 4s over 4
        // columns ⇒ 1s slices: [0,1) [1,2) [2,3) [3,4] relative to now-window.
        // Place samples so slot 0 has one, slot 1 has two (newest wins), slot 2 is
        // empty, slot 3 has one at exactly `now`.
        let base = Instant::now();
        let window = Duration::from_secs(4);
        let mut history = History::new(window);
        let now = base + Duration::from_secs(4);

        // now - window = base. Offsets are relative to base.
        history.push(base + Duration::from_millis(0), tagged(10)); // slot 0
        history.push(base + Duration::from_millis(1200), tagged(20)); // slot 1 (older)
        history.push(base + Duration::from_millis(1800), tagged(21)); // slot 1 (newer — wins)
        // slot 2 [2s,3s) intentionally left empty
        history.push(base + Duration::from_secs(4), tagged(40)); // slot 3, exactly `now`

        let buckets = history.bucket_last(now, 4);
        assert_eq!(
            tags(&buckets),
            vec![Some(10), Some(21), None, Some(40)],
            "samples must map to their time slices; newest wins within a slice; \
             empty slices are None",
        );
    }

    #[test]
    fn bucket_last_empty_buffer_is_all_none_of_length_columns() {
        // An empty ring (nothing polled yet — the launch state) must yield a vec
        // of exactly `columns` None entries, so the sparkline draws a full row of
        // baseline gaps rather than a short or panicking slice.
        let history = History::new(Duration::from_secs(10));
        let now = Instant::now();

        let buckets = history.bucket_last(now, 5);
        assert_eq!(
            tags(&buckets),
            vec![None, None, None, None, None],
            "an empty buffer must produce `columns` None entries",
        );
    }

    #[test]
    fn bucket_last_zero_columns_is_empty_vec() {
        // Zero columns means there is no room to draw anything; the result must be
        // an empty vec, never a divide-by-zero panic from the slice-width math.
        let base = Instant::now();
        let mut history = History::new(Duration::from_secs(10));
        history.push(base, tagged(1));

        let buckets = history.bucket_last(base + Duration::from_secs(1), 0);
        assert!(
            buckets.is_empty(),
            "columns == 0 must yield an empty vec, got {:?}",
            tags(&buckets),
        );
    }

    #[test]
    fn bucket_last_boundary_now_lands_in_last_bucket() {
        // A sample whose timestamp equals `now` exactly must land in the final
        // bucket, not round off the end into a phantom `columns`-th slot. window =
        // 3s over 3 columns; a single sample at exactly `now`.
        let base = Instant::now();
        let window = Duration::from_secs(3);
        let mut history = History::new(window);
        let now = base + Duration::from_secs(3);
        history.push(now, tagged(99));

        let buckets = history.bucket_last(now, 3);
        assert_eq!(
            tags(&buckets),
            vec![None, None, Some(99)],
            "a sample exactly at `now` must occupy the last bucket",
        );
    }

    #[test]
    fn bucket_last_boundary_now_minus_window_lands_in_first_bucket() {
        // The opposite edge: a sample exactly at `now - window` is still in range
        // and must occupy the FIRST bucket — the window interval is inclusive of
        // its lower edge.
        let base = Instant::now();
        let window = Duration::from_secs(3);
        let mut history = History::new(window);
        let now = base + Duration::from_secs(3);
        // now - window = base exactly.
        history.push(base, tagged(7));

        let buckets = history.bucket_last(now, 3);
        assert_eq!(
            tags(&buckets),
            vec![Some(7), None, None],
            "a sample exactly at `now - window` must occupy the first bucket",
        );
    }

    #[test]
    fn bucket_last_excludes_samples_outside_the_window() {
        // Defensive range-checking inside bucket_last: samples older than
        // `now - window` (that somehow survived pruning because a smaller window
        // is used at bucket time) and any sample newer than `now` must be excluded
        // from the buckets entirely. We push with a generous window so nothing is
        // pruned, then bucket over a tighter window.
        let base = Instant::now();
        let mut history = History::new(Duration::from_secs(1000)); // keep everything

        history.push(base + Duration::from_secs(0), tagged(1)); // too old for the bucket window
        history.push(base + Duration::from_secs(50), tagged(2)); // in the bucket window
        history.push(base + Duration::from_secs(200), tagged(3)); // newer than `now`

        // Bucket over [now - 10s, now] = [40s, 50s]: only tag 2 is in range. The
        // t=0s sample is below the lower edge; the t=200s sample is above `now`.
        let now = base + Duration::from_secs(50);
        let buckets = history.bucket_last(now, 1);
        assert_eq!(
            tags(&buckets),
            vec![Some(2)],
            "samples below `now - window` or above `now` must be excluded",
        );
    }
}
