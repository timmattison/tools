# Plan: seescc — sccache stats viewer

> Source PRD/spec: `specs/2026-06-02-seescc-design.md`

A self-refreshing terminal viewer for sccache statistics. Config-selected stats
(Rust-focused by default), in-memory sparkline history, `--one-shot` mode, no `viddy`.
Sibling to `gsw`; reuses its `crossterm` + `colored` stack and pure-core/thin-shell split.

Each phase is a vertical tracer bullet — poll → parse → aggregate/filter → render → output
— that lands a runnable `seescc` binary doing strictly more than the previous phase. Every
phase is built **test-first (red → commit → green → commit)** per the repo's mandatory TDD
rule; pure modules carry the behavioral tests, the subprocess/terminal shells stay thin.

## Architectural decisions

Durable decisions that apply across all phases (from the spec):

- **Data source**: `sccache --show-stats --stats-format=json` per poll, parsed with `serde`.
  Rides sccache's own IPC; no raw socket protocol. Confirmed against sccache 0.15.0.
  Deserialize defensively (`#[serde(default)]`) so unknown/added fields never break parsing.
- **Crate location**: `src/seescc/` — auto-joined to the workspace via `members = ["src/*"]`.
  Binary name `seescc`. `--version` via `buildinfo::version_string!()`.
- **Dependencies**: existing workspace deps only — `anyhow`, `thiserror`, `clap`, `serde`,
  `serde_json`, `toml`, `dirs`, `crossterm`, `colored`, `terminal_size`, `unicode-width`,
  `num-format`, `human_bytes`, `which`, `buildinfo`; `tempfile` (dev). No new crate.
- **Module split** (mirrors `gsw`): pure & unit-testable — `stats.rs` (serde types +
  `parse`), `config.rs` (TOML/defaults/duration-parser/metric-catalog), `aggregate.rs`
  (language-filtered counters + `hit_rate`), `history.rs` (timestamped ring buffer,
  prune-by-window, bucket-to-N), `sparkline.rs` (`&[f64] -> String`), `render.rs`
  (`build_output(...) -> String`); thin shells — `watch.rs` (timer loop, keyboard thread,
  resize, repaint suppression, `TerminalGuard` RAII), `main.rs` (clap, `which`, mode wiring).
- **Mode decision** (mirrors `gsw`): `--one-shot || !stdout_is_tty → OneShot`, else `Watch`.
- **Config**: TOML; precedence `--config` > `$XDG_CONFIG_HOME/seescc/config.toml`
  (`dirs::config_dir()`) > built-in defaults. Schema:
  `poll_interval` / `window` (duration strings), `languages` (per-language filter; `[]` =
  all), `metrics = [{ key, label?, spark? }]` in display order.
- **Metric-key catalog**: per-language (respect `languages`) — `cache_hits`, `cache_misses`,
  `cache_errors`, `hit_rate`; global — `compile_requests`, `requests_executed`,
  `requests_not_cacheable`, `requests_not_compile`, `requests_unsupported_compiler`,
  `cache_writes`, `compilations`, `compile_fails`, `forced_recaches`, `cache_size`,
  `max_cache_size`. Unknown key → load error listing the catalog.
- **Default metric set** (Rust-focused, compact): `compile_requests`, `requests_executed`,
  `cache_hits` (spark), `cache_misses` (spark), `hit_rate` (spark); `languages = ["Rust"]`;
  `poll_interval = "1s"`; `window = "15m"`.
- **Sparkline semantics**: glyphs `▁▂▃▄▅▆▇█`; samples bucketed to the available width;
  counters spark per-bucket **deltas** (auto-scaled), `hit_rate` sparks the **windowed**
  rate per bucket (clamped 0–100, no NaN), size metrics spark absolute value. History is
  **in-memory only** (empty at launch, fills over `window`).
- **Number formatting**: counts via `num-format` (`4,786`); sizes via `human_bytes`
  (`809 MB`); `hit_rate` as `64.1%`.
- **UTF-8 safety**: display widths via `unicode-width`; never byte-slice strings. Tests
  include multi-byte labels (日本語 / café / emoji).
- **Parallel-safe tests**: every temp file/dir or `PATH`-stub keyed on `pid` + nanosecond
  timestamp; no hardcoded shared paths.

---

## Phase 1: One-shot skeleton & default human output

**User stories / goals**: Rust-only default stats, hiding C/C++ & Assembler (G1); compact
default display (G2); `--one-shot` single render for scripting/piping (G4).

### What to build

A complete poll → parse → aggregate → render → stdout path. Scaffold `src/seescc/` with a
clap CLI exposing `--one-shot` and `-V/--version`. At startup, detect `sccache` with
`which`. Run `sccache --show-stats --stats-format=json`, parse into typed `Stats`
(`stats.rs`, defensive serde). Filter the per-language hit/miss buckets to Rust and compute
hit rate (`aggregate.rs`). Render the **hardcoded default 5 metrics** (compile requests,
requests executed, Rust cache hits, Rust cache misses, Rust hit rate) as a plain,
right-aligned human table with the header clock (`render.rs`). Pick one-shot mode when
`--one-shot` is set or stdout is not a TTY.

### Acceptance criteria

- [ ] `seescc --one-shot` (with a running sccache) prints the 5 default metrics with real
      values; Rust hits/misses/hit-rate reflect only the `Rust` bucket.
- [ ] `seescc -V` prints `seescc <version> (<hash>, <clean|dirty>)` via `buildinfo`.
- [ ] `sccache` not on `PATH` → clear error message, non-zero exit, no panic.
- [ ] A failed/garbled poll → error to stderr, non-zero exit.
- [ ] `stats.rs` parses the captured 0.15.0 fixture; tolerates unknown extra fields and
      empty `counts` maps. (red/green committed)
- [ ] `aggregate.rs` hit-rate matches the fixture (1718/(1718+963) ≈ 64.1%) and returns a
      defined value (not NaN) when hits + misses = 0. (red/green committed)
- [ ] Crate builds clean under `cargo clippy` with workspace lints.

---

## Phase 2: Config file & CLI overrides

**User stories / goals**: user chooses which stats and which languages to show (G1);
configurable poll interval and history window (G3 groundwork).

### What to build

`config.rs`: load TOML from `--config` else the XDG path else built-in defaults; the
built-in defaults expressed as a parsed `Config` (the Phase 1 hardcoded set moves here).
The duration parser (`ms`/`s`/`m`/`h`; invalid → error). The metric-key catalog with
validation — an unknown `key` is a load error that prints the valid catalog. `languages`
filter applied to per-language metrics (`[]` = sum all). `--write-default-config`
(+`--force`) scaffolds an annotated config at the resolved path. `--poll-interval` /
`--window` CLI flags override config values. One-shot output is now fully config-driven
(metric set, order, labels, languages).

### Acceptance criteria

- [ ] With no config file present, output is identical to Phase 1's defaults.
- [ ] A config selecting a different metric set / `languages` changes the one-shot output
      accordingly (e.g. `languages = []` sums all languages; adding `cache_writes` shows it).
- [ ] Unknown metric key in config → error naming the bad key and listing the catalog.
- [ ] Duration parser accepts `500ms`/`1s`/`15m`/`1h` and rejects garbage. (red/green)
- [ ] `--config <path>` takes precedence over the XDG path. (red/green)
- [ ] `seescc --write-default-config` writes an annotated config; refuses to overwrite an
      existing file without `--force`.
- [ ] `--poll-interval`/`--window` override config; config-file tests use pid+nanos paths.

---

## Phase 3: `--format json` one-shot

**User stories / goals**: scripting-friendly one-shot output (G4).

### What to build

Add `--format human|json` (default `human`). In one-shot, `json` emits the selected,
language-filtered metrics as a single JSON object keyed by metric key (counts as numbers,
`hit_rate` as a number). `render.rs` gains a json renderer alongside the human one.

### Acceptance criteria

- [ ] `seescc --one-shot --format json` emits valid JSON of exactly the configured metrics,
      respecting the `languages` filter, and pipes cleanly into `jq`.
- [ ] `--format human` is unchanged from Phase 2.
- [ ] json renderer covered by a unit test against the fixture. (red/green)

---

## Phase 4: Live watch mode (no sparklines yet)

**User stories / goals**: self-refreshing live view with no `viddy` or external wrapper (G5).

### What to build

`watch.rs`: the timer poll loop (`poll_interval`), alternate-screen entry with a
`TerminalGuard` RAII that restores the terminal on exit/panic, a keyboard thread (`q`/`Esc`/
`Ctrl-C` quit), terminal-resize → recompute + repaint, and byte-compare repaint suppression
so an idle server doesn't flicker. The live frame adds the header clock and the footer
(cache size / max + window length). A failed poll shows a non-fatal error banner while
keeping the last good frame on screen. Mode decision wired in `main.rs`: watch when stdout
is a TTY and `--one-shot` absent. Force-color logic (à la `gsw`) so colors survive a pipe.

### Acceptance criteria

- [ ] Running `seescc` in a terminal shows the live frame and refreshes at `poll_interval`
      without `viddy`.
- [ ] `q`, `Esc`, and `Ctrl-C` each exit cleanly and the terminal is fully restored
      (no leftover alternate screen, cursor visible).
- [ ] Resizing the terminal re-lays-out the frame.
- [ ] A transient poll failure shows an error banner and retains the last good numbers;
      recovery clears the banner.
- [ ] Idle server → no repaint churn (repaint-suppression verified on the pure render path).
- [ ] Mode-decision helper unit-tested (`--one-shot`/non-TTY → one-shot). (red/green)

---

## Phase 5: Sparklines & history

**User stories / goals**: ~15-minute sparkline history per selected metric (G3).

### What to build

`history.rs`: a timestamped (`Instant`) ring buffer of `Stats` snapshots, pruned to
`window`, with bucketing of N samples into W width-columns (decoupling poll cadence from
column count). `sparkline.rs`: `&[f64] -> String` of `▁▂▃▄▅▆▇█` with per-series auto-scale
(no divide-by-zero when all values equal). `render.rs`/`watch.rs` wire `spark = true` rows:
counters spark per-bucket deltas, `hit_rate` sparks the windowed rate (clamped 0–100,
baseline on zero-activity buckets). A mid-run `sccache --zero-stats` (counter drop) clamps
deltas to 0 — no spurious spike — and history continues from the new baseline. Narrow
terminals shrink then drop the sparkline before sacrificing the numbers; widths computed via
`unicode-width`.

### Acceptance criteria

- [ ] Live view shows sparklines for `cache_hits`, `cache_misses`, `hit_rate` that fill in
      over time (empty at launch, populated as samples accrue).
- [ ] Bucketing maps N samples to W columns correctly; prune drops entries older than
      `window`. (red/green)
- [ ] `sparkline.rs` maps known series to known glyphs; all-equal series doesn't panic;
      single value handled. (red/green)
- [ ] Counter sparklines show per-bucket deltas; `hit_rate` shows the windowed rate; a
      simulated stats reset clamps to 0 with no negative spike. (red/green)
- [ ] Narrow terminal drops the sparkline before truncating numbers; multi-byte labels
      (日本語 / café / emoji) keep alignment. (red/green)
- [ ] End-to-end integration test runs the built binary in `--one-shot` against a stub
      `sccache` on a pid+nanos `PATH` emitting fixture JSON (no live server needed).

---

## Definition of done (whole tool)

- [ ] All five phases complete, each committed via TDD red/green.
- [ ] `cargo test`, `cargo clippy`, and `cargo build` clean for the `seescc` crate.
- [ ] `seescc` and `seescc --one-shot` both work against the local sccache server.
- [ ] README/usage doc for `seescc` (config schema + metric catalog + examples).
- [ ] Tool entry added to any repo tool index alongside `gsw`.
