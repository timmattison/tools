# Plan: gsw behind-base ("needs rebase") indicator

> Source spec: `specs/2026-06-03-gsw-behind-base-indicator-design.md`

## Architectural decisions

Durable decisions that apply across all phases (settled in the spec):

- **Header format**: `… {n} commits ahead of {base}, {m} behind • …` — the
  `, {m} behind` segment renders yellow + bold and appears **only when
  `behind > 0`**. When `behind == 0` the header is byte-identical to today.
- **Data model**: `BaseStatus { ahead: u32, behind: u32 }` returned by
  `base_status(repo, base)` in `src/gsw/src/repo.rs`, replacing
  `commits_ahead`. `Snapshot` gains `commits_behind: u32` alongside
  `commits_ahead`.
- **Failure semantics**: any resolution/walk error → `(0, 0)`; counts clamp
  to `u32::MAX`. Matches existing `commits_ahead` behavior.
- **Base resolution unchanged**: counts are against the local base ref
  (`resolve_base` / `--base`); preferring `origin/main` is out of scope.
- **Watch mode unchanged**: the existing git-dir watcher already triggers
  re-renders when the base ref moves.
- **TDD**: red → commit → green → commit per behavior, per global testing
  rules. Test repos use unique temp dirs (existing `init_repo` pattern).

---

## Phase 1: Behind count end-to-end in the header

**Spec sections**: Problem, Behavior, Data layer, Snapshot, Header rendering, Edge cases, Testing

### What to build

The full vertical slice: gsw counts commits on the base that are not
reachable from HEAD and, when that count is nonzero, appends a yellow+bold
`, {m} behind` segment to the base field of the header. The count flows
data layer → snapshot → header rendering. When the base hasn't moved on,
output is byte-identical to current gsw.

Demo: run `gsw` on a branch whose `main` has advanced past the fork point —
the header shows the behind count, and in watch mode it updates live when
`main` advances again.

### Acceptance criteria

- [ ] `base_status` returns `(ahead, behind)` for a branch whose base has
      advanced past the fork point; `(ahead, 0)` when the base hasn't moved;
      `(0, 0)` when HEAD == base or the base is unresolvable
- [ ] Existing `commits_ahead` unit-test cases are ported to
      `base_status().ahead` and still pass
- [ ] Header contains `, {m} behind` (yellow + bold) when
      `commits_behind > 0`
- [ ] Header is byte-identical to current output when `commits_behind == 0`
      (existing header tests pass unmodified)
- [ ] Integration test mirroring the upstream `↑1`/`↓1` test: branch from
      main, advance main, header shows the behind count
- [ ] `cargo test`, `cargo clippy`, `cargo fmt --check` clean

---

## Phase 2: Consolidate rev walks into one `ahead_behind` helper

**Spec sections**: Data layer ("three hand-rolled walks collapse into one helper")

### What to build

Internal refactor under green: extract a private
`ahead_behind(repo, ours, theirs)` helper in `repo.rs` and have both
`base_status` and `upstream_status` call it, removing the duplicated
hand-rolled rev walks. No behavior change.

### Acceptance criteria

- [ ] `base_status` and `upstream_status` both delegate to the shared helper;
      no duplicated walk code remains in either
- [ ] All existing unit, header, and integration tests pass unmodified
- [ ] `cargo test`, `cargo clippy`, `cargo fmt --check` clean
