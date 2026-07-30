# grime and grind as Rust binaries on a shared `gitscratch` harness

Date: 2026-07-30
Status: Approved, ready for planning

## Problem

`grime` and `grind` are zsh functions in `~/.zshrc##template.default` (lines
506-563). Each dry-runs a git operation — `grind` a rebase, `grime` a merge — in
a throwaway detached worktree and reports whether it would conflict.

`grist`, in this repo, does the same thing but hardened: every git invocation
goes through `Git::safety_config()` (`src/grist/src/git.rs:107`), which pins the
configuration that keeps a simulation from touching anything real. Those
guarantees are pinned by `src/grist/tests/safety.rs` and were each validated by
mutation.

The shell functions have none of that. There are now two implementations of
"safely dry-run git in a scratch worktree" — one tested, one not — and every
guard added to `grist` from here on is a guard `grime`/`grind` silently do not
get.

### The concrete defects

| Guard | `grist` | `grime`/`grind` |
| --- | --- | --- |
| `rebase.updateRefs=false` | pinned, regression-tested | **absent** |
| `rerere.enabled=false` | pinned | absent |
| hooks disabled (`core.hooksPath`) | pinned | absent |
| `GIT_EDITOR=true` | pinned | absent |
| `commit.gpgsign=false` | pinned | absent |
| `gc.auto=0` | pinned | absent |
| leaked-worktree cleanup | `Drop` removes the scratch worktree by path, and deliberately *never* prunes | trap misses `SIGHUP`, so the worktree entry leaks |

The first row is the serious one. `grind` runs a bare `git -C "$d" rebase
"$branch"`. The range it replays is the current branch's commits, so with
`rebase.updateRefs=true` git rewrites every real ref pointing into that range —
including the branch being "simulated". `tests/safety.rs:17` exists because that
exact scenario was reproduced.

Severity, honestly stated: `rebase.updateRefs` is currently unset and
`rerere.enabled` is `false` in the target environment, so rows 1 and 2 are
latent rather than live. They are one `git config --global` away from live, and
a dry-run tool should be safe unconditionally.

### A live defect, not latent

Both functions conflate "conflicts" with "git failed". `git rebase
nonexistent-branch` exits 128, and the function prints `grind: conflicts …
(rc=128)`. A typo'd branch name reports conflicts that do not exist.

### The doctrine violation

`CLAUDE.md` permits a shell function only when it is **load-bearing** — when it
must mutate the parent shell (`cd`, exported vars). Both functions run entirely
inside a `( … )` subshell and return an exit code. They mutate nothing. By the
repo's own rule they should be binaries.

## Goals

- One crate owns the safety configuration; no tool can build a scratch worktree
  without going through it.
- `grime` and `grind` become Rust binaries with the hardening `grist` already
  has, and report *how much* conflict rather than just whether.
- The unhardened shell path is removed, not merely shadowed.

## Non-goals

- Changing `grist`'s behavior, output, or CLI.
- Giving `grime`/`grind` a `--onto`-style flexible source ref. They keep today's
  single-positional signature.
- Shell integration. Neither tool gets `--shell-setup`.

## Architecture

### New crate: `src/gitscratch/`

A lib crate, matching the repo's existing shared-library convention
(`shellsetup`, `termbar`, `buildinfo`, `repowalker`, `filewalker`,
`portplz-core`). Picked up automatically by `members = ["src/*"]`.

Moved in from `grist`:

| Item | From | Change |
| --- | --- | --- |
| `Git` | `grist::git` | verbatim; this is `safety_config()`, the thing being centralized |
| `Scratch` | private in `grist::simulate` | promoted to `pub` |
| `count_conflict_hunks` | private in `grist::simulate` | becomes a `Scratch` internal |
| `BranchName`, `Hunks`, `Stops`, `Files` | `grist::metrics` | verbatim |
| `Cost` (private) | `grist::simulate` | becomes `pub struct Conflicts` |

`Conflicts` is the shared result type — `{ hunks, stops, files: BTreeSet<String> }`
with accessors and the existing `absorb` fold. `grist`'s private `Cost` becomes
this type; `grime`/`grind` return it directly.

`grist` retains `plan`, `rank`, `simulate`, and `OrderingScore`, and re-exports
the moved newtypes so its public API is unchanged. Ranking stays in `grist`
because `grime`/`grind` evaluate one candidate — there is nothing to rank.

### The two replay operations

`Simulator::replay_onto` (`src/grist/src/simulate.rs:193`) takes `&self` but
calls no method on it, so it moves to `Scratch` without behavior change. Its
signature does change: the `git` and `worktree` parameters become `self`, and
the `branch` parameter — used only to word error messages — is dropped, with the
caller attaching that context instead.

```rust
impl Scratch {
    pub fn create(repo: &Path, at: &str) -> Result<Self>;

    /// Rebase the checked-out HEAD onto `onto`, walking the whole rebase and
    /// auto-resolving conflicts by staging markers verbatim.
    pub fn replay_rebase(&self, onto: &str) -> Result<Conflicts>;

    /// `merge --no-commit --no-ff <branch>`, measuring what conflicted.
    pub fn replay_merge(&self, branch: &str) -> Result<Conflicts>;
}
```

All three tools then reduce to the same shape with a different verb:

- `grind <b>` — `Scratch::create(repo, "HEAD")` → `replay_rebase(b)` → report
- `grime <b>` — `Scratch::create(repo, "HEAD")` → `replay_merge(b)` → report
- `grist` — `Scratch::create(repo, base)` → per branch: `checkout --detach`,
  `replay_rebase`, `squash_into` → rank

`replay_merge` is the only genuinely new logic, roughly 25 lines:

1. `git merge --no-commit --no-ff <branch>`.
2. Exit 0 → `Conflicts::default()`.
3. Non-zero **with** unmerged paths → count hunks per file via
   `count_conflict_hunks`, collect file names, `stops = 1`.
4. Non-zero **without** unmerged paths → error, carrying git's stderr.

Step 4 matters: "refusing to merge unrelated histories" must be reported as
neither clean nor conflicting. `replay_rebase` already has the equivalent guard
(`simulate.rs:205`).

### Reporting

Reporting lives in `gitscratch`, not in each binary. `grime` and `grind` must
print the same shape, and two renderers drifting apart is the bug class this
work exists to eliminate. This is a deliberate acceptance of a small amount of
presentation logic in a library crate.

`grime` omits the stops count: a merge halts exactly once, so the number carries
no information. `Conflicts` still records it; the merge renderer does not print
it.

## CLI

```console
grind [-q] <BRANCH>     # would rebasing HEAD onto BRANCH conflict?
grime [-q] <BRANCH>     # would merging BRANCH into HEAD conflict?
```

clap derive, `#[clap(author, version = version_string!(), about)]` per the
repo's buildinfo requirement.

`-q` / `--quiet` suppresses all output, stdout and the stderr dirty-tree note
alike, leaving only the exit code. Unlike `grist` there is no answer to pipe;
the answer *is* the exit code, so a scripted caller wants silence, not a
different rendering.

### Output

```console
$ grind main
grind: conflicts - replaying HEAD onto main
       4 hunks across 2 files, 3 stops

  src/lib.rs     3 hunks
  src/main.rs    1 hunk

$ grind origin/main
grind: clean - replaying HEAD onto origin/main hit no conflicts

$ grime feature
grime: conflicts - merging feature into HEAD
       4 hunks across 2 files

  src/lib.rs     3 hunks
  src/main.rs    1 hunk
```

When the working tree is dirty, a note goes to stderr and the exit code is
unaffected:

```console
grind: note: 3 uncommitted files are not included; simulating from HEAD
```

Simulating from `HEAD` is the existing behavior and the only thing that *can* be
simulated. The note exists so a `clean` verdict is never misread as covering
uncommitted work.

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | clean |
| 1 | conflicts |
| 2 | error — bad ref, not a repo, unrelated histories, git failed |

`fn main() -> Result<()>` maps every error to exit 1, which would re-collide
with "conflicts". `main` therefore returns `ExitCode` explicitly.

## Error handling

- The branch ref is resolved with `Git::rev_parse` **before** a scratch is
  created, so a typo fails in milliseconds with a clear message and no worktree
  churn.
- Not inside a git repository → exit 2.
- Merge or rebase exits non-zero with no unmerged paths → exit 2, carrying git's
  stderr.
- Dirty working tree → stderr note, execution continues.
- `MAX_RESOLUTION_ROUNDS` (`simulate.rs:37`) is inherited, so an unanticipated
  git state stalls the run instead of spinning.

## Testing

TDD red → green per `CLAUDE.md` and `TESTING.md`, with one stated exception: the
extraction commit is a pure move with no behavior change, covered by `grist`'s
existing suite staying green. There is no meaningful failing behavioral test for
relocating a file, so that commit is a refactor under existing coverage. Every
subsequent commit is red → green.

### Fixture sharing

`src/grist/tests/support/mod.rs` compiles per test binary inside `grist`. Three
crates now need it. It moves to `gitscratch` as `pub mod testing` behind a
`testing` feature; `grist`, `grime`, and `grind` dev-depend on
`gitscratch = { path = "../gitscratch", features = ["testing"] }`.

### Safety suite — `src/gitscratch/tests/safety.rs`

Three guarantees migrate from `grist`; five are new. All now cover the shared
harness rather than `Simulator` alone.

1. `rebase.updateRefs=true` moves no real branch ref *(migrated)*
2. branches checked out in other worktrees still work *(migrated)*
3. an unrelated worktree whose directory is *temporarily* missing keeps its
   administrative state *(migrated)* — this is what pins teardown to removing
   the scratch worktree by path and never running the repo-wide, no-grace-period
   `worktree prune`
4. `rerere.enabled=true` leaves `rr-cache` unwritten
5. hooks do not fire — plant `post-checkout`, `pre-rebase`, `post-rewrite`, and
   `pre-merge-commit` hooks that each touch a sentinel; assert none appear
6. the real working tree and index are untouched — a dirty file survives
   byte-identical
7. no worktree is left registered, including after a *conflicting* run
8. `commit.gpgsign=true` neither hangs nor fails a replay

### Behavior suites

`grind`: clean rebase reports zero conflicts; a conflicting rebase counts hunks
and files; a multi-commit branch produces `stops > 1`, the asymmetry `grist`'s
README describes; a bad ref is an error, not conflicts.

`grime`: clean merge; conflicting merge counts hunks and files; unrelated
histories exits 2; and a **fast-forwardable merge still exercises a real
three-way merge** — a regression test that fails the day someone drops
`--no-ff`.

### CLI suites

Exit codes 0, 1, and 2 each asserted individually. `-q` prints nothing on all
three paths. `--version` matches the `buildinfo` format required by `CLAUDE.md`.

### UTF-8

Per the repo's UTF-8 rule: a conflicting file named `日本語.txt` and a branch
name containing multi-byte characters. Conflict output must neither panic nor
mis-truncate.

### Parallel safety

Every fixture goes through `TempDir`. The new hook-sentinel and `rr-cache` tests
must key their paths the same way, per `CLAUDE.md`'s shared-resource rule — no
fixed paths under `/tmp`, the repo, or the home directory.

### Mutation verification

For each of the eight safety tests, identify the guard it pins — the pinned
config setting for 1, 4, 5 and 8; `--detach` for 2; running in a scratch
worktree at all for 6; the `Drop` teardown's `worktree remove --force` for 7 —
then remove that guard, confirm the specific test goes red, and restore it.

Test 3 mutates in the opposite direction, because the guard it pins is an
*absence*: **add** a `worktree prune` to the teardown and confirm the test goes
red.

This is how `grist`'s existing guards were validated and is required by
`CLAUDE.md`'s enforced-helper rule — it is the only thing that stops a
quietly-broken guard from passing green forever.

## Documentation and migration

- `src/grime/README.md` and `src/grind/README.md`.
- `src/grist/README.md`: the "planned siblings — neither exists yet" paragraph
  becomes a description of shipped tools.
- Root `README.md`: `gitscratch` under `## Shared Libraries`, `grime` and
  `grind` under `## The tools`.
- `TLDR.md`: both new tools, alphabetized. Nothing enforces parity between the
  two indexes, so both must be updated by hand.
- Delete `grime` and `grind` from `~/.zshrc##template.default` (never
  `~/.zshrc`, which is rendered from it), then run `yadm alt` to re-render.
  Until they are deleted, the shell functions shadow the binaries on `PATH` and
  the unhardened path stays live.
- No `--shell-setup` for either tool.

## Risks

**The shell functions shadow the binaries.** Installing the binaries alone
changes nothing about day-to-day behavior; the template edit is what completes
the migration. It is a required step, not a follow-up.

**Walking the whole operation costs more time than bailing at the first
conflict.** Bounded: one replay per invocation, not `grist`'s factorial fan-out.
A badly-conflicting rebase of a long branch is the worst case.

**Moving code between crates can silently drop a guard.** Mitigated by the
safety suite migrating with the code and by the mutation pass rerun after the
move.
