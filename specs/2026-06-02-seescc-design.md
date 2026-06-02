# seescc — sccache stats viewer (design)

**Date:** 2026-06-02
**Status:** Approved (brainstorming complete)
**Branch:** `seescc`

## 1. Purpose

`seescc` is a self-refreshing terminal viewer for [sccache](https://github.com/mozilla/sccache)
statistics. It polls the running sccache server, displays a **small, config-selected**
subset of stats (Rust-focused by default), and draws Unicode sparklines showing recent
history. It needs no `viddy` or other external refresh wrapper — it refreshes itself.

It is a sibling to the repo's `gsw` tool and deliberately reuses `gsw`'s rendering stack:
`crossterm` + `colored`, alternate screen with an RAII restore guard, hand-rolled Unicode
output, and a clean split between pure (testable) logic and thin terminal/subprocess shells.

### Goals

- Show only the stats the user cares about — by default the Rust compile/cache numbers,
  with C/C++ and Assembler buckets hidden.
- Default display is intentionally compact.
- A 15-minute (configurable) sparkline history per selected metric.
- A `--one-shot` mode for a single render (scripting / piping / quick check).
- No dependency on `viddy` or any external polling wrapper.

### Non-goals (WISHLIST)

- Reimplementing sccache's raw socket wire protocol.
- Persisting history across runs.
- A keybinding that zeroes sccache stats (`--zero-stats`).
- Multi-server / remote / distributed sccache views.

## 2. Data source & polling model

### Source

Each poll runs:

```
sccache --show-stats --stats-format=json
```

and parses the JSON with `serde`. This rides sccache's own IPC — the `sccache` client
connects to the server socket for us — so it stays stable across sccache versions
(reimplementing the length-prefixed bincode wire protocol would couple us to sccache
internals and break silently on upgrades). Confirmed against sccache **0.15.0**.

`which` is used at startup to detect the `sccache` binary; if it is absent, exit with a
clear error. A failed or unparseable poll while live shows a non-fatal error banner and
keeps displaying the last good frame; in `--one-shot` it exits non-zero with the error.

### Captured JSON shape (sccache 0.15.0)

```jsonc
{
  "stats": {
    "compile_requests": 4786,
    "requests_unsupported_compiler": 0,
    "requests_not_compile": 54,
    "requests_not_cacheable": 852,
    "requests_executed": 3880,
    "cache_errors":  { "counts": {}, "adv_counts": {} },
    "cache_hits":    { "counts": { "Assembler": 196, "Rust": 1718, "C/C++": 516 }, "adv_counts": { /* ... */ } },
    "cache_misses":  { "counts": { "Assembler": 98,  "Rust": 963,  "C/C++": 312 }, "adv_counts": { /* ... */ } },
    "cache_timeouts": 0,
    "cache_read_errors": 0,
    "non_cacheable_compilations": 0,
    "forced_recaches": 0,
    "cache_write_errors": 0,
    "cache_writes": 1373,
    "cache_write_duration":   { "secs": 10,  "nanos": 59067351 },
    "cache_read_hit_duration":{ "secs": 18,  "nanos": 802462789 },
    "compilations": 1373,
    "compiler_write_duration":{ "secs": 884, "nanos": 919613679 },
    "compile_fails": 75,
    "not_cached": { "-o": 33, "crate-type": 598, "-": 68, "missing input": 151, "-E": 2 },
    "dist_compiles": {},
    "dist_errors": 0,
    "multi_level": null
  },
  "cache_location": "Local disk: \"/Users/.../sccache\"",
  "cache_size": 809212237,
  "max_cache_size": 10737418240,
  "use_preprocessor_cache_mode": true,
  "version": "0.15.0",
  "basedirs": []
}
```

Key observation that drives the "Rust only" feature: `cache_hits` / `cache_misses` /
`cache_errors` are **per-language** maps (`counts`), while `compile_requests` and
`requests_executed` are **global** counters with no per-language breakdown. "Rust only"
therefore means: keep the global request counters, and restrict the per-language maps to
the `Rust` bucket.

We deserialize defensively: unknown fields are ignored (`#[serde(default)]` on every field
we read) so future sccache versions that add fields don't break parsing. We do **not**
hard-fail if a field we don't use disappears.

### Polling loop

sccache exposes no filesystem event to watch (unlike `gsw`, which watches the git dir via
`notify`). So `seescc` is a **timer loop**:

1. Poll sccache at `poll_interval`, parse, push a timestamped snapshot into the history
   ring buffer, prune entries older than `window`.
2. Recompute the output string.
3. Repaint only if the rendered bytes changed (byte-compare suppression, like `gsw`), so an
   idle server doesn't flicker.

A dedicated keyboard thread reads `crossterm` events: `q`, `Esc`, and `Ctrl-C` quit; a
terminal resize event forces a recompute + repaint. The main loop selects between the
poll timer and the keyboard channel.

## 3. Configuration

### Location & precedence

- `--config <path>` (explicit) wins.
- Else `$XDG_CONFIG_HOME/seescc/config.toml`, resolving via the `dirs` crate
  (`dirs::config_dir()` → `~/.config/seescc/config.toml` on Linux/macOS).
- If no config file exists, **built-in defaults are used** (the Rust-focused set below).
- `--write-default-config` writes an annotated default config to the resolved path
  (creating parent dirs) and exits. Refuses to overwrite an existing file unless `--force`.

### Schema (TOML)

```toml
poll_interval = "1s"      # how often to query sccache
window        = "15m"     # sparkline history retention
languages     = ["Rust"]  # per-language metrics filtered to these; [] = all languages

# Rows to show, in order. `label` optional (pretty default per key). `spark` defaults false.
metrics = [
  { key = "compile_requests",  label = "Compile requests" },
  { key = "requests_executed", label = "Requests executed" },
  { key = "cache_hits",        label = "Cache hits",   spark = true },
  { key = "cache_misses",      label = "Cache misses", spark = true },
  { key = "hit_rate",          label = "Hit rate",     spark = true },
]
```

`poll_interval` and `window` are parsed by a small in-crate duration parser (no new
dependency) supporting integer + unit suffix: `ms`, `s`, `m`, `h` (e.g. `500ms`, `1s`,
`15m`, `1h`). Invalid durations are a config load error.

`languages` filters every **per-language** metric (`cache_hits`, `cache_misses`,
`cache_errors`, `hit_rate`). An empty list (`[]`) means "sum across all languages".
A language name that never appears in sccache output simply contributes 0 (not an error —
sccache only lists languages it has seen).

### Metric key catalog

Per-language (respect `languages`):

| key                 | source                                  |
| ------------------- | --------------------------------------- |
| `cache_hits`        | sum of `stats.cache_hits.counts[lang]`  |
| `cache_misses`      | sum of `stats.cache_misses.counts[lang]`|
| `cache_errors`      | sum of `stats.cache_errors.counts[lang]`|
| `hit_rate`          | derived: hits / (hits + misses) × 100   |

Global counters (ignore `languages`):

| key                             | source                                  |
| ------------------------------- | --------------------------------------- |
| `compile_requests`              | `stats.compile_requests`                |
| `requests_executed`             | `stats.requests_executed`               |
| `requests_not_cacheable`        | `stats.requests_not_cacheable`          |
| `requests_not_compile`          | `stats.requests_not_compile`            |
| `requests_unsupported_compiler` | `stats.requests_unsupported_compiler`   |
| `cache_writes`                  | `stats.cache_writes`                    |
| `compilations`                  | `stats.compilations`                    |
| `compile_fails`                 | `stats.compile_fails`                   |
| `forced_recaches`               | `stats.forced_recaches`                 |
| `cache_size`                    | top-level `cache_size` (bytes)          |
| `max_cache_size`                | top-level `max_cache_size` (bytes)      |

An unknown `key` in config is a load error that prints the full valid catalog.

Number formatting: counts use `num-format` (`4,786`); `cache_size` / `max_cache_size` use
`human_bytes` (`809 MB`). `hit_rate` renders as `64.1%`.

## 4. Display

### Live (watch) frame

```
sccache · Rust                 12:34:56

 Compile requests   4,786
 Requests executed   3,880
 Cache hits          1,718  ▁▂▃▅▇█▆▄
 Cache misses          963  ▁▁▂▂▃▂▁▁
 Hit rate            64.1%  ▆▆▇▇▇▆▆▇

 cache 809 MB / 10 GB · 15m window
```

- Header: `sccache · <languages joined>` (or `sccache · all`) on the left, current
  wall-clock time on the right.
- Each row: right-aligned label column, the **current cumulative** value, then (if
  `spark = true`) the sparkline.
- Footer: cache size / max size and the history window length.
- Colors via `colored`, following `gsw`'s force-color logic so colors survive being piped
  through a wrapper (`set_override(true)` when not a TTY but `COLUMNS` is present and
  `NO_COLOR` is unset).

### Sparkline semantics

- Glyphs: `▁▂▃▄▅▆▇█` (8 levels).
- The history ring buffer stores `(Instant, Stats)` samples at `poll_interval`. At render
  time, samples are **bucketed to fit the available terminal width** — the `window` is
  divided into N columns where N is the sparkline width budget, decoupling poll cadence
  from column count (a 1 s poll over a 15 m window aggregates smoothly into ~width columns).
- **Counter metrics** (`cache_hits`, `cache_misses`, `compile_requests`, …) spark the
  **per-bucket delta** (activity in that time slice), auto-scaled to the metric's own
  min..max over the window so the shape is visible regardless of magnitude.
- **`hit_rate`** sparks the **windowed** rate per bucket — `bucket_hits / (bucket_hits +
  bucket_misses)` — clamped to 0..100. Buckets with no hit/miss activity render at baseline
  (`▁`), never NaN.
- **Size metrics** (`cache_size`) spark the absolute value per bucket (not a delta).
- History is **in-memory only**: empty at launch, fills over `window`. Rationale: sccache
  exposes no historical series, so we could only ever persist what this process observed;
  not worth the state-file complexity.

### One-shot

`--one-shot` (or stdout not a TTY) renders once and exits — mirroring `gsw`'s mode
decision: `force_one_shot || !stdout_is_tty → OneShot`.

- `--format human` (default): the same rows as plain numbers, **no sparklines** (a single
  sample has no history). Header time still shown.
- `--format json`: emits the filtered, selected metrics as a JSON object (e.g.
  `{"compile_requests":4786,"requests_executed":3880,"cache_hits":1718,"cache_misses":963,
  "hit_rate":64.06}`) for scripting. Cheap to add since we already parse JSON.

## 5. Architecture

Modules mirror `gsw`'s split — pure logic is unit-testable with no TTY and no real sccache:

| module         | responsibility                                                               | pure? |
| -------------- | ---------------------------------------------------------------------------- | ----- |
| `stats.rs`     | serde types for sccache JSON + `parse(&str) -> Result<Stats>`                | yes   |
| `config.rs`    | TOML load/validate, built-in defaults, duration parser, metric-key catalog   | yes   |
| `aggregate.rs` | language-filtered counter extraction; `hit_rate` computation                 | yes   |
| `history.rs`   | timestamped ring buffer (monotonic `Instant`), prune-by-window, bucket-to-N  | yes   |
| `sparkline.rs` | `&[f64] -> String` block-char sparkline with auto-scale                      | yes   |
| `render.rs`    | `build_output(stats, history, config, dims) -> String`; one-shot human/json  | yes   |
| `watch.rs`     | timer loop, keyboard thread, resize, repaint suppression, `TerminalGuard`    | no    |
| `main.rs`      | clap args, `which` check, mode decision, wiring                              | no    |

### CLI (clap derive)

```
seescc [--one-shot] [--config <path>] [--format human|json]
       [--write-default-config] [--force] [--poll-interval <dur>] [--window <dur>]
       [-V | --version]
```

- `--version` via `buildinfo::version_string!()` → `seescc 0.1.0 (abc1234, clean)`.
- `--poll-interval` / `--window` override config values when given.
- `--format` is meaningful in one-shot; in watch mode it's ignored (always the live view).

### `Cargo.toml`

All dependencies are existing workspace deps (no new crate):

```toml
anyhow.workspace = true
buildinfo.workspace = true
clap.workspace = true
colored.workspace = true
crossterm.workspace = true
dirs.workspace = true
human_bytes.workspace = true
num-format.workspace = true
serde.workspace = true
serde_json.workspace = true
terminal_size.workspace = true
thiserror.workspace = true
toml.workspace = true
unicode-width.workspace = true
which.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

The crate is added to the workspace automatically via `members = ["src/*"]`; the new tool
lives at `src/seescc/`.

## 6. Error handling & edge cases

- **sccache not installed** → startup error via `which`, non-zero exit, no terminal takeover.
- **Poll fails / non-zero exit / unparseable JSON** → watch: error banner + keep last good
  frame; one-shot: print error to stderr, exit non-zero.
- **No server running** → `sccache --show-stats` auto-starts the server; handled like a
  normal poll. If it still errors, treated as a failed poll.
- **Zero activity** → counter sparklines are all-baseline; `hit_rate` with 0 hits + 0
  misses renders `—`/`0.0%` (decision: show `0.0%` with baseline sparkline; never NaN).
- **Stats reset mid-run** (user runs `sccache --zero-stats`): cumulative counters drop. A
  bucket delta would go negative; deltas are clamped at 0 so the sparkline shows a flat
  slice rather than a spurious spike, and history continues from the new baseline.
- **Narrow terminal** → label/value columns take priority; sparkline width shrinks to a
  minimum (e.g. 4 cols) and is dropped entirely if there's no room. UTF-8/display-width via
  `unicode-width`; no byte-slicing of strings.
- **Multi-byte labels** → handled by `unicode-width`; covered by tests.

## 7. Testing (TDD)

Every pure module is built test-first (red → commit → green → commit). Tests:

- `stats.rs`: parse the captured 0.15.0 fixture; tolerate added/removed unknown fields;
  empty `counts` maps.
- `config.rs`: default config loads; duration parser (`500ms`/`1s`/`15m`/`1h`, invalid →
  error); unknown metric key → error listing catalog; `--config` precedence.
- `aggregate.rs`: `languages = ["Rust"]` sums only Rust; `[]` sums all; `hit_rate` math
  including the 0+0 → no-NaN case and a known fixture (1718/(1718+963) ≈ 64.08%).
- `history.rs`: prune-by-window; bucketing N samples into W columns; delta clamping on
  reset; empty buffer.
- `sparkline.rs`: known series → known glyphs; single value; all-equal (no divide-by-zero
  in scaling); auto-scale range.
- `render.rs`: snapshot of the default frame; one-shot human + json output; narrow-width
  layout; multi-byte label widths (日本語 / café / emoji).
- Integration: run the built binary in `--one-shot` against a **stub `sccache`** placed on
  a per-test `PATH` (a tiny script emitting fixture JSON) so the test never needs a live
  server. Stub path/dir keyed on `pid + nanos` for parallel safety (no hardcoded shared
  paths).

All temp files/dirs and any shared resources use process-unique paths
(`pid` + nanosecond timestamp) per the repo's parallel-safety rule.

## 8. Open items resolved

- Data source: **sccache CLI JSON** (not raw socket).
- Rendering: **lean crossterm + colored** (match `gsw`), hand-rolled sparkline.
- Sparkline scope: **per-metric, configurable**; default enables `cache_hits`,
  `cache_misses`, `hit_rate`.
- Name: **`seescc`**.
- One-shot `--format json`: **included**.
- History: **in-memory only**.
