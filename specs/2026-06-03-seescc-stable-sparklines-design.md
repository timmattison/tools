# seescc: Stable Sparklines — Pinned Buckets and Fixed Scale

**Date:** 2026-06-03
**Status:** Approved
**Supersedes:** the "Sparkline semantics" section of
[2026-06-02-seescc-design.md](./2026-06-02-seescc-design.md) where the two
conflict (bucketing anchor and y-scaling). Everything else in that spec stands.

## Problem

In watch mode, sparkline columns visibly change *after* they have moved left,
even though the underlying history ring is append-only and no retained sample
ever mutates. The history is immutable but its rendering is recomputed every
frame against three moving references:

1. **Per-frame auto-rescaling.** `sparkline()` scales each frame to the visible
   window's own min..max, so a new spike entering on the right (new max) or the
   old max scrolling off the left re-maps every glyph in the row.
2. **Sliding bucket grid.** `History::bucket_last(now, columns)` carves the
   window into slices anchored at the current `Instant::now()`, so bucket
   boundaries sweep continuously and samples re-bin one at a time as boundaries
   pass them — temporal aliasing.
3. **Representative churn.** Each bucket keeps the newest sample that lands in
   it; as the grid slides, "newest in slice *i*" keeps changing, and Count
   metrics diff adjacent representatives, so activity wobbles between
   neighboring columns.

A fourth, subtler mutation: when the oldest sample is pruned, the next bucket
becomes "first observed" and its delta is forced to 0, so the leftmost column
changes just before it exits.

The goal: **once a column's time slice has completed, its glyph never changes**
— it only shifts left, one whole column per slice, until it falls off the edge.

## Decisions (made with Tim, 2026-06-03)

- **Y-scale: high-water mark.** Baseline pinned at 0. Rate is fixed 0–100.
  Sizes scale against `max_cache_size`. Counts scale against the highest
  per-bucket delta observed since launch (monotonic). Columns rescale only when
  a new all-time record is set — never when a spike scrolls off. Accepted
  trade-off: after a huge spike, later modest activity renders low.
- **Right edge: live in-progress column.** The rightmost column accumulates and
  re-renders as polls land in the current slice, then freezes forever when the
  slice completes. Rejected alternative (completed-slices-only) hides up to a
  full slice (~20–45 s) of activity in a tool polling every 1 s.
- **Approach: surgical re-projection (Approach A).** Keep the pure
  "render = f(ring)" architecture and change its inputs, rather than
  materializing a per-metric shift register (Approach B), which duplicates
  state, complicates resize and language-filter changes, and buys nothing
  visible beyond A.

## Design

### 1. Epoch-pinned slice grid (`history.rs`)

`History` gains an `epoch: Instant` field, injected at construction:

```rust
History::new(window: Duration, epoch: Instant)
```

`WatchState::new` takes and forwards the same `epoch`; production passes
`Instant::now()` once at watch startup, tests fabricate it. The epoch is the
anchor of the **slice grid**: with `slice = window / columns` (nanosecond
`u128` math), grid points sit at `epoch + k * slice`.

`bucket_last(now, columns)` quantizes its leading edge **up** to the grid
internally:

```text
elapsed = now - epoch
k       = ceil(elapsed / slice)        // integer ceil in nanoseconds
leading = epoch + k * slice
buckets span [leading - window, leading], newest slice last
```

All existing bucket-index math (`bucket_index`, the inclusive-edge rules, the
clamp into the last slot) is reused verbatim with `leading` substituted for
`now`. The inputs do not change — callers still pass a raw `now` — and the
quantization is hidden inside `History`, which is exactly what keeps the module
deep. (The *return type* changes for the baseline, §2.)

Consequences:

- A sample's bucket assignment is a pure function of its timestamp and the grid
  — it never changes between frames. When `k` increments, every sample shifts
  exactly one column left.
- The rightmost slice spans `[leading - slice, leading]` with `leading >= now`:
  it is the **in-progress** slice. New polls land in it (they are never newer
  than `leading`), so it live-updates until the grid steps past it.
- The displayed window no longer ends exactly at `now`; it ends at the current
  slice's end, snapping forward in whole-slice steps. Invisible in practice.
- A different `columns` (terminal resize) means a different slice width and a
  different grid; the whole row legitimately re-renders once. Within a fixed
  `columns`, the grid is permanent for the process lifetime.

Degenerate guards: `columns == 0` still returns an empty `Vec` before any
slice-width math; if `slice` computes to zero nanoseconds (window shorter than
`columns` ns), fall back to the unquantized `now` — the window is degenerate
and stability is moot.

### 2. Pre-window baseline (`history.rs`)

To freeze the left edge, the delta engine needs a diff anchor that survives the
leftmost visible sample scrolling off.

- **Retention:** `push` keeps **the newest sample older than the cutoff**
  instead of pruning the entire aged-out prefix. Cost: one extra retained
  sample. The ring still cannot grow unboundedly.
- **Exposure:** `bucket_last` returns a struct instead of a bare `Vec`:

  ```rust
  pub(crate) struct BucketedHistory<'a> {
      /// Newest sample strictly before the window start, if any.
      pub(crate) baseline: Option<&'a Stats>,
      /// One slot per column; `Some` = newest sample in that slice.
      pub(crate) buckets: Vec<Option<&'a Stats>>,
  }
  ```

- **Consumption:** `metric_series` takes `&BucketedHistory` and implements the
  baseline by **prepending it as a virtual bucket, running the existing
  shaping rules unchanged, then dropping the first output element**:
  - Count: the baseline becomes the "first observed" cumulative (emits the
    dropped 0 and seeds the carry), so the leftmost visible column shows its
    true delta even after its predecessor scrolled off. Reset clamping
    (`saturating_sub`) composes unchanged.
  - Rate: same prepend/drop applied to the hit and miss cumulative series
    before `hit_rate`.
  - Size: the baseline seeds the carry-forward value, so a leading gap shows
    the pre-window size instead of 0.

  With `baseline == None` (early in the process lifetime) the virtual bucket is
  absent and behavior is exactly today's — including the deliberate
  launch-spike suppression (the first-ever observed sample still emits 0).

### 3. Fixed scale (`sparkline.rs` + new `scale.rs`)

`sparkline` takes an explicit scale; the baseline is pinned at 0:

```rust
pub(crate) fn sparkline(values: &[f64], max: f64) -> String
// index = (v / max * 7.0).round() clamped to 0..=7
```

- `max <= 0.0` or non-finite → all-baseline row of `values.len()` glyphs (the
  no-data / no-scale case; also what an all-zero series produces naturally).
- `v` non-finite → baseline at that position (unchanged defensive rule).
- `v > max` clamps to the top glyph; `v < 0` clamps to baseline (counts and
  sizes are non-negative by construction; rates are 0–100 by `hit_rate`'s
  contract — both clamps are belt-and-braces).
- The min..max auto-scaling and its "flat series ⇒ no shape" reasoning are
  deleted along with their module docs.

The per-metric `max` comes from a new **`ScaleTracker`** in a new `scale.rs`
module — pure, no clock, no terminal:

```rust
pub(crate) struct ScaleTracker { /* private: per-key high-water map + the
                                    column count it was seeded at */ }

impl ScaleTracker {
    pub(crate) fn new() -> Self;
    /// The y-scale max for `key` given this frame's `series`, updating
    /// internal high-water state. `columns` invalidation is self-managed.
    pub(crate) fn scale_for(
        &mut self,
        key: MetricKey,
        series: &[f64],
        max_cache_size: Option<u64>,
        columns: usize,
    ) -> f64;
}
```

Scale policy by `MetricKind`:

| Kind | Scale |
|---|---|
| `Rate` | fixed `100.0` |
| `Size` | `max_cache_size` from the latest good stats; when sccache reports 0 (or none is available), fall back to the high-water of the metric's own observed values |
| `Count` | monotonic high-water of that metric's per-bucket deltas since launch: `hw = hw.max(series max)` each frame |

**Column-count invalidation:** per-bucket deltas scale with slice width, so a
high-water recorded at one `columns` is meaningless at another. The tracker
records the `columns` it was last fed; when it changes (terminal resize), all
count high-waters reset and re-seed from that frame's series. The window is
fixed for the process lifetime, so `columns` is the only way slice width moves.

### 4. Integration (`watch.rs`)

The `ScaleTracker` is **render-side state, not poll-side state**: it depends on
the render-time column budget, so it does not belong in `WatchState` (whose
render closure contract is `FnMut(&WatchState)` and stays immutable).

- `run_watch` creates one `ScaleTracker` and moves it into the render closure;
  the closure passes it `&mut` into `compose_watch_frame`, which threads it to
  `attach_sparklines`.
- `attach_sparklines` per spark row: compute the series from the
  `BucketedHistory`, ask `tracker.scale_for(...)` for the max, render
  `sparkline(&series, max)`.
- The event loop, poll cadence, byte-identical paint suppression, and resize
  handling are untouched. Pinning makes consecutive frames byte-identical
  between grid steps except the live right column — the existing suppression
  then skips the repaint for free.
- One-shot mode renders no sparklines and never constructs a tracker.

### 5. Semantic changes (user-visible, intended)

- **Completed columns are frozen.** A glyph changes only while its slice is the
  rightmost (live) one, or when a new all-time count record / `max_cache_size`
  change rescales the row (rare, monotonic), or on terminal resize.
- **Flat nonzero series render at their true height.** Today's auto-scale
  collapses any flat series to `▁▁▁`. Under a 0-pinned fixed scale,
  `cache_size` near its cap is a row of tall glyphs (informative: "cache is
  full") and `max_cache_size` is a constant full-height row. This is the honest
  absolute reading and is accepted.
- **Heights are comparable.** Within a row, equal heights now mean equal values
  across the whole window — no longer true under per-frame renormalization.
- **Less shape amplification.** A series hovering in a narrow band near its max
  (e.g. 950–1000) reads as nearly flat instead of spanning the full glyph
  range. Accepted as the cost of stability; the trend is still visible, just
  proportionate.

## Testing (TDD, red → green per behavior)

1. **Grid pinning** (`history.rs`): with a fabricated epoch, a sample's bucket
   index is invariant as `now` advances within one slice; advancing `now` past
   a grid point shifts every sample exactly one column left.
2. **Live right column**: a sample pushed after the last grid point (i.e. in
   the in-progress slice) lands in the final bucket.
3. **Baseline retention**: `push` retains exactly the newest pre-cutoff sample;
   `bucket_last` surfaces it as `baseline`; a Count column's delta is unchanged
   when its predecessor sample ages out of the window; a Size series seeds its
   carry-forward from the baseline.
4. **Fixed-scale sparkline** (`sparkline.rs`): known value→glyph mappings
   against an explicit max; flat-nonzero series renders at its true height;
   `max <= 0` → all-baseline; `v > max` clamps to the top glyph; non-finite
   values and empty input unchanged.
5. **`ScaleTracker`** (`scale.rs`): count high-water is monotonic across
   frames; changing `columns` resets and re-seeds it; Rate always returns 100;
   Size uses `max_cache_size` and falls back to its own high-water when that
   is 0/absent.
6. **Frame-level stability** (`watch.rs`): `compose_watch_frame` twice with
   `now` advanced within one slice → spark strings identical except possibly
   the last glyph; advanced across a grid step → previous glyphs shifted left
   by one.

Existing history/sparkline tests are updated to the new constructors and
signatures; the boundary rules they pin (inclusive edges, newest-wins,
zero-columns, UTF-8 glyph invariants) all still hold and stay pinned.

## Documentation updates

- Module docs: `sparkline.rs` (auto-scaling rationale → fixed-scale rationale),
  `history.rs` (window `[now − window, now]` → epoch-pinned grid + baseline),
  `watch.rs` (`WatchState`/`compose_watch_frame` doc comments).
- `README.md` seescc entry: sparkline description.
- This spec supersedes the "Sparkline semantics" bucketing/scaling text in
  [2026-06-02-seescc-design.md](./2026-06-02-seescc-design.md).

## Out of scope

- **Direction-colored hit-rate bars** (in the original spec, not yet
  implemented) — unaffected by this design; whenever built, color compares
  adjacent bucket values, which are now stable.
- **Glyph-set divergence**: the original spec calls for 7 glyphs (excluding
  `█`); the implementation uses 8 (including it). Pre-existing divergence,
  tracked separately, not changed here.
- **Persisting history or high-water marks across runs** — history remains
  in-memory only per the original spec's rationale.
