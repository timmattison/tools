# gsw: rebase / merge in-progress indicators

**Date:** 2026-07-01
**Status:** Approved
**Crate:** `src/gsw`

## Problem

The gsw header describes how the branch relates to its base and upstream, but
says nothing about an *in-progress git operation*. When the working tree is
mid-rebase or mid-merge — often the exact moment you reach for a status view —
gsw looks identical to a clean tree except for the conflicted file rows. There
is no at-a-glance signal that you are rebasing or merging, how many conflicts
remain, or (for a rebase) how far through the sequence you are.

## Behavior

When a rebase or merge is in progress, gsw inserts one **dedicated line
directly below the header and above the separator**. When no such operation is
in progress, the output is **byte-identical to today** (no line, separator stays
on the second row).

Rebase and merge are surfaced **separately** — they are mutually exclusive git
states, so at most one indicator ever shows.

Clean tree (unchanged):

```
gsw • feat • 4 commits ahead of main • last commit 2m ago
────────────────────────────────────────────────────────
 M src/foo.rs
```

Rebase in progress (indicator line yellow + bold; conflict count red + bold):

```
gsw • feat • 4 commits ahead of main • last commit 2m ago
⚠ rebase 3/10 · 2 conflicts to resolve
────────────────────────────────────────────────────────
 M src/foo.rs
```

Merge in progress:

```
gsw • main • 1 commit ahead of main • last commit 2m ago
⚠ merge · 2 conflicts to resolve
────────────────────────────────────────────────────────
 M src/foo.rs
```

### Indicator text

- **Merge:** `⚠ merge` followed by ` · {n} conflict[s] to resolve` **only when
  `n > 0`**. A merge stopped with no conflicts (e.g. `--no-commit` on a clean
  merge) shows just `⚠ merge`.
- **Rebase:** `⚠ rebase` followed by ` {current}/{total}` **when the step
  counts are readable**, then ` · {n} conflict[s] to resolve` **only when
  `n > 0`**. Steps are git's own `current/total` form (step 3 of 10 →
  `3/10`), matching `git status` and the shell prompt.
- Pluralization: `1 conflict` vs `2 conflicts`.
- Graceful degradation ("if possible"): if the step files are missing or
  unparseable the count is simply omitted (`⚠ rebase · 1 conflict to resolve`,
  or `⚠ rebase` if there are also no conflicts).

## Design

### Data layer — operation detection (`src/gsw/src/repo.rs`)

Add a detector that returns the render-layer `Operation` (mirroring how
`upstream_status` already returns the render-layer `UpstreamStatus`):

```rust
pub fn operation_state(repo: &gix::Repository, conflicts: u32) -> Option<Operation>
```

Classification uses gix's native `repo.state() -> Option<gix::state::InProgress>`,
which is modeled on git's own `wt-status.c` / `git-prompt.sh` logic (it inspects
`MERGE_HEAD`, `rebase-merge/`, `rebase-apply/` under the repo's git dir, so it is
worktree-aware and takes no locks — consistent with gsw's read-only, gix-only
philosophy):

| `repo.state()`                                       | `operation_state` result                 |
| ---------------------------------------------------- | ---------------------------------------- |
| `Merge`                                              | `Operation::Merge { conflicts }`         |
| `Rebase` / `RebaseInteractive` / `ApplyMailboxRebase`| `Operation::Rebase { step, conflicts }`  |
| `ApplyMailbox` (plain `git am`), `CherryPick*`, `Revert*`, `Bisect`, `None` | `None` (out of scope) |

**Step counts** are not exposed by gix, so read them directly from the git dir
(`repo.path()`, the same base gix's `state()` uses), exactly as git's prompt
does:

- `rebase-merge/msgnum` + `rebase-merge/end` (merge-backend / interactive rebase), else
- `rebase-apply/next` + `rebase-apply/last` (apply-backend rebase).

Parse each as `u32`; if either file is missing or unparseable, `step` is `None`.
Extract a small private helper:

```rust
fn rebase_step(git_dir: &Path) -> Option<StepProgress>
```

### Conflict count (`src/gsw/src/main.rs`)

No new git work. `collect_changes` already surfaces every unmerged path as
`FileStatus::Conflicted`. In `collect_snapshot`, after `build_snapshot`, count
those rows and feed the total into `operation_state`:

```rust
let conflicts = snapshot.files.iter()
    .filter(|f| f.status == FileStatus::Conflicted)
    .count() as u32;
snapshot.operation = repo::operation_state(repo, conflicts);
```

### Snapshot / data model (`src/gsw/src/render.rs`)

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Merge  { conflicts: u32 },
    Rebase { step: Option<StepProgress>, conflicts: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepProgress { pub current: u32, pub total: u32 }
```

Add `pub operation: Option<Operation>` to `Snapshot`. Every existing `Snapshot`
literal (in `snapshot.rs`, `render.rs` tests, `watch.rs` tests, `main.rs` tests)
gains `operation: None`, so current behavior is preserved by default.

### Rendering (`src/gsw/src/render.rs`)

In `render_with_offset`, immediately after pushing the header line and **before**
`render_separator`, push an operation line when `snapshot.operation.is_some()`:

```rust
lines.push(header_line);
if let Some(op) = &snapshot.operation {
    lines.push(render_operation_line(op));
}
lines.push(render_separator(opts.terminal_width));
```

`render_operation_line` composes the styled pieces: the `⚠ rebase 3/10` /
`⚠ merge` label as `.yellow().bold()` (the same warning convention as the
existing "behind" segment), and the ` · {n} conflict[s] to resolve` clause as
`.red().bold()` to flag the pending action. Like the header, the line is free
text and is **not** width-truncated. `NO_COLOR` / non-TTY handling falls out of
the `colored` crate exactly as today (the global override is already set in
`main`).

### Layout budget (`src/gsw/src/main.rs`, `render_frame`)

The frame reserves rows for chrome before splitting the remainder between the
file list and the log. The operation line is one more row of chrome **only when
present**:

```rust
let header_chrome: usize = 2 + usize::from(snapshot.operation.is_some());
```

This keeps the file list from being over-truncated when the indicator shows.

### Watch mode

No changes needed. `spawn_fs_watcher` watches the worktree git dir (and shared
common dir) recursively, and `should_react` accepts anything under the git dir
wholesale. Creating/removing `MERGE_HEAD` or `rebase-merge/`, and updates to
`rebase-merge/msgnum` as the rebase advances, all trigger a re-collect, so the
indicator appears, updates its step/conflict counts, and disappears live.

## Edge cases

| Case | Result |
| --- | --- |
| No operation in progress | No line; output byte-identical to today |
| Merge, 0 conflicts | `⚠ merge` (no conflict clause) |
| Merge, N conflicts | `⚠ merge · N conflict[s] to resolve` |
| Rebase, steps readable, N conflicts | `⚠ rebase C/T · N conflict[s] to resolve` |
| Rebase, steps readable, 0 conflicts | `⚠ rebase C/T` (e.g. stopped for `edit`/`reword`) |
| Rebase, step files missing/unparseable | `⚠ rebase[ · N conflict[s] to resolve]` (no `C/T`) |
| Interactive rebase | Treated as rebase (`RebaseInteractive`) |
| Apply-backend rebase (`git rebase --apply`) | Treated as rebase (`ApplyMailboxRebase`) |
| Plain `git am` (not a rebase) | `None` — no indicator (out of scope) |
| Cherry-pick / revert / bisect | `None` — out of scope for this feature |
| `1` conflict | Singular: `1 conflict to resolve` |

## Out of scope (accepted)

- **Cherry-pick, revert, bisect, and plain `git am`** get no indicator. Only
  rebase and merge were requested; the enum leaves room to add them later.
- **Rebase todo detail** (which command — pick/edit/squash — is stopped on) is
  not shown; only `current/total` and conflict count.

## Testing (TDD, red → green per commit)

- **`render.rs` unit tests:** operation line renders the expected visible text
  for each shape — `merge` with/without conflicts; `rebase` with steps + with
  step files absent; with/without conflicts; `1 conflict` vs `2 conflicts`
  pluralization. Absent-when-`None`: the separator stays on the second output
  line and the frame is unchanged (guards the byte-identical claim). Use the
  existing `strip_ansi` helper to assert on visible glyphs.
- **`repo.rs` unit tests:** using the existing tempdir git fixture pattern
  (`init_repo`, isolated config, parallel-safe unique dirs): a real merge
  conflict → `Operation::Merge { conflicts: 1 }`; a real rebase conflict →
  `Operation::Rebase { step: Some(_), conflicts: 1 }` with the expected
  `current/total`; a clean repo → `None`. `rebase_step` returns the parsed
  `current/total` for a `rebase-merge` dir and `None` when the files are absent.
- **`render_frame` test (`main.rs`):** when `operation.is_some()`, the chrome
  budget reserves one extra row so the file list is not over-truncated versus
  the same snapshot with `operation: None`.
- **Integration test (`tests/integration.rs`):** drive a real rebase (or merge)
  conflict in a temp repo and assert the rendered frame contains the indicator
  line with the conflict count. Parallel-safe unique temp dirs (existing
  pattern).
