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

use crate::config::{MetricKey, MetricKind};
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
    /// The fixed anchor of the sparkline's slice grid, captured once at watch
    /// startup. Grid points sit at `epoch + k * slice` (with
    /// `slice = window / columns`), so a sample's bucket assignment is a pure
    /// function of its timestamp and this anchor — it never shifts as `now`
    /// advances within a slice, only when the grid steps to the next `k`. See
    /// [`History::bucket_last`].
    epoch: Instant,
    /// The retained samples, oldest at the front. Kept ordered by push time
    /// (which is monotonic, so insertion order *is* time order), so pruning is a
    /// cheap pop-from-front of the aged-out prefix and bucketing is a single
    /// forward scan.
    samples: VecDeque<(Instant, Stats)>,
}

impl History {
    /// Create an empty history that retains samples observed within `window`,
    /// with its slice grid anchored at `epoch`.
    ///
    /// `epoch` is captured once at watch startup (production passes
    /// `Instant::now()`; tests fabricate it) and never changes for the process
    /// lifetime. It anchors the sparkline's slice grid so a sample's bucket
    /// assignment is stable between frames — see [`History::bucket_last`].
    /// Nothing is sampled yet — the watch view starts with a blank sparkline that
    /// fills in over the first `window` of runtime, matching the design's
    /// "in-memory only, empty at launch" rule.
    pub(crate) fn new(window: Duration, epoch: Instant) -> Self {
        History {
            window,
            epoch,
            samples: VecDeque::new(),
        }
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
        self.samples.push_back((at, stats));
        // Prune the aged-out prefix: drop every front sample older than the
        // trailing edge of the window relative to the just-pushed `at`. A
        // saturating subtraction means a window larger than the elapsed runtime
        // simply keeps everything (the cutoff floors at the epoch `at` itself
        // isn't past), so early samples survive until real time advances.
        let cutoff = at.checked_sub(self.window);
        if let Some(cutoff) = cutoff {
            while let Some(&(timestamp, _)) = self.samples.front() {
                if timestamp < cutoff {
                    self.samples.pop_front();
                } else {
                    break;
                }
            }
        }
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
    /// The slice index is chosen by integer duration arithmetic in nanoseconds —
    /// `elapsed_since_window_start * columns / window`, clamped to `columns - 1`
    /// (see [`History::bucket_index`]) — so a sample sitting exactly on the
    /// trailing edge can't round into a phantom `columns`-th bucket. `columns == 0`
    /// yields an empty `Vec` (no room to draw anything); an empty ring yields a
    /// `Vec` of `columns` `None`s.
    pub(crate) fn bucket_last(&self, now: Instant, columns: usize) -> Vec<Option<&Stats>> {
        // No columns ⇒ no room to draw anything; bail before any slice-width math
        // so the division below can never see a zero divisor.
        if columns == 0 {
            return Vec::new();
        }

        let mut buckets: Vec<Option<&Stats>> = vec![None; columns];

        // The retained samples are ordered oldest-first, so a forward scan visits
        // each slice's candidates in increasing-time order; writing every in-range
        // sample into its slot means the LAST write per slot — the most recent
        // sample — is the one that survives. That is the "newest wins" rule.
        for (timestamp, stats) in &self.samples {
            if let Some(index) = self.bucket_index(now, *timestamp, columns) {
                buckets[index] = Some(stats);
            }
        }

        buckets
    }

    /// The bucket index a sample at `timestamp` belongs to within `columns`
    /// slices spanning `[now - window, now]`, or `None` when it lies outside that
    /// closed interval.
    ///
    /// Index is `elapsed_since_window_start * columns / window`, computed in
    /// nanoseconds so no slice-width truncation accumulates, then clamped to
    /// `columns - 1`. That clamp is what folds a sample sitting exactly on the
    /// trailing edge (`timestamp == now`, where the raw quotient equals `columns`)
    /// back into the final bucket instead of overflowing into a phantom
    /// `columns`-th slot — pinning the "exactly at `now` ⇒ last bucket" boundary.
    /// A sample exactly at `now - window` has zero elapsed time and so lands in
    /// bucket `0`, making the lower edge inclusive. Samples newer than `now` (which
    /// shouldn't happen with monotonic time, but are guarded defensively) are
    /// rejected.
    fn bucket_index(&self, now: Instant, timestamp: Instant, columns: usize) -> Option<usize> {
        // Above the upper edge: a sample newer than `now` is out of range.
        if timestamp > now {
            return None;
        }
        // How far the sample sits *after* the window's leading edge. A `None` here
        // means `timestamp` precedes `now - window`, i.e. it is below the lower
        // edge and out of range. Computing it as `window - (now - timestamp)`
        // keeps the arithmetic on `Duration`s without materializing the
        // possibly-pre-epoch instant `now - window`.
        let age = now.duration_since(timestamp);
        let elapsed = self.window.checked_sub(age)?;

        // index = elapsed * columns / window, in nanoseconds for an exact, slice
        // -width-free mapping. `u128` headroom keeps `elapsed_ns * columns` from
        // overflowing even for very long windows and wide terminals.
        let window_ns = self.window.as_nanos();
        if window_ns == 0 {
            // A zero-length window degenerates to a single instant; everything in
            // range collapses into the last bucket.
            return Some(columns - 1);
        }
        let elapsed_ns = elapsed.as_nanos();
        let raw = elapsed_ns * columns as u128 / window_ns;
        // Clamp the trailing-edge sample (raw == columns) back into the last slot.
        let index = (raw as usize).min(columns - 1);
        Some(index)
    }
}

/// The per-bucket `f64` series for `key` over `buckets` (as produced by
/// [`History::bucket_last`]), honoring the `languages` filter for per-language
/// metrics. The output length always equals `buckets.len()`.
///
/// This is the bridge between the raw bucketed [`Stats`] snapshots and the
/// pure [`crate::sparkline::metric_sparkline`] renderer: it turns each
/// `MetricKind`'s cumulative or absolute values into exactly the series the
/// sparkline expects.
/// The shaping rules come straight from the design's "Sparkline semantics" and
/// differ by kind:
///
/// - [`MetricKind::Count`] → the **per-bucket delta** (activity in that slice),
///   computed by [`count_deltas`]. Cumulative counters only ever increase, so
///   the interesting signal is how much each slice added.
/// - [`MetricKind::Rate`] → the **windowed hit rate** per bucket, computed from
///   the same per-bucket hit/miss deltas via [`crate::aggregate::hit_rate`], so
///   a zero-activity slice renders `0.0` (baseline, never `NaN`) and every value
///   is inherently in `0.0..=100.0`.
/// - [`MetricKind::Size`] → the **absolute value** per bucket (not a delta),
///   computed by [`size_absolutes`]: a sample's own value, carried forward
///   across gaps.
///
/// Per-language counters (`cache_hits`/`cache_misses`/`cache_errors`, and the
/// hit/miss inputs to `hit_rate`) are filtered through
/// [`crate::aggregate::lang_sum`] with `languages`; global counters and sizes
/// ignore `languages` entirely.
pub(crate) fn metric_series(
    key: MetricKey,
    buckets: &[Option<&Stats>],
    languages: &[String],
) -> Vec<f64> {
    match key.kind() {
        // Cumulative counters → per-bucket deltas. Extract each bucket's
        // cumulative value (per-language counters route through `lang_sum`;
        // globals read the field directly) and let `count_deltas` do the
        // carry-forward / first-bucket-zero / reset-clamp shaping.
        MetricKind::Count => {
            let cumulatives = count_cumulatives(key, buckets, languages);
            count_deltas(&cumulatives)
        }
        // hit_rate → windowed rate per bucket: the per-bucket hits-delta and
        // misses-delta (shaped exactly like Count), combined via `hit_rate` so a
        // zero-activity slice is 0.0 (baseline, never NaN) and values are 0..=100.
        MetricKind::Rate => {
            let hit_cumulatives =
                lang_cumulatives(buckets, languages, |stats| &stats.stats.cache_hits.counts);
            let miss_cumulatives =
                lang_cumulatives(buckets, languages, |stats| &stats.stats.cache_misses.counts);
            // Keep the hit/miss deltas as `u64` here — feeding them straight into
            // `hit_rate` avoids a lossy `f64 -> u64` round-trip (which clippy's
            // cast lints forbid) and lets `hit_rate` own the 0..=100 / no-NaN
            // contract.
            let hit_deltas = count_deltas_u64(&hit_cumulatives);
            let miss_deltas = count_deltas_u64(&miss_cumulatives);
            hit_deltas
                .iter()
                .zip(miss_deltas.iter())
                .map(|(&hits, &misses)| crate::aggregate::hit_rate(hits, misses))
                .collect()
        }
        // Sizes → absolute value per bucket (not a delta), carried forward.
        MetricKind::Size => size_absolutes(key, buckets),
    }
}

/// The per-bucket cumulative value of the `Count` metric `key`, one `Option<u64>`
/// per bucket: `Some(value)` when a sample landed in that slice, `None` for a gap.
///
/// Per-language counters (`cache_hits`/`cache_misses`/`cache_errors`) sum the
/// selected `languages` via [`crate::aggregate::lang_sum`]; every global counter
/// reads its scalar field directly and ignores `languages`. The result feeds
/// [`count_deltas`], which turns these cumulatives into per-slice activity.
fn count_cumulatives(
    key: MetricKey,
    buckets: &[Option<&Stats>],
    languages: &[String],
) -> Vec<Option<u64>> {
    match key {
        MetricKey::CacheHits => {
            lang_cumulatives(buckets, languages, |stats| &stats.stats.cache_hits.counts)
        }
        MetricKey::CacheMisses => {
            lang_cumulatives(buckets, languages, |stats| &stats.stats.cache_misses.counts)
        }
        MetricKey::CacheErrors => {
            lang_cumulatives(buckets, languages, |stats| &stats.stats.cache_errors.counts)
        }
        // Every remaining `Count` key is a global scalar counter: read the field
        // and ignore `languages`. `kind()` guarantees the size/rate keys never
        // reach here, so the catch-all only ever sees globals.
        _ => buckets
            .iter()
            .map(|bucket| bucket.map(|stats| global_count(key, stats)))
            .collect(),
    }
}

/// The cumulative value of a global scalar `Count` counter `key` from one
/// `stats` snapshot.
///
/// Only the global `Count` keys are reachable — per-language and non-`Count`
/// keys are routed elsewhere by [`metric_series`]/[`count_cumulatives`] — so the
/// size, rate, and per-language arms collapse into an unreachable `0`, keeping
/// the match total without inventing values for keys that never arrive.
fn global_count(key: MetricKey, stats: &Stats) -> u64 {
    let counters = &stats.stats;
    match key {
        MetricKey::CompileRequests => counters.compile_requests,
        MetricKey::RequestsExecuted => counters.requests_executed,
        MetricKey::RequestsNotCacheable => counters.requests_not_cacheable,
        MetricKey::RequestsNotCompile => counters.requests_not_compile,
        MetricKey::RequestsUnsupportedCompiler => counters.requests_unsupported_compiler,
        MetricKey::CacheWrites => counters.cache_writes,
        MetricKey::Compilations => counters.compilations,
        MetricKey::CompileFails => counters.compile_fails,
        MetricKey::ForcedRecaches => counters.forced_recaches,
        // Unreachable for global `Count` keys; per-language counters, sizes, and
        // the rate are handled before `global_count` is ever called.
        MetricKey::CacheHits
        | MetricKey::CacheMisses
        | MetricKey::CacheErrors
        | MetricKey::HitRate
        | MetricKey::CacheSize
        | MetricKey::MaxCacheSize => 0,
    }
}

/// The per-bucket cumulative of a per-language counter map, filtered by
/// `languages`.
///
/// `select` picks the relevant `HashMap<String, u64>` out of each sample (e.g.
/// `cache_hits.counts`); [`crate::aggregate::lang_sum`] then sums the selected
/// languages (an empty `languages` sums all). A `None` bucket stays `None` so the
/// gap propagates into the delta engine as "no sample landed here".
fn lang_cumulatives(
    buckets: &[Option<&Stats>],
    languages: &[String],
    select: impl Fn(&Stats) -> &std::collections::HashMap<String, u64>,
) -> Vec<Option<u64>> {
    buckets
        .iter()
        .map(|bucket| bucket.map(|stats| crate::aggregate::lang_sum(select(stats), languages)))
        .collect()
}

/// Turn a per-bucket cumulative series into the per-bucket *delta* series the
/// sparkline draws — the activity that occurred within each time slice.
///
/// The shaping rules (from the design's "Sparkline semantics") are:
///
/// - **Carry-forward across gaps.** A `None` bucket means no sample landed in
///   that slice, so the cumulative is unchanged and the delta is `0.0`. The
///   previously-seen cumulative is carried forward to the next observed sample.
/// - **Leading gap and first sample are baseline.** Buckets before the first
///   observed sample are `0.0` (no data yet). The first observed bucket is *also*
///   `0.0`: its cumulative includes everything since sccache started, and
///   sparking that as a delta would draw a giant spurious launch spike.
/// - **Reset clamping.** A cumulative *drop* (a mid-run `sccache --zero-stats`)
///   would make the raw delta negative; [`u64::saturating_sub`] clamps it to
///   `0.0` and the carried baseline becomes the new, lower value, so history
///   continues from there and a later increase shows its true delta.
///
/// The output length always equals `cumulatives.len()`.
///
/// This is the `f64` face of [`count_deltas_u64`] used directly by `Count`
/// metrics; the `Rate` path consumes the `u64` deltas instead, so the two share
/// one definition of the shaping rules.
fn count_deltas(cumulatives: &[Option<u64>]) -> Vec<f64> {
    count_deltas_u64(cumulatives)
        .into_iter()
        .map(|delta| delta as f64)
        .collect()
}

/// The integer per-bucket deltas backing [`count_deltas`] — identical shaping
/// (carry-forward, first-bucket-zero, reset clamping), but kept as `u64` so the
/// `hit_rate` path can consume them without a lossy `f64 -> u64` round-trip.
///
/// The output length always equals `cumulatives.len()`.
fn count_deltas_u64(cumulatives: &[Option<u64>]) -> Vec<u64> {
    // The most recent cumulative actually observed, or `None` until the first
    // sample appears. While it is `None` every bucket is baseline `0` (no data to
    // diff against); once set, each new sample's delta is measured against it.
    let mut previous: Option<u64> = None;
    cumulatives
        .iter()
        .map(|cumulative| match (*cumulative, previous) {
            // No sample in this slice → cumulative unchanged → zero activity. The
            // carried baseline is untouched so the next real sample diffs against
            // the last observed value, not against this gap.
            (None, _) => 0,
            // First observed sample: establish the baseline but emit 0 — its
            // cumulative is the all-time total, not this slice's activity.
            (Some(current), None) => {
                previous = Some(current);
                0
            }
            // A subsequent sample: the delta is `current - baseline`, clamped to
            // 0 on a reset (a drop). Either way the baseline advances to `current`
            // so post-reset history continues from the new floor.
            (Some(current), Some(baseline)) => {
                let delta = current.saturating_sub(baseline);
                previous = Some(current);
                delta
            }
        })
        .collect()
}

/// The per-bucket *absolute* value series for a `Size` metric `key`, carried
/// forward across gaps.
///
/// Sizes (`cache_size`, `max_cache_size`) are not cumulative counters, so they
/// spark their literal value, not a delta. A bucket with a sample contributes
/// that sample's size; a `None` bucket carries the previously-observed size
/// forward (the cache hasn't changed in that idle slice); buckets before the
/// first sample are `0.0` (no data yet). The output length always equals
/// `buckets.len()`.
fn size_absolutes(key: MetricKey, buckets: &[Option<&Stats>]) -> Vec<f64> {
    // The most recent size observed, carried into later gap buckets. `0` until the
    // first sample, matching the "no data yet → baseline" rule for the leading run
    // of `None`s.
    let mut last: u64 = 0;
    buckets
        .iter()
        .map(|bucket| {
            if let Some(stats) = bucket {
                last = size_value(key, stats);
            }
            last as f64
        })
        .collect()
}

/// The absolute byte value of the `Size` metric `key` from one `stats` snapshot.
///
/// Only the two size keys are reachable here ([`metric_series`] dispatches on
/// [`MetricKind::Size`]); any other key collapses to `0`, keeping the match total
/// without inventing a size for a non-size metric.
fn size_value(key: MetricKey, stats: &Stats) -> u64 {
    match key {
        MetricKey::CacheSize => stats.cache_size,
        MetricKey::MaxCacheSize => stats.max_cache_size,
        // Unreachable: only `MetricKind::Size` keys reach `size_absolutes`.
        _ => 0,
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
        // Epoch = base, so the slice grid is anchored at base. The bucket `now`s
        // below sit on exact grid multiples (slice == window for a single
        // column), so `leading == now` and the pre-quantization expectations hold.
        let mut history = History::new(window, base);

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
        // Epoch = base; `now = base + 4s` is an exact grid multiple (4 slices of
        // 1s), so `leading == now` and the slice boundaries match the pre-pinning
        // window exactly.
        let mut history = History::new(window, base);
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
        let now = Instant::now();
        // Epoch is irrelevant to an empty ring (no samples to place), so any
        // anchor <= `now` works; `now` itself keeps `elapsed` non-negative.
        let history = History::new(Duration::from_secs(10), now);

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
        // Epoch = base; `columns == 0` bails before any slice-width math, so the
        // grid anchor never participates.
        let mut history = History::new(Duration::from_secs(10), base);
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
        // Epoch = base; `now = base + 3s` is an exact grid multiple (3 slices of
        // 1s), so `leading == now` and "exactly at `now` ⇒ last bucket" still
        // pins the trailing edge.
        let mut history = History::new(window, base);
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
        // Epoch = base; `now = base + 3s` is an exact grid multiple, so
        // `leading == now` and `leading - window == base`: the sample at `base`
        // sits on the inclusive lower edge and still lands in the first bucket.
        let mut history = History::new(window, base);
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
    fn bucket_assignment_is_invariant_as_now_advances_within_a_slice() {
        // The headline stability property: as long as two `now`s quantize to the
        // SAME grid point, a retained sample lands in the SAME bucket both times —
        // no temporal aliasing, no glyph reshuffling between frames. window = 4s
        // over 4 columns ⇒ 1s slices, epoch = base. Both bucket times sit in the
        // same slice [base+4s, base+5s), so both ceil to k = 5 and
        // leading = base + 5s for both calls.
        //
        // The sample at base+1.2s is chosen so the OLD sliding-window code
        // re-bins it between the two nows: at now = base+4.1s the un-quantized
        // window [base+0.1s, base+4.1s] puts it in bucket 1, but at now = base+4.5s
        // the window [base+0.5s, base+4.5s] puts it in bucket 0 — a visible shift.
        // Under the epoch-pinned grid both calls bucket against leading = base+5s,
        // so the sample is bucket 0 both times and the assignment is stable.
        let base = Instant::now();
        let window = Duration::from_secs(4);
        let mut history = History::new(window, base);
        history.push(base + Duration::from_millis(1200), tagged(42));

        let now1 = base + Duration::from_millis(4100);
        let now2 = base + Duration::from_millis(4500);
        let buckets1 = history.bucket_last(now1, 4);
        let buckets2 = history.bucket_last(now2, 4);

        assert_eq!(
            tags(&buckets1),
            tags(&buckets2),
            "a sample's bucket assignment must not change as `now` advances within \
             one slice (epoch-pinned grid); got {:?} then {:?}",
            tags(&buckets1),
            tags(&buckets2),
        );
        // Pin the actual assignment too, so a future regression can't make both
        // calls agree on the *wrong* (un-pinned) bucket: leading = base+5s puts
        // the base+1.2s sample 3.8s back from the leading edge, i.e. 0.2s into the
        // window ⇒ bucket 0.
        assert_eq!(
            tags(&buckets1),
            vec![Some(42), None, None, None],
            "the pinned grid (leading = epoch + 5*slice) must place the sample in \
             bucket 0",
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
        let mut history = History::new(Duration::from_secs(1000), base); // keep everything

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

    // ---- metric_series ----

    use crate::config::MetricKey;

    /// The float epsilon used by series assertions involving rate math. Counter
    /// deltas and sizes are exact integers stored as `f64`, but `hit_rate` does a
    /// real division, so its expectations carry this tolerance.
    const EPSILON: f64 = 1e-9;

    /// A `Stats` whose only meaningful field is a global counter
    /// (`compile_requests`), tagged with `value`. Used to distinguish buckets of
    /// a global Count metric by giving each its own cumulative value.
    fn global_counter(value: u64) -> Stats {
        let mut stats = Stats::default();
        stats.stats.compile_requests = value;
        stats
    }

    /// A `Stats` whose `cache_hits`/`cache_misses` per-language maps are set from
    /// the supplied `(lang, hits, misses)` triples. Everything else is default.
    /// Used to craft per-language hit/miss cumulatives for windowed-rate and
    /// language-filter tests.
    fn lang_hits_misses(entries: &[(&str, u64, u64)]) -> Stats {
        let mut stats = Stats::default();
        for &(lang, hits, misses) in entries {
            stats.stats.cache_hits.counts.insert(lang.to_string(), hits);
            stats
                .stats
                .cache_misses
                .counts
                .insert(lang.to_string(), misses);
        }
        stats
    }

    /// A `Stats` whose top-level `cache_size` is `value`. Used to drive the
    /// absolute-value Size series.
    fn sized(value: u64) -> Stats {
        Stats {
            cache_size: value,
            ..Default::default()
        }
    }

    /// Assert two `f64` series are equal element-wise within [`EPSILON`], with a
    /// length check first so a length mismatch fails loudly rather than panicking
    /// on a zip.
    fn assert_series_eq(actual: &[f64], expected: &[f64]) {
        assert_eq!(
            actual.len(),
            expected.len(),
            "series length mismatch: {actual:?} vs {expected:?}",
        );
        for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
            assert!(
                (a - e).abs() < EPSILON,
                "series[{i}] = {a}, expected {e} (full: {actual:?} vs {expected:?})",
            );
        }
    }

    #[test]
    fn count_series_is_per_bucket_deltas_with_carry_forward() {
        // Cumulative counter values [None, 10, 15, None, 18] over five buckets.
        // The first OBSERVED bucket (10) deltas to 0 — its cumulative includes
        // everything since launch, so sparking it as a delta would draw a giant
        // spurious spike. 15 - 10 = 5. The None carries the previous cumulative
        // (15) forward unchanged → delta 0. 18 - 15 = 3. The leading None is
        // before any sample → baseline 0.
        let b1 = global_counter(10);
        let b2 = global_counter(15);
        let b4 = global_counter(18);
        let buckets = vec![None, Some(&b1), Some(&b2), None, Some(&b4)];

        let series = metric_series(MetricKey::CompileRequests, &buckets, &[]);
        assert_series_eq(&series, &[0.0, 0.0, 5.0, 0.0, 3.0]);
    }

    #[test]
    fn count_series_clamps_negative_deltas_on_reset() {
        // A mid-run `sccache --zero-stats` makes the cumulative DROP: [100, 40,
        // 45]. The first observed bucket (100) is 0. 40 - 100 would be negative —
        // clamp to 0 (saturating_sub) and continue history from the new lower
        // baseline (40). 45 - 40 = 5: the later real increase shows its true
        // delta, proving the baseline reset rather than staying at 100.
        let b0 = global_counter(100);
        let b1 = global_counter(40);
        let b2 = global_counter(45);
        let buckets = vec![Some(&b0), Some(&b1), Some(&b2)];

        let series = metric_series(MetricKey::CompileRequests, &buckets, &[]);
        assert_series_eq(&series, &[0.0, 0.0, 5.0]);
    }

    #[test]
    fn rate_series_is_windowed_hit_rate_per_bucket_never_nan() {
        // hit_rate sparks the WINDOWED rate: per bucket, hits-delta and
        // misses-delta computed exactly like Count, then hit_rate(h, m).
        // Cumulatives so each bucket's delta is what we want to test:
        //   b0: (10, 10)  — first observed, deltas are 0/0 → hit_rate(0,0) = 0.0
        //   b1: (15, 25)  — +5 hits, +15 misses... but we want +5/+5 → use (15,15)
        // Recompute deliberately: choose cumulatives so the deltas are the
        // crafted (hits_delta, misses_delta) pairs:
        //   b0 = (10, 10):  first observed → 0/0  → 0.0
        //   b1 = (15, 15):  +5 / +5        → 50.0
        //   b2 = (15, 15):  +0 / +0        → 0.0  (zero-activity slice, baseline)
        //   b3 = (25, 15):  +10 / +0       → 100.0 (all-hits slice)
        let b0 = lang_hits_misses(&[("Rust", 10, 10)]);
        let b1 = lang_hits_misses(&[("Rust", 15, 15)]);
        let b2 = lang_hits_misses(&[("Rust", 15, 15)]);
        let b3 = lang_hits_misses(&[("Rust", 25, 15)]);
        let buckets = vec![Some(&b0), Some(&b1), Some(&b2), Some(&b3)];

        let series = metric_series(MetricKey::HitRate, &buckets, &[]);
        assert_series_eq(&series, &[0.0, 50.0, 0.0, 100.0]);
        assert!(
            series.iter().all(|v| !v.is_nan()),
            "no rate bucket may be NaN, got {series:?}",
        );
    }

    #[test]
    fn rate_series_respects_languages_filter() {
        // Two languages with different per-bucket deltas. Cumulatives:
        //   b0: Rust (10,10), C/C++ (10,10)  — first observed → all deltas 0
        //   b1: Rust (20,10), C/C++ (10,20)  — Rust +10h/+0m, C/C++ +0h/+10m
        // Rust-only window: hit_rate(10, 0) = 100.0.
        // All-languages window: hits_delta = 10 (Rust) + 0 (C/C++) = 10,
        //   misses_delta = 0 (Rust) + 10 (C/C++) = 10 → hit_rate(10, 10) = 50.0.
        // The two filters MUST give different rates for the same buckets.
        let b0 = lang_hits_misses(&[("Rust", 10, 10), ("C/C++", 10, 10)]);
        let b1 = lang_hits_misses(&[("Rust", 20, 10), ("C/C++", 10, 20)]);
        let buckets = vec![Some(&b0), Some(&b1)];

        let rust_only = metric_series(MetricKey::HitRate, &buckets, &["Rust".to_string()]);
        let all_langs = metric_series(MetricKey::HitRate, &buckets, &[]);

        assert_series_eq(&rust_only, &[0.0, 100.0]);
        assert_series_eq(&all_langs, &[0.0, 50.0]);
        assert!(
            (rust_only[1] - all_langs[1]).abs() > EPSILON,
            "the language filter must change the windowed rate: {rust_only:?} vs {all_langs:?}",
        );
    }

    #[test]
    fn count_series_per_language_respects_languages_global_ignores_it() {
        // Per-language cache_hits cumulatives: Rust climbs by 5/bucket while
        // C/C++ climbs by 100/bucket.
        //   b0: Rust 10, C/C++ 1000  — first observed → 0
        //   b1: Rust 15, C/C++ 1100  — Rust +5, C/C++ +100, all +105
        // Rust-only delta = 5; all-languages delta = 105. The filter MUST change
        // a per-language counter's series.
        let b0 = lang_hits_misses(&[("Rust", 10, 0), ("C/C++", 1000, 0)]);
        let b1 = lang_hits_misses(&[("Rust", 15, 0), ("C/C++", 1100, 0)]);
        let buckets = vec![Some(&b0), Some(&b1)];

        let rust_only = metric_series(MetricKey::CacheHits, &buckets, &["Rust".to_string()]);
        let all_langs = metric_series(MetricKey::CacheHits, &buckets, &[]);
        assert_series_eq(&rust_only, &[0.0, 5.0]);
        assert_series_eq(&all_langs, &[0.0, 105.0]);

        // A global counter ignores `languages`: the per-bucket delta is identical
        // whether a filter is supplied or not.
        let g0 = global_counter(10);
        let g1 = global_counter(15);
        let globals = vec![Some(&g0), Some(&g1)];
        let global_filtered =
            metric_series(MetricKey::CompileRequests, &globals, &["Rust".to_string()]);
        let global_unfiltered = metric_series(MetricKey::CompileRequests, &globals, &[]);
        assert_series_eq(&global_filtered, &[0.0, 5.0]);
        assert_series_eq(&global_unfiltered, &[0.0, 5.0]);
    }

    #[test]
    fn size_series_is_absolute_values_with_carry_forward() {
        // Size sparks the ABSOLUTE value per bucket, not a delta. A None bucket
        // carries the previous value forward; a leading None is 0 (no data yet).
        // [None, 100, 100(carried), 250, 250(carried)].
        let b1 = sized(100);
        let b3 = sized(250);
        let buckets = vec![None, Some(&b1), None, Some(&b3), None];

        let series = metric_series(MetricKey::CacheSize, &buckets, &[]);
        assert_series_eq(&series, &[0.0, 100.0, 100.0, 250.0, 250.0]);
    }

    #[test]
    fn all_none_buckets_are_all_zeros_and_empty_is_empty() {
        // Every kind: a row of None buckets (nothing observed yet) must yield all
        // zeros with the length preserved, and an empty buckets slice must yield
        // an empty vec.
        let all_none: Vec<Option<&Stats>> = vec![None, None, None];
        for key in [
            MetricKey::CompileRequests, // Count
            MetricKey::HitRate,         // Rate
            MetricKey::CacheSize,       // Size
        ] {
            let series = metric_series(key, &all_none, &[]);
            assert_series_eq(&series, &[0.0, 0.0, 0.0]);

            let empty = metric_series(key, &[], &[]);
            assert!(
                empty.is_empty(),
                "an empty buckets slice must yield an empty series for {key:?}, got {empty:?}",
            );
        }
    }
}
