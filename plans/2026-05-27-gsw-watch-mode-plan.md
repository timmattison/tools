# Plan: gsw watch mode

> Source PRD/spec: `specs/2026-05-27-gsw-watch-mode-design.md`

## Architectural decisions

Durable decisions that apply across all phases:

- **CLI / modes**:
  - `gsw` (no args) → watch mode (new default).
  - `gsw --one-shot` → today's single-render-and-exit behavior, unchanged.
  - stdout not a TTY (piped/captured) → auto-fallback to one-shot.
  - not in a git repo (or bare repo) → print the existing one-shot output and exit; never enter the watch loop.
  - `--version`/`-V` keep using `buildinfo::version_string!()`.
- **Event model**: a single `std::sync::mpsc` channel carrying four event kinds — `FsChanged(path)`, `Tick`, `Resize`, `Quit`. The main thread owns all rendering. No async runtime.
- **Dependencies**: `notify` (FS watching / FSEvents), `ignore` (ripgrep ignore-set matcher), `crossterm` (alternate screen, cursor, key/resize events).
- **Pure, terminal-free, unit-tested functions**:
  - `next_tick(freshest_age: Duration) -> Option<Duration>` — adaptive cadence (`None` = timer disabled).
  - `should_react(path, ignore_matcher, git_dir) -> bool` — event classification.
  - render-equality check for byte-identical-output suppression.
- **Size source keys off mode**: one-shot uses env (`COLUMNS`/`LINES`, reserves viddy chrome rows); watch uses `terminal_size` directly and reserves no chrome. Existing one-shot/viddy behavior and its tests stay unchanged.
- **Module shape**: new `src/gsw/src/watch.rs` holds the event loop, channel wiring, and the RAII terminal-lifecycle guard; it stays thin and delegates every decision to the pure functions above. `render.rs`, `repo.rs`, `snapshot.rs`, `age.rs`, `git.rs`, `bar.rs` are reused as-is.
- **Process discipline**: every phase follows the repo's mandatory TDD workflow (red→commit→green→commit; red may use `--no-verify`). Tests are parallel-safe — temp repos keyed on `pid` + nanos, never hardcoded shared paths. Terminal-control byte sequences are not asserted; the lifecycle guard is verified by construction (RAII).

---

## Phase 1: Watch skeleton — mode selection + terminal lifecycle

**User stories / goals**: drop-in default watch mode that takes over the pane; scriptable fallback preserved; clean terminal restore on every exit path.

### What to build

A complete-but-minimal watch loop. Parsing `--one-shot` selects the existing path verbatim. With no args, `gsw` decides at startup: if stdout is not a TTY, or there is no working-tree repo, it renders once exactly like `--one-shot` and exits. Otherwise it enters watch mode: enter alternate screen + hide cursor (behind an RAII guard that restores the main screen and cursor on drop and via a panic hook), render once using `terminal_size` (no viddy chrome reserved), then block on the event channel. In this phase the only channel producers are the `crossterm` event reader thread (emitting `Quit` on `q`/`Ctrl-C`, `Resize` on terminal resize); a `Resize` re-renders at the new size, a `Quit` exits cleanly through the guard. No filesystem watching and no timer yet.

### Acceptance criteria

- [ ] `gsw --one-shot` produces byte-identical output to current `gsw` (existing one-shot/viddy integration tests stay green).
- [ ] `gsw` with stdout not a TTY renders once and exits (no alternate screen, no loop) — covered by an integration smoke test using a non-TTY stdout.
- [ ] `gsw` outside any working-tree repo prints the existing one-shot output and exits without entering the loop.
- [ ] In a TTY repo, `gsw` enters the alternate screen, renders once via `terminal_size`, and `q` / `Ctrl-C` restores the terminal cleanly.
- [ ] A panic inside the loop still restores the main screen + cursor (guard + panic hook).
- [ ] Unit test: the size-source split selects env dimensions in one-shot mode and `terminal_size`-derived dimensions in watch mode (dimensions injected; chosen source asserted).
- [ ] `crossterm` added as a dependency; `--version` unchanged.

---

## Phase 2: Filesystem-driven re-render

**User stories / goals**: re-render only when something that affects the output actually changes; ignore build/dependency churn so watch mode is strictly cheaper than the old 2 s poll.

### What to build

Add a `notify` recursive watcher rooted at the worktree root, feeding `FsChanged(path)` into the channel. Each path is classified by `should_react`: worktree paths matched by an `ignore`-crate matcher (built from `.gitignore`, nested ignores, `.git/info/exclude`, global excludes) are dropped; `.git/` paths are accepted (watched wholesale, no curated allowlist); other worktree paths are accepted. The main loop coalesces a burst with a ~150 ms quiet window (`recv_timeout`), then recomputes the render. Before repainting, compare the new render string to the previously displayed one and skip the repaint when identical (byte-identical-output suppression), so `.git/` object/log/pack churn costs at most one status walk and never a flicker.

### Acceptance criteria

- [ ] Editing a tracked file triggers exactly one repaint (after debounce coalescing).
- [ ] Writing into an ignored path (`target/`, `node_modules/`) triggers no repaint.
- [ ] A `git commit` (a burst of `.git/` writes) produces a single repaint.
- [ ] FS churn that does not change visible state produces a status walk but no repaint (suppression).
- [ ] Unit test: `should_react` — ignored worktree path dropped, `target/` dropped, tracked/untracked non-ignored path accepted, `.git/HEAD` accepted, `.git/objects/...` accepted (classification accepts; suppression handled separately).
- [ ] Unit test: byte-identical-output suppression returns "no repaint" for an unchanged snapshot.
- [ ] `notify` and `ignore` added as dependencies.

---

## Phase 3: Adaptive decay timer

**User stories / goals**: age-based color decay and age text stay fresh over time without any filesystem events, while idle aged repos cost ~zero.

### What to build

Add a timer thread that emits `Tick` on an adaptive cadence derived from the freshest displayed item (newest commit or working-tree change), recomputed after every render via `next_tick`: `< 1 min` → 1 s; `1 min – 2 h` → ~60 s; `≥ 2 h` → disabled (no ticks). A `Tick` re-renders (subject to the same suppression as Phase 2, so a tick that changes nothing visible is a no-op repaint). The cadence is a pure function of the freshest age, matching the `age.rs` fade model (linear ramp 0→2 h, then frozen at the floor).

### Acceptance criteria

- [ ] After a fresh commit, the age text counts up second-by-second (1 s ticks) for the first minute, then drops to ~60 s cadence.
- [ ] The fade shading visibly advances over time on a `< 2 h` commit with no file activity.
- [ ] An idle repo whose freshest item is `≥ 2 h` old produces no ticks (timer disabled).
- [ ] Unit test: `next_tick` boundaries — just under/over 1 min (1 s vs ~60 s) and just under/over 2 h (~60 s vs `None`).
- [ ] A tick whose recomputed render is identical to the displayed one produces no repaint.

---

## Phase 4: Rollout — drop the viddy wrapper for the gsw pane

**User stories / goals**: eliminate the `viddy` polling wrapper for the gsw pane in the live worktree layout.

### What to build

Update the zellij layout in `~/.zshrc##template.default` (the yadm template, never the rendered `~/.zshrc`): change the gsw pane from `command="viddy" args="gsw"` to `command="gsw"`, then re-render via `yadm alt`. `viddy` stays installed for the `sccache --show-stats` pane.

### Acceptance criteria

- [ ] The gsw pane in `~/.zshrc##template.default` invokes `gsw` directly (no `viddy`).
- [ ] The `sccache --show-stats` viddy pane is untouched.
- [ ] After `yadm alt` re-render, a fresh worktree pane runs `gsw` with no `viddy gsw` process present.
- [ ] Edit is made to the template, not the rendered `~/.zshrc`.
