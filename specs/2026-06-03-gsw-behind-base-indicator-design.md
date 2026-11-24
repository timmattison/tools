# gsw: behind-base ("needs rebase") indicator

**Date:** 2026-06-03
**Status:** Approved
**Crate:** `src/gsw`

## Problem

The gsw header reports how far the branch is *ahead* of its base and how the
branch relates to its *upstream tracking ref*:

```
gsw • issue-1129-permissions • 1141 commits ahead of main • ↑0 ↓0 origin/issue-1129-permissions • last commit 6d0h ago
```

It says nothing about whether the base has moved on since the branch forked —
i.e., whether the branch needs a rebase onto `main`. A branch can show
`↑0 ↓0` against its upstream and still be hundreds of commits behind `main`.

## Behavior

When commits exist on the base that are not reachable from HEAD, append a
warning-colored segment to the base field. When the branch is fully up to
date with its base, the header is **byte-identical to today**.

Up to date (unchanged):

```
gsw • issue-1129-permissions • 1141 commits ahead of main • ↑0 ↓0 origin/issue-1129-permissions • last commit 6d0h ago
```

Base has moved on (`, 87 behind` rendered yellow + bold; the rest of the
header stays bold as today):

```
gsw • issue-1129-permissions • 1141 commits ahead of main, 87 behind • ↑0 ↓0 origin/issue-1129-permissions • last commit 6d0h ago
```

## Design

### Data layer (`src/gsw/src/repo.rs`)

Replace `commits_ahead(repo, base) -> u32` with:

```rust
pub struct BaseStatus {
    pub ahead: u32,  // commits on HEAD not on base  (base..HEAD)
    pub behind: u32, // commits on base not on HEAD  (HEAD..base)
}

pub fn base_status(repo: &gix::Repository, base: &str) -> BaseStatus
```

The behind walk is the mirror of the existing ahead walk: walk from
`base_id` with `head_id` hidden. This is the same shape `upstream_status`
already computes against `@{upstream}`, so extract one private helper:

```rust
fn ahead_behind(repo: &gix::Repository, ours: gix::ObjectId, theirs: gix::ObjectId) -> Option<(u32, u32)>
```

and have **both** `base_status` and `upstream_status` call it — three
hand-rolled rev walks collapse into one helper rather than adding a fourth.

Failure semantics are unchanged from `commits_ahead`: any resolution or walk
error → `BaseStatus { ahead: 0, behind: 0 }`, so a missing/unresolvable base
degrades to today's behavior (ahead 0, no behind segment). Counts clamp to
`u32::MAX` like the existing code.

### Snapshot (`src/gsw/src/render.rs`, `src/gsw/src/snapshot.rs`)

Add `commits_behind: u32` to `Snapshot` alongside `commits_ahead`, threaded
through `build_snapshot` and `build_output` (`src/gsw/src/main.rs`).

### Header rendering (`src/gsw/src/render.rs`)

Today the header is one plain string from `header_text` with `.bold()`
applied to the whole line at the assembly point (`render`, ~line 92). The
behind segment needs its own color, so the assembly composes three pieces:

```
prefix.bold() + ", 87 behind".yellow().bold() + suffix.bold()
```

`header_text` (or a successor returning segments) keeps producing the exact
current text when `commits_behind == 0`. `NO_COLOR` / non-TTY handling falls
out of the `colored` crate exactly as today. Header width math is unaffected
(the header is not width-truncated).

### Watch mode

No changes needed. The watcher already covers the git dir and the shared
common dir recursively (`spawn_fs_watcher`), so a ref update to
`refs/heads/main` (commit, pull, fetch in another worktree) triggers a
re-render and the behind count stays live.

## Edge cases

| Case | Result |
| --- | --- |
| `behind == 0` | No segment; header byte-identical to today |
| `ahead == 0, behind > 0` | `0 commits ahead of main, 87 behind` — still signals rebase |
| Base unresolvable | `(0, 0)` — degrades to current behavior |
| `head == base_id` | `(0, 0)` — short-circuit preserved |
| Counts overflow `u32` | Clamp to `u32::MAX` (matches existing) |
| `--base <ref>` flag | Works unchanged; same `base` string feeds `base_status` |

## Known limitation (accepted)

The behind count is relative to the **local** base ref (`main`). If local
`main` is itself stale (not pulled), the count reflects what's on disk — the
same limitation the existing ahead count has. Changing base resolution (e.g.,
preferring `origin/main`) is explicitly out of scope.

## Testing (TDD, red → green per commit)

- **`repo.rs` unit tests:** `base_status` reports behind when base advances
  past the fork point; `(ahead, 0)` when base hasn't moved; `(0, 0)` when
  HEAD == base; existing `commits_ahead` cases ported to `base_status.ahead`.
- **`render.rs` header tests:** header contains `, N behind` when
  `commits_behind > 0`; header does **not** contain `behind` when it's 0;
  existing header tests keep passing unmodified.
- **Integration test (`tests/integration.rs`):** mirror of the existing
  upstream `↑1`/`↓1` test — branch from main, advance main, assert the
  header shows the behind count.
- Test repos use unique temp dirs (existing `init_repo` pattern) — parallel-safe.
