# gsw — adaptive CPU throttle for watch mode

**Date:** 2026-06-26
**Status:** Approved (design), pending implementation plan
**Tool:** `src/gsw`

## Problem

In watch mode `gsw` can pin a core to ~70% on a single project. The cost is the
**git update** — one call to `build_output` (`src/gsw/src/main.rs:338`), which
runs a full `gix status` walk over the whole worktree plus per-file histogram
blob diffs (`repo::collect_changes`, `src/gsw/src/repo.rs:229`), four commit
rev-walks (ahead/behind vs base and vs upstream), and a 20-commit log walk. On a
large or divergent repo that is easily 100–200 ms of CPU per update.

`gsw` is **not** a fixed-interval poller. A git update fires on two triggers
today:

1. **Filesystem events** — a `notify` watcher on the worktree + git dir wakes the
   loop, coalesced over a **150 ms debounce** window (`watch.rs::DEBOUNCE`).
   Under continuous churn that is a full walk roughly every `150 ms + walk-time`.
2. **A decay timer** — purely to keep the age text and color fade current, the
   loop re-walks **every 1 s** when the freshest on-screen item is < 1 min old,
   **every 60 s** when < 2 h old, and never once everything is ≥ 2 h
   (`watch.rs::next_tick`). Each tick runs the *entire* walk via `build_output`.

Two amplifiers turn that into 70%:

- **The 1 s decay tick re-runs the entire walk every second** for a minute after
  *any* change, even with zero file activity — the dominant idle-after-commit
  burn.
- **Sustained FS churn** (a dev server, log files, `.git` churn from another
  tool) fires a fresh full walk every debounce window, unbounded.

## Goal

Keep watch-mode CPU at **≤ 1% of one core** as a hard ceiling (less is better;
idle is ~0%), while keeping the live display smooth. The throttle must
**self-adjust**: measure each git update and back off proportionally, so a repo
that gets slower checks less often and one that gets faster checks more often.
The current cadence is the **floor** — the throttle may only ever slow updates
down, never speed them past today's behavior.

### Non-goals

- No change to one-shot mode output (must stay byte-identical).
- No change to *what* is rendered — only *how often* git is walked.
- No configurable budget above 1% (1% is a principled ceiling, not a dial).

## Design

Two independent parts. Neither alone suffices: A leaves churn-driven walks
unbounded; B alone makes the live seconds-counter stutter. Together: smooth
display, ≤1% under load, ~0% idle.

### Part A — decouple decay re-renders from the git walk

The decay tick exists only to advance the **age text and color fade**, which are
pure functions of elapsed wall-clock time (`src/gsw/src/age.rs`). It does not
need fresh git data.

The watch loop will **cache the last `Snapshot`** together with the `Instant` it
was collected at. The four refresh triggers split by whether they need git:

| Trigger | Needs git? | Action |
|---|---|---|
| Startup | yes | one walk to seed the cache |
| **FS change** | **yes** | re-walk git (throttled — Part B) |
| Decay tick | no | re-render cached snapshot with advanced ages |
| Resize | no | re-render cached snapshot at new dimensions |

**Age advancement.** On a no-git re-render, every age is bumped by a single
`age_offset = now − collected_at`. File mtime age, last-commit age, and log ages
all grow linearly with wall time, so one offset is exact for all of them.
One-shot mode and the seed walk render with `offset = 0`, so their output is
byte-identical to today. The existing `should_repaint` byte-identical
suppression still absorbs no-op ticks (e.g. an age whose text hasn't ticked
over).

This is deliberately lower-blast-radius than retyping every `Duration` field in
`Snapshot`/`RenderEntry`/`LogEntry` to an absolute timestamp: the offset is
applied at render time at the three age sites, leaving the structs and their
extensive tests intact.

After this part, **git is walked only on FS events (and startup)** — decay ticks
and resizes never touch git.

### Part B — adaptive throttle on the git walk

A small **pure `Throttle` state machine** gates the git walk. It owns no I/O; the
loop drives it and feeds it `Instant`s, which makes it fully unit-testable.

**Measurement.** Wall-clock duration `D` of each walk, measured with `Instant`
around the git-collect call. Wall-clock is the correct proxy for "% of one core"
here — the walk is single-threaded and CPU-bound, and counting any I/O wait as
"busy" only makes the throttle *more* conservative about the ceiling.

**Cooldown.** After a walk that started at `walk_start` and took `D`, the next
walk may start no earlier than:

```
next_allowed = walk_start + max(FLOOR, D / BUDGET)
```

- `BUDGET = 0.01` (1%, a hard-coded constant).
- `FLOOR = 150 ms` (today's debounce window).
- **No ceiling** — the 1% guarantee is absolute.

**Deferral.** During a cooldown, FS events do not walk; they set a `dirty` flag.
When the cooldown expires, if `dirty`, exactly one walk runs — reflecting the
latest coalesced state — and `dirty` clears.

**Estimator.** The **last measured `D`** sizes the next cooldown — no smoothing.
This is the most literal reading of "measure each update and adjust," and it is
conservative on the ceiling: a slow walk instantly earns a long cooldown. (An
EWMA could *undershoot* after a slowdown and briefly exceed 1%, violating the
hard ceiling, so it is intentionally not used. Easy to revisit if jitter is a
problem in practice.)

#### Why this meets the spec

- `D / BUDGET = 100·D`, so under continuous churn a walk runs once per `100·D`
  and burns `D` → **duty cycle = 1%** of one core. Idle → no FS events → ~0%.
  1% is the ceiling; less is the norm.
- **Self-adjusting:** the cooldown is recomputed from each walk's own `D`.
  Slower repo → longer cooldown; faster repo → shorter.
- **Floor = current cadence:** `100·D ≥ 150 ms` for any walk costing ≥ 1.5 ms
  (every real repo), so the throttle only ever *slows* updates; sub-1.5 ms walks
  stay pinned at the 150 ms floor. It can never check more often than today.
- **Snappy after idle:** once idle, the cooldown has long since expired, so the
  first edit triggers an *immediate* walk. Throttling engages only under
  *sustained* churn.

#### Staleness trade-off

Under continuous churn a brand-new change waits up to one cooldown to appear:

| Walk cost `D` | Cooldown (`100·D`) | Worst-case staleness | Duty cycle |
|---|---|---|---|
| 30 ms | 3 s | 3 s | 1% |
| 150 ms | 15 s | 15 s | 1% |
| 500 ms | 50 s | 50 s | 1% |

A transient-slow walk (e.g. a one-off 5 s walk → ~500 s cooldown) causes one long
stale window that self-corrects after the next walk re-measures. The
**manual-refresh key** (below) is the escape hatch for that case, which is why
no cooldown ceiling is needed.

### Manual refresh

Add a **`r`** key to the watch loop's key reader (`spawn_event_reader`,
`watch.rs:498`). Pressing `r` forces an immediate git walk, bypassing the
cooldown, and resets the throttle from the fresh measurement. Cheap (the reader
thread already exists) and the user's escape hatch when a long cooldown would
otherwise make them wait.

## Components & boundaries

- **`Throttle` (new, `watch.rs`)** — pure state machine. Inputs: `Instant`s and
  measured `Duration`s. Methods (shape, not final names):
  - `on_change(now) -> WalkNow | Defer` — decide whether an FS event walks now or
    sets `dirty`.
  - `record(walk_start, D)` — set the next cooldown.
  - `next_allowed() -> Option<Instant>` — when a deferred walk may run.
  - `force()` — bypass cooldown for manual refresh.
  Holds `BUDGET`, `FLOOR`, last `next_allowed`, `dirty`.
- **Snapshot cache (loop state)** — `Option<Snapshot>` + `collected_at: Instant`
  + last `Dimensions`.
- **Split of `build_output`** — separate the expensive git collection from the
  cheap render:
  - `collect_snapshot(repo, cfg) -> Snapshot` (the throttled, FS-driven git work).
  - `render_frame(snapshot, cfg, dims, age_offset) -> Render` (cheap; used by
    every trigger).
- **`event_loop` (existing, `watch.rs:385`)** — extended with a controllable
  clock (an injected `Fn() -> Instant`, real `Instant::now` in production) so the
  throttle/decay timing is deterministic in tests, plus the snapshot cache and
  `Throttle`.

## Testing (TDD, red → commit → green → commit)

- **`Throttle` unit tests** (no I/O, injected `Instant`s):
  - cooldown equals `100·D` for representative `D`;
  - `FLOOR` clamps sub-1.5 ms walks to 150 ms (never faster than today);
  - an FS event during cooldown defers (`dirty`) and exactly one walk runs at
    expiry;
  - a slower `D` lengthens and a faster `D` shortens the next cooldown;
  - `force()` bypasses an active cooldown.
- **Age-offset re-render tests:**
  - a decay tick advances ages **without** invoking the injected git-collect
    closure;
  - `render_frame` with `offset = 0` reproduces today's output (byte-identical
    one-shot regression).
- **Loop integration** (extends the existing injected-closure tests with the
  controllable clock):
  - decay tick → no git walk; FS event → walk then throttle;
  - a burst during cooldown collapses to one deferred walk;
  - `r` forces an immediate walk mid-cooldown.

## Risks

- **Loop signature churn:** threading a clock into `event_loop` updates the
  existing loop tests. Contained — the tests already inject closures.
- **Staleness under heavy churn** is by design and bounded by the cooldown;
  `r` covers the pathological transient-slow case.
- **Render cost on huge change sets:** `render_frame` runs per decay tick, but
  its cost is bounded by terminal rows (the file list is capped to height), not
  repo size — far below a git walk.
