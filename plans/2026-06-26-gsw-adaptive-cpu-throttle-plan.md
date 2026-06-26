# Plan: gsw — adaptive CPU throttle for watch mode

> Source PRD/spec: `specs/2026-06-26-gsw-adaptive-cpu-throttle-design.md`

Watch mode can pin a core to ~70% on a single project. The cost is the **git
update** (`build_output` → `repo::collect_changes` plus rev-walks and a log
walk). Two amplifiers drive it: the **1 s decay tick re-runs the entire walk**
for a minute after any change even with zero file activity (the dominant
idle-after-commit burn), and **sustained FS churn** fires a fresh full walk
every debounce window, unbounded.

The fix is two independent parts plus an escape hatch:

- **Part A** decouples decay/resize re-renders from the git walk (cache the last
  `Snapshot`, advance ages by a wall-clock offset at render time). Git is then
  walked only on FS events and startup.
- **Part B** gates the git walk with a pure, self-adjusting `Throttle`: after a
  walk costing `D`, the next walk may start no earlier than
  `walk_start + max(FLOOR, D / BUDGET)`, so under continuous churn the duty cycle
  is exactly **1% of one core**; idle is ~0%.
- **Manual refresh** (`r`) forces an immediate walk, the escape hatch for a long
  cooldown.

**Goal:** keep watch-mode CPU at **≤ 1% of one core** (hard ceiling; idle ~0%)
while the live display stays smooth. The current cadence is the **floor** — the
throttle may only ever slow updates, never speed them past today's behavior.

---

## Architectural decisions

Durable decisions that apply across every phase. Method names below are the
spec's shapes, not contracts — they may be renamed during TDD.

### The collect/render seam (`build_output` split)

`build_output(repo, cfg, dims) -> Result<Render>` splits into two functions so
the expensive git work and the cheap render can be triggered independently:

- **`collect_snapshot(repo, cfg) -> Result<Snapshot>`** — all the FS-driven git
  work: `branch_name`, `resolve_base` + `base_status`, `last_commit_age`,
  `collect_changes`, `collect_ages`, `build_snapshot`, `fetch_log`,
  `upstream_status`. This is the unit the `Throttle` gates.
- **`render_frame(snapshot, cfg, dims, age_offset) -> Render`** — the cheap,
  terminal-size-dependent work: the row-budget split (`plan_section_caps`, the
  `--max-files` handling, chrome math) plus `render()`. Its cost is bounded by
  terminal rows, not repo size.

`build_output` is retained as `collect_snapshot` followed by
`render_frame(.., dims, Duration::ZERO)`, so one-shot mode is unchanged.

### Age offset (Part A's render-time age advancement)

A no-git re-render advances every displayed age by a single
`age_offset = now − collected_at`. The three age sites that consume a `Duration`
all grow linearly with wall time, so one offset is exact for all of them:

1. **Header last-commit age** — `header_segments` / `snap.last_commit_age`.
2. **File-row mtime age** — `render_row` / `entry.age` (drives the format string,
   `colorize_age`, and `file_fade_factor`).
3. **Log-row age** — `render_log_row` / `entry.age` (drives the format string and
   every truecolor/ANSI fade colorizer).

The offset is applied **at render time**, inside the `render_frame`/`render`
path — `Snapshot`, `RenderEntry`, and `LogEntry` keep their existing `Duration`
fields and their extensive tests intact. `age_offset = Duration::ZERO` must be an
exact no-op: one-shot mode and the seed walk render with offset 0, so their
output stays **byte-identical** to today. The existing `should_repaint`
byte-identical suppression still absorbs no-op ticks.

### The `Throttle` (Part B's pure state machine)

A pure state machine in `watch.rs`. Owns no I/O; the loop feeds it `Instant`s and
measured `Duration`s, which makes it fully unit-testable.

- Constants: **`BUDGET = 0.01`** (1%, a hard-coded constant, not a dial),
  **`FLOOR = 150 ms`** (today's debounce window). **No ceiling.**
- Cooldown: after a walk that started at `walk_start` and took `D`,
  **`next_allowed = walk_start + max(FLOOR, D / BUDGET)`**. `D / BUDGET = 100·D`.
- Estimator: the **last measured `D`** sizes the next cooldown — no smoothing.
  A slow walk instantly earns a long cooldown (conservative on the ceiling); an
  EWMA could undershoot after a slowdown and is intentionally not used.
- Deferral: during a cooldown, FS events do not walk — they set a `dirty` flag.
  When the cooldown expires, if `dirty`, exactly one walk runs (reflecting the
  latest coalesced state) and `dirty` clears.
- Method shapes: `on_change(now) -> WalkNow | Defer`, `record(walk_start, D)`,
  `next_allowed() -> Option<Instant>`, `force()`.

### Loop state and the injected clock

`event_loop` gains:

- A **snapshot cache**: `Option<Snapshot>` + `collected_at: Instant` + the last
  `Dimensions` (so resize can re-render the cached snapshot at new dimensions).
- A **`Throttle`**.
- An **injected clock** (`Fn() -> Instant`, real `Instant::now` in production) so
  throttle/decay timing is deterministic in tests. The existing loop tests
  already inject closures, so this extends a known seam.

The loop's wait window becomes the earliest of the decay-tick cadence
(`next_tick`) and, when a walk is deferred, the `Throttle`'s `next_allowed`.

### Events and keys

- A new **`Event::ForceRefresh`** variant, produced by an **`r`** key press in
  `spawn_event_reader` (the reader thread already exists).
- `Event::Resize` and the decay tick (a `recv_timeout` timeout) become **no-git**
  re-renders of the cached snapshot.
- `Event::FsChanged` remains the only git-walk trigger (besides startup and
  forced refresh), now gated by the `Throttle`.

### Non-goals (unchanged across all phases)

- One-shot mode output stays **byte-identical**.
- *What* is rendered does not change — only *how often* git is walked.
- No configurable budget above 1%.

---

## Phase 1: Split `build_output` into `collect_snapshot` + `render_frame(age_offset)`

**User stories**: byte-identical one-shot (non-goal guard); foundation for the
smooth live display (Part A) and the gated walk (Part B).

### What to build

A pure refactor with no watch-loop changes. Carve `build_output` into
`collect_snapshot(repo, cfg) -> Result<Snapshot>` (the git work) and
`render_frame(snapshot, cfg, dims, age_offset: Duration) -> Render` (the
row-budget split + `render()`), and thread `age_offset` through to the three age
sites so a non-zero offset advances the header last-commit age, every file-row
mtime age, and every log-row age in lockstep. `build_output` becomes
`collect_snapshot` then `render_frame(.., Duration::ZERO)`. One-shot mode calls
the same path with offset 0.

The offset is added inside the render path (e.g. `render_frame` hands `render()`
the offset, or applies it to a working copy of the ages it passes down) — the
`Snapshot`/`RenderEntry`/`LogEntry` structs and their tests are untouched.

### Acceptance criteria

- [ ] `collect_snapshot(repo, cfg)` returns a `Snapshot` containing exactly what
      `build_output` assembled today (branch, base, ahead/behind, last-commit
      age, files, ages, log, upstream).
- [ ] `render_frame(snapshot, cfg, dims, Duration::ZERO)` produces output
      **byte-identical** to today's `build_output(repo, cfg, dims).output` for a
      representative snapshot (regression test over the existing render fixtures).
- [ ] A non-zero `age_offset` advances all three age sites: the header
      last-commit age, each file-row age, and each log-row age each increase by
      the offset (asserted on the rendered text / fade where observable).
- [ ] `age_offset = Duration::ZERO` is an exact no-op at every site (no `+0`
      formatting drift).
- [ ] `gsw --one-shot` and piped output remain unchanged end-to-end.
- [ ] `cargo test`, `cargo clippy`, and `cargo fmt --check` pass for `gsw`.

---

## Phase 2: Snapshot cache + age-offset re-render in the watch loop (Part A)

**User stories**: stop pinning the CPU on the idle-after-commit decay tick; keep
the live seconds/minutes counter and color fade ticking smoothly.

### What to build

Wire Part A into the live loop. Inject a clock (`Fn() -> Instant`) into
`event_loop` and give the loop a snapshot cache (`Option<Snapshot>` +
`collected_at: Instant` + last `Dimensions`). Re-route the four triggers:

| Trigger | Needs git? | Action |
|---|---|---|
| Startup | yes | one `collect_snapshot` to seed the cache, render at offset 0 |
| FS change | yes | `collect_snapshot` + re-seed cache, render at offset 0 |
| Decay tick | no | `render_frame` from the cached snapshot, `age_offset = now − collected_at` |
| Resize | no | `render_frame` from the cached snapshot at the new `Dimensions` |

After this phase, **git is walked only on FS events and startup** — decay ticks
and resizes never touch git. Throttling is still absent (FS churn is still
unbounded; Part B handles that). The existing `next_tick` decay cadence and
`should_repaint` suppression are preserved.

### Acceptance criteria

- [ ] A decay tick re-renders **without** invoking the injected git-collect
      closure (asserted via a collect-call counter in the loop test).
- [ ] A resize re-renders the cached snapshot at the new dimensions with **no**
      git collect.
- [ ] An FS change does call `collect_snapshot` and re-seeds `collected_at`.
- [ ] On a no-git re-render the displayed ages advance by `now − collected_at`
      (driven by the injected clock), and a tick whose text hasn't changed is
      suppressed by `should_repaint` (no repaint).
- [ ] Startup still seeds the cache and paints the first frame at offset 0
      (byte-identical to today's first frame).
- [ ] The existing event-loop tests (coalescing, suppression, quit, decay tick)
      still pass after the clock/cache signature change.
- [ ] Manual check: after a commit, the live counter keeps ticking but idle CPU
      drops toward ~0% (no per-second git walk).

---

## Phase 3: Pure `Throttle` state machine (no wiring)

**User stories**: foundation for the ≤1% ceiling under sustained churn.

### What to build

The pure `Throttle` state machine in `watch.rs`, fully unit-tested in isolation
with injected `Instant`s and `Duration`s — no loop integration yet. It holds
`BUDGET = 0.01`, `FLOOR = 150 ms`, the last `next_allowed`, and the `dirty` flag,
and exposes `on_change(now)`, `record(walk_start, D)`, `next_allowed()`, and
`force()`.

### Acceptance criteria

- [ ] Cooldown equals `100·D` for representative `D` (e.g. 30 ms → 3 s, 150 ms →
      15 s, 500 ms → 50 s).
- [ ] `FLOOR` clamps a sub-1.5 ms walk to a 150 ms cooldown (never faster than
      today).
- [ ] An `on_change` during an active cooldown returns `Defer` and sets `dirty`;
      after expiry exactly **one** walk is owed, and `dirty` clears once consumed.
- [ ] A slower `D` lengthens, and a faster `D` shortens, the next cooldown
      (`record` recomputes purely from the new `D`).
- [ ] `force()` makes a walk allowed immediately even mid-cooldown.
- [ ] No ceiling: a one-off large `D` (e.g. 5 s) produces a proportionally large
      cooldown (~500 s) with no cap.
- [ ] `cargo test`/`clippy`/`fmt` pass; the state machine has no I/O dependency.

---

## Phase 4: Wire the `Throttle` into the loop — measure + defer (Part B)

**User stories**: keep watch-mode CPU at ≤1% of one core under sustained FS churn.

### What to build

Drive the `Throttle` from the event loop. Measure each walk's wall-clock `D`
with the injected clock around the `collect_snapshot` call and feed it to
`record(walk_start, D)`. An FS event consults `on_change(now)`: if allowed, it
walks now; if deferred, it sets `dirty` and the loop arms a wakeup at
`next_allowed()`. The loop's wait window becomes the earliest of the decay-tick
cadence and (when dirty) the cooldown expiry; when the cooldown expires with
`dirty` set, exactly one walk runs reflecting the latest coalesced state.

Because the cooldown has long since expired after idle, the first edit after a
quiet period walks **immediately** — throttling engages only under *sustained*
churn. The existing debounce coalescing still collapses a burst before the walk.

### Acceptance criteria

- [ ] An FS event after idle walks immediately, then the next walk is held off
      until `walk_start + max(FLOOR, 100·D)`.
- [ ] A burst of FS events during an active cooldown collapses to **one**
      deferred walk that runs at cooldown expiry (collect-call counter proves a
      single walk).
- [ ] The deferred walk reflects the latest coalesced state and clears `dirty`.
- [ ] The loop wakes at the correct earliest-of(decay-tick, cooldown-expiry)
      time under the injected clock (deterministic, no real sleeping in tests).
- [ ] Decay ticks during a cooldown still re-render from cache with **no** git
      walk (Part A and Part B compose).
- [ ] Under simulated continuous churn the duty cycle is ~`D / (100·D)` = 1%;
      idle produces no walks.
- [ ] Manual check: a dev-server / log-churn scenario holds watch-mode CPU at
      ≤1% of one core while the display stays current within one cooldown.

---

## Phase 5: Manual refresh `r` key

**User stories**: give the user an escape hatch when a long cooldown (e.g. after
a transient-slow walk) would otherwise make them wait.

### What to build

Add an `r` key to `spawn_event_reader` that sends a new `Event::ForceRefresh`.
The loop handles it by calling `throttle.force()` and immediately running a walk
that re-measures `D` and resets the throttle from the fresh measurement —
bypassing any active cooldown. Key-release events stay ignored, consistent with
the existing `q`/Ctrl-C handling.

### Acceptance criteria

- [ ] Pressing `r` mid-cooldown forces an immediate `collect_snapshot` and
      repaints (collect-call counter increments despite an unexpired cooldown).
- [ ] The forced walk re-measures `D` and re-arms the cooldown from it (a
      subsequent FS event is throttled against the fresh measurement, not the
      stale one).
- [ ] `r` on a clean/idle loop simply walks and renders (no error, no double
      walk).
- [ ] Key-release events for `r` are ignored (no spurious refresh on kitty /
      Windows release reports).
- [ ] The existing reader-driven events (`q`, Ctrl-C, resize) are unaffected.
- [ ] Manual check: `r` produces an instant refresh during a long cooldown.

---

## Sequencing notes

- **Phase 1 is a prerequisite** for both Phase 2 (needs `render_frame` + offset)
  and Phase 4 (needs `collect_snapshot` as the gated unit).
- **Phases 1–2 deliver Part A** (the dominant idle-after-commit win) and are
  independently shippable even before Part B lands.
- **Phases 3–4 deliver Part B**; Phase 3's pure machine is verifiable by unit
  tests alone, and Phase 4 wires it in.
- **Phase 5** is a thin escape-hatch slice on top of the wired throttle.
- Every phase follows TDD red → commit → green → commit, and ends green
  (`cargo test`/`clippy`/`fmt` for `gsw`). The loop tests already inject
  closures, so the clock/cache/throttle signature churn is contained.
