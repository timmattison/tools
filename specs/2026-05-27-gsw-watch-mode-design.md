# gsw watch mode — design

**Date:** 2026-05-27
**Status:** Approved (design); pending implementation plan
**Component:** `src/gsw`

## Problem

`gsw` is a one-shot git-status renderer designed to be polled by `viddy`. The
zellij worktree layout runs `viddy gsw` in a pane per worktree, so `viddy`
re-executes `gsw` every ~2 seconds **unconditionally**. With many worktrees open
that means N independent processes each doing a full `gix` working-tree status
walk (including untracked-file collection) every 2 s, whether or not anything
changed. The work is wasted whenever the repo is idle, which is most of the time.

> Note on the original report: `gsw` does **not** shell out to `ugrep` (it is not
> installed on the system) or to anything else — it reads the repo in-process via
> `gix`. The cost is the fixed-interval polling, not a subprocess.

We want `gsw` to drive its own refresh from real filesystem events plus a
time-decay timer, eliminating the `viddy` dependency for the `gsw` pane and
dropping idle CPU to ~zero.

## Goals

- Re-render only when something that affects the output actually changes.
- Keep the age-based color decay and age text fresh over time without FS events.
- Drop the `viddy` wrapper for the `gsw` pane.
- Stay scriptable: piped/non-TTY usage keeps the current one-shot behavior.
- Honor the repo's mandatory TDD workflow and parallel-safe test rules.

## Non-goals

- No config file, no tunable intervals/flags beyond `--one-shot` (YAGNI).
- No scrollback/streaming output mode; watch mode takes over the pane.
- No change to the rendered *content* or its layout math beyond the size-source
  split described below.

## CLI / mode selection

- `gsw` (no args) → **watch mode** (new default).
- `gsw --one-shot` → current single-render-and-exit behavior, unchanged.
- **Auto-fallback:** if stdout is not a TTY (piped/captured), watch mode behaves
  exactly as `--one-shot`. Keeps `gsw | …` working and protects against stale
  `viddy gsw` wiring.
- Not in a git repo (or bare repo) → print the existing one-shot output and exit;
  do not enter the watch loop.
- `--version`/`-V` continue to use `buildinfo::version_string!()` as today.

## Architecture (chosen approach: `crossterm` + `notify` + `ignore`, mpsc loop)

A single `std::sync::mpsc` channel receives four event kinds; the main thread
owns all rendering. No async runtime.

```
                 ┌─────────────────────────┐
 notify watcher  │ FsChanged(path)         │  filtered by ignore matcher
 thread ────────▶│                         │  before send
                 │                         │
 timer thread ──▶│ Tick                    │  adaptive cadence
                 │                         │
 crossterm event │ Resize | Quit           │  q / Ctrl-C / SIGWINCH
 reader thread ─▶│                         │
                 └───────────┬─────────────┘
                             │ recv_timeout (debounce window)
                             ▼
                   main loop: coalesce → render
```

### Event sources

- **`notify`** recursive watcher rooted at the worktree root. macOS uses FSEvents
  (kernel-coalesced, cheap). Each event's path is tested against an `ignore`-crate
  matcher built from the repo's full ignore set (`.gitignore`, nested ignores,
  `.git/info/exclude`, global excludes). Ignored paths (`target/`, `node_modules/`,
  …) are dropped before they reach the channel — this matches what `gix status`
  already honors, so the watch filter and the rendered view agree by construction.
- **`.git/`** is watched wholesale (no curated allowlist). Its churn (objects, logs,
  packs, lockfiles, background `gc`) is absorbed by the debounce window and by the
  byte-identical-output suppression below, so it cannot cause visible flicker.
- **Timer thread** emits `Tick` on an adaptive cadence (see below).
- **`crossterm` event reader thread** translates key events (`q`, `Ctrl-C`) into
  `Quit` and terminal resize into `Resize`.

### Debounce / coalescing

The main loop uses `recv_timeout` with a ~150 ms quiet window: after the first
event it keeps draining the channel until 150 ms pass with no new event, then does
one render. A `git commit` (a burst of `.git/` writes) thus produces a single
repaint.

### Byte-identical-output suppression

After computing the render string, compare it to the previously displayed string.
If identical, skip the repaint entirely. This makes the "watch all of `.git/`"
choice robust: object/pack/log churn that doesn't change the visible state costs at
most one status walk, never a repaint.

### Adaptive timer cadence

Recomputed after every render from the **freshest** displayed item (newest commit
or working-tree change), driven by the existing fade model in `age.rs`
(linear ramp 0→`FADE_DARKEST_AT` = 2 h, then frozen at `FADE_FLOOR`):

| Freshest item age | Tick interval | Rationale |
| --- | --- | --- |
| `< 1 min` | 1 s | live seconds in the age text + fade moving |
| `1 min – 2 h` | ~60 s | fade moves ~1 RGB unit/min; minute text updates |
| `≥ 2 h` | timer disabled | fade frozen at floor; FS events only; idle cost ≈ 0 |

The cadence is a pure function of the freshest age and is unit-tested directly.

## Layout source split

The current render reads `COLUMNS`/`LINES` from the environment and reserves rows
for *viddy's* chrome (`VIDDY_CHROME_ROWS`). In **watch mode** `gsw` owns the whole
pane, so it takes width/height from `terminal_size` directly and reserves **no**
viddy chrome rows. The `--one-shot` path keeps the existing viddy-aware env logic
untouched for backward compatibility. The size-source therefore keys off the mode,
not off ambient env detection.

## Terminal lifecycle

- On entering watch mode: enter alternate screen, hide cursor.
- A guard (`Drop` + panic hook) restores the main screen and cursor no matter how
  the process exits, so a panic can never leave the terminal wedged.
- `q` or `Ctrl-C` quits cleanly.
- `Resize` triggers an immediate re-render at the new size.

## New dependencies

- `notify` — filesystem watching (FSEvents on macOS).
- `ignore` — ripgrep's ignore-set matcher.
- `crossterm` — alternate screen, cursor control, key/resize events.

All are mainstream, widely used Rust crates. No async runtime is introduced.

## Module shape (within `src/gsw/src`)

Keep units small and independently testable. Proposed new module(s):

- `watch.rs` — the event loop, channel wiring, terminal lifecycle guard. Thin;
  delegates all decisions to pure functions below.
- Pure, terminal-free functions (in `watch.rs` or a small sibling), each
  unit-tested:
  - `next_tick(freshest_age: Duration) -> Option<Duration>` — adaptive cadence
    (returns `None` when the timer is disabled).
  - `should_react(path, ignore_matcher, git_dir) -> bool` — event classification
    (drop ignored worktree paths; accept `.git/` paths; accept tracked/untracked
    non-ignored worktree paths).
  - render-equality check for suppression (string compare; trivial but exercised
    in the loop test).

`render.rs`, `repo.rs`, `snapshot.rs`, `age.rs`, `git.rs`, `bar.rs` are reused
as-is; the only change to existing render code is threading the size source
(env vs. `terminal_size`) through based on mode.

## Testing

Per the repo's **mandatory** TDD workflow (red→commit→green→commit), parallel-safe
(temp repos keyed on `pid` + nanos; never hardcode shared paths):

- Unit: `next_tick` boundaries (just under/over 1 min, 2 h).
- Unit: `should_react` — ignored worktree path dropped, `target/` dropped,
  tracked/untracked non-ignored path accepted, `.git/HEAD` accepted, `.git/objects`
  path accepted-but-suppressed-downstream (classification accepts; suppression is a
  separate concern tested separately).
- Unit: byte-identical-output suppression (same snapshot → no repaint signal).
- Unit: layout source split — one-shot uses env, watch uses `terminal_size`-derived
  dimensions (inject dimensions; assert the chosen source).
- Integration smoke test: spawn watch mode against a temp repo with stdout **not** a
  TTY, assert it falls back to one-shot (renders once and exits) — this exercises the
  mode-selection and fallback paths without a pty.
- Existing `--one-shot` / viddy integration tests stay green unchanged.

Terminal-control byte sequences are not asserted; the lifecycle guard is verified by
construction (RAII) rather than by capturing escape codes.

## Rollout

Update `~/.zshrc##template.default`'s zellij layout: the gsw pane changes from

```
pane name="gsw" cwd="$worktree" command="viddy" {
    args "gsw"
}
```

to

```
pane name="gsw" cwd="$worktree" command="gsw"
```

`viddy` stays installed for the `sccache --show-stats` pane; `gsw` simply no longer
needs it. (Edit the yadm template, not the rendered `~/.zshrc`.)

## Risks / open considerations

- **Many watchers:** N worktrees → N FSEvents watchers, each scoped to its own
  subtree. FSEvents is cheap and there is no shared state, so this is fine and is a
  strict improvement over N polling loops.
- **`.git` churn under heavy git activity:** absorbed by debounce + suppression;
  worst case is extra status walks (cheap) with no repaint.
- **Non-macOS:** `notify` falls back to the platform backend (inotify/kqueue);
  design is platform-neutral, though the primary target is macOS.
