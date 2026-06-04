# Plan: seescc Stable Sparklines — Pinned Buckets and Fixed Scale

> Source spec: [specs/2026-06-03-seescc-stable-sparklines-design.md](../specs/2026-06-03-seescc-stable-sparklines-design.md)
> (supersedes the "Sparkline semantics" bucketing/scaling text of
> [specs/2026-06-02-seescc-design.md](../specs/2026-06-02-seescc-design.md))

Goal: **once a column's time slice has completed, its glyph never changes** —
it only shifts left, one whole column per slice, until it falls off the edge.
The spec identifies four mutation sources; each phase below eliminates one or
two of them, and the final phase proves the composed guarantee end-to-end.

Every phase is TDD: red test committed first, then the green implementation,
per behavior. Existing boundary tests (inclusive edges, newest-wins,
zero-columns, UTF-8 glyph invariants) stay pinned throughout — they are updated
to new constructors/signatures, never weakened.

## Architectural decisions

Durable decisions that apply across all phases (from the approved spec):

- **Epoch injection**: `History::new(window, epoch)` — the epoch is an
  `Instant` injected at construction. `WatchState::new` takes and forwards it;
  production passes `Instant::now()` once at watch startup; tests fabricate it.
  The slice grid (`slice = window / columns`, nanosecond `u128` math) is
  anchored at `epoch + k * slice` for the process lifetime.
- **Quantization is hidden inside `History`**: callers still pass a raw `now`
  to `bucket_last`; the leading edge is quantized **up** to the grid
  internally. The module stays deep — no caller ever sees grid math.
- **Bucketing return type**: `bucket_last` returns
  `BucketedHistory<'a> { baseline: Option<&'a Stats>, buckets: Vec<Option<&'a Stats>> }`
  instead of a bare `Vec`.
- **Baseline mechanism**: `push` retains the newest sample older than the
  cutoff (one extra retained sample, ring still bounded); `metric_series`
  consumes the baseline by prepending it as a virtual bucket, running the
  existing shaping rules unchanged, then dropping the first output element.
- **Fixed scale**: `sparkline(values: &[f64], max: f64)`, baseline pinned at 0,
  `index = (v / max * 7.0).round()` clamped to `0..=7`. The `▇` cap (no full
  block `█`) is retained. Min..max auto-scaling is deleted.
- **Scale policy** (new `scale.rs`, pure `ScaleTracker`):
  | Kind | Scale |
  |---|---|
  | `Rate` | fixed `100.0` |
  | `Size` | `max_cache_size` from latest good stats; fall back to own observed high-water when 0/absent |
  | `Count` | monotonic high-water of per-bucket deltas since launch; reset + re-seed when `columns` changes |
- **Tracker ownership**: `ScaleTracker` is render-side state — created in
  `run_watch`, moved into the render closure, threaded `&mut` through
  `compose_watch_frame` → `attach_sparklines`. It never enters `WatchState`.
  One-shot mode never constructs one.
- **Untouched**: event loop, poll cadence, byte-identical paint suppression,
  resize handling, history-window semantics, one-shot rendering.

---

## Phase 1: Epoch-pinned slice grid with live right column

**Eliminates**: mutation source #2 (sliding bucket grid / temporal aliasing)
and #3 (representative churn). **Implements decisions**: epoch-pinned grid,
live in-progress right column, surgical re-projection (Approach A).

### What to build

Thread an injected `epoch` from `run_watch` (production: `Instant::now()` once
at watch startup) through `WatchState::new` into `History::new`. Inside
`bucket_last`, quantize the leading edge up to the grid
(`k = ceil(elapsed / slice)`, integer ceil in nanoseconds; buckets span
`[leading - window, leading]`) and reuse all existing bucket-index math with
`leading` substituted for `now`. The rightmost slice spans
`[leading - slice, leading]` with `leading >= now`, so new polls land in it and
it live-updates until the grid steps past it.

Degenerate guards: `columns == 0` still returns empty before any slice-width
math; a zero-nanosecond slice falls back to the unquantized `now`.

End-to-end result: a sample's bucket assignment is a pure function of its
timestamp and the grid — between frames, columns either stay put or the whole
row shifts exactly one column left.

### Acceptance criteria

- [ ] With a fabricated epoch, a sample's bucket index is invariant as `now`
      advances within one slice.
- [ ] Advancing `now` past a grid point shifts every sample exactly one column
      left.
- [ ] A sample pushed after the last grid point (in the in-progress slice)
      lands in the final bucket.
- [ ] `columns == 0` yields an empty result; a degenerate (sub-`columns`-ns)
      window falls back to unquantized `now` without panicking.
- [ ] Existing bucket boundary tests (inclusive edges, newest-wins,
      zero-columns) still pass, updated to the new constructor.
- [ ] `cargo test`, `cargo clippy` clean; red and green commits separate.

---

## Phase 2: Pre-window baseline

**Eliminates**: mutation source #4 (leftmost column's delta forced to 0 when
its predecessor sample is pruned).

### What to build

`push` keeps the newest sample older than the cutoff instead of pruning the
whole aged-out prefix (ring stays bounded — exactly one extra sample).
`bucket_last` returns `BucketedHistory { baseline, buckets }`, surfacing that
sample as `baseline`. `metric_series` takes `&BucketedHistory` and implements
the baseline as a prepended virtual bucket whose first output element is
dropped: Count seeds the diff carry (leftmost visible column shows its true
delta), Rate applies the same prepend/drop to the hit and miss cumulative
series, Size seeds the carry-forward value. With `baseline == None`, behavior
is exactly today's — including launch-spike suppression. Reset clamping
(`saturating_sub`) composes unchanged. `attach_sparklines` consumes the new
return type.

### Acceptance criteria

- [ ] `push` retains exactly the newest pre-cutoff sample; older ones are
      still pruned.
- [ ] `bucket_last` surfaces the retained sample as `baseline`.
- [ ] A Count column's delta is unchanged when its predecessor sample ages out
      of the window (the frozen-left-edge behavior).
- [ ] A Rate column's value is unchanged when its predecessor ages out.
- [ ] A Size series with a leading gap seeds carry-forward from the baseline
      (pre-window size, not 0).
- [ ] `baseline == None` reproduces today's behavior exactly: first-ever
      observed sample still emits 0 (launch-spike suppression).
- [ ] Reset clamping (`--zero-stats` mid-run) still clamps to 0 and continues
      from the new floor, with and without a baseline.
- [ ] `cargo test`, `cargo clippy` clean; red and green commits separate.

---

## Phase 3: Fixed 0-pinned scale

**Eliminates**: mutation source #1 (per-frame min..max auto-rescaling).
**Implements decisions**: high-water y-scale, baseline pinned at 0.

### What to build

`sparkline(values, max)` replaces auto-scaling: `index = (v / max * 7.0)
.round()` clamped to `0..=7`; `max <= 0` or non-finite → all-baseline row;
`v > max` clamps to the top glyph (`▇`); `v < 0` and non-finite values clamp
to baseline; empty input unchanged. Delete the min..max logic and its module
docs. Direction coloring for Rate is untouched (it reads values, not glyphs).

New `scale.rs` with `ScaleTracker` (pure, no clock, no terminal): per-key
high-water map plus the column count it was seeded at;
`scale_for(key, series, max_cache_size, columns) -> f64` per the scale-policy
table, with `columns`-change invalidation resetting and re-seeding all count
high-waters.

Integration: `run_watch` creates the tracker and moves it into the render
closure; `compose_watch_frame` takes it `&mut` and threads it to
`attach_sparklines`, which asks `scale_for` per spark row and renders
`sparkline(&series, max)`. One-shot mode never constructs a tracker.

End-to-end result: a frozen column's glyph survives spikes scrolling off;
rescaling happens only on a new all-time count record, a `max_cache_size`
change, or resize.

### Acceptance criteria

- [ ] Known value→glyph mappings against an explicit max pin the
      `(v / max * 7).round()` rule.
- [ ] A flat nonzero series renders at its true height (not collapsed to
      `▁▁▁`); all-zero renders all-baseline naturally.
- [ ] `max <= 0` / non-finite max → all-baseline row of `values.len()` glyphs;
      `v > max` clamps to `▇`; `v < 0` and non-finite values render baseline;
      empty input → empty string. No `█` ever appears.
- [ ] `ScaleTracker`: count high-water is monotonic across frames; changing
      `columns` resets and re-seeds it; Rate always returns `100.0`; Size uses
      `max_cache_size` and falls back to its own high-water when 0/absent.
- [ ] A `compose_watch_frame` frame where a spike scrolls off leaves the
      remaining columns' glyphs unchanged (no re-map on window exit).
- [ ] Rate sparklines keep their direction coloring (green rise / red fall)
      under the fixed scale.
- [ ] Existing sparkline glyph invariants (UTF-8, one glyph per value, known
      glyph set) still pass under the new signature.
- [ ] `cargo test`, `cargo clippy` clean; red and green commits separate.

---

## Phase 4: Frame-level stability proof + documentation

**Proves**: the spec's headline guarantee, composed from phases 1–3.

### What to build

Capstone tests at the `compose_watch_frame` level: render twice with `now`
advanced within one slice → every spark row's glyph string is identical except
possibly the last glyph (the live column); render across a grid step → the
previous glyphs appear shifted left by one. With the byte-identical paint
suppression already in the event loop, pinned frames between grid steps now
skip repaints for free — assert frames are byte-identical when the live column
hasn't changed.

Documentation: rewrite module docs for `sparkline.rs` (auto-scaling rationale →
fixed-scale rationale), `history.rs` (window `[now − window, now]` →
epoch-pinned grid + baseline), and `watch.rs` (`WatchState` /
`compose_watch_frame` doc comments, tracker threading). Update the README
seescc entry's sparkline description. Confirm the new spec's supersession note
over the 2026-06-02 spec is accurate as built.

### Acceptance criteria

- [ ] `compose_watch_frame` twice with `now` advanced within one slice → spark
      strings identical except possibly the last glyph.
- [ ] `compose_watch_frame` across a grid step → previous glyphs shifted left
      by one.
- [ ] Two renders between grid steps with an unchanged live column produce
      byte-identical frames (paint suppression engages).
- [ ] Module docs in `sparkline.rs`, `history.rs`, `watch.rs` describe the new
      semantics; no stale references to min..max auto-scaling or the
      `[now − window, now]` sliding window remain.
- [ ] README seescc sparkline description updated.
- [ ] Full suite green: `cargo test`, `cargo clippy`, `cargo fmt --check`.
