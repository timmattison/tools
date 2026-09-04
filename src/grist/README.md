# grist

Git Ranks Its Squash Trials — work out which order to squash-merge branches in,
before you commit to one. A trial is one ordering, replayed and costed.

If your question is about *one* branch — would rebasing HEAD onto it conflict,
and by how much? — that's [`grind`](../grind/README.md), which ships alongside
this and answers with its exit code. `grime` (does a *merge* conflict?) is still
a planned sibling and does not exist yet. `grist` answers the question neither of
them can: when you have several branches to land and each one makes the next one
harder, **which order costs the least?**

```console
$ grist issue-130 issue-120
Simulating 2 orderings of 2 branches onto HEAD...
  replaying issue-130
  replaying issue-120
  replaying issue-120
  replaying issue-130

┌───┬───────────────────────┬───────┬───────┬───────┐
│   ┆ Order                 ┆ Hunks ┆ Stops ┆ Files │
╞═══╪═══════════════════════╪═══════╪═══════╪═══════╡
│ ✓ ┆ issue-130 → issue-120 ┆ 3     ┆ 1     ┆ 3     │
├╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┤
│   ┆ issue-120 → issue-130 ┆ 9     ┆ 3     ┆ 3     │
└───┴───────────────────────┴───────┴───────┴───────┘

Land them in this order: issue-130 issue-120
```

## Why order matters

A squash merge collapses a branch into one commit with no ancestry link to the
original. When you rebase the *next* branch afterwards, git has no patch
identity left to recognise the work that already landed — so it replays it and
collides. Whichever branch goes second pays that bill, and the bill is not
symmetric. A branch that rewrote the same region across five commits will stop
you five times if it goes second, and once if it goes first.

`grist` measures that instead of making you guess.

## Usage

```console
grist [--onto <REF>] [-q] <BRANCH>...
```

| Flag | Meaning |
| --- | --- |
| `--onto <REF>` | What the branches land on. Defaults to `HEAD`, so run it from the branch you're merging into. |
| `-q`, `--quiet` | Print only the winning order, space separated, for piping. |

```console
# feed the answer straight into the next step
$ for branch in $(grist -q issue-130 issue-120); do echo "next: $branch"; done
```

Up to six branches (720 orderings) — past that, waiting for the answer costs
more than picking one and finding out.

Run it somewhere that is not a git repository and it says so, by name, before it
starts anything. That refusal is `gitscratch`'s pre-flight, which `grist` now
reaches through the only door there is to a scratch worktree — so a wrong
directory reads as the bad argument it is, rather than arriving as git's own
`not a git repository … .git` from inside `worktree add`, after a run has already
been announced.

A branch name that starts with a dash is a branch name too, and it used to be
read as an option. `git checkout -q --detach --progress` is a complete and valid
command: git reads `--progress` as its own option, finds no branch left to check
out, and detaches HEAD where it already stands. So the scratch worktree stayed on
the base, the rebase found nothing to replay, and the ordering scored zero — the
same zero a genuinely free ordering scores. Every revision `grist` hands to git
now arrives after `--end-of-options`, so git refuses the name rather than obeying
it.

## The three numbers

| Column | What it counts |
| --- | --- |
| **Hunks** | Individual conflict regions you'd hand-merge. The primary ranking key, because it's the closest proxy for actual work. |
| **Stops** | Times a rebase halts and waits for you. Fixed overhead per interruption. |
| **Files** | Distinct files touched by any conflict. Blast radius. |

Ties fall back to stops, then files, then the order you typed — so if nothing
distinguishes two plans, the one you already had in mind wins.

## What it does to your repo

Nothing. Every replay happens in a detached scratch worktree in a temp
directory, which is torn down afterwards. Specifically:

- **Branch refs are never moved.** Checkouts are `--detach`, and
  `rebase.updateRefs` is pinned off. That second one is not paranoia: with the
  setting enabled and the guard removed, a dry run *destroys the branch it is
  simulating*. There's a test for it.
- **`rerere` is disabled.** A simulated conflict resolution would otherwise be
  recorded in the shared `rr-cache` and silently pre-resolve your real merges
  later.
- **Nothing of yours runs, and nothing of yours is collected.** Hooks are
  redirected at an empty directory, and `core.fsmonitor` is pinned off beside
  them — the filesystem monitor names a program git executes directly, so the
  hooks redirect alone would leave it running. `gc.auto` and `maintenance.auto`
  are both off, which is two switches rather than one: `gc.auto` holds back the
  gc task, while `maintenance.auto` holds back the rest of automatic
  maintenance, including a prefetch that would otherwise fetch from every remote
  and write refs into your repository.
- **Branches checked out in other worktrees work fine** — the usual case, and
  the one where a plain `git checkout` refuses outright.

## Reading the numbers honestly

When a replay conflicts, `grist` resolves it by staging the conflict markers
verbatim and carrying on, so it can walk the *whole* rebase rather than stopping
at the first collision. That is the conservative choice — unlike `--ours` or
`--theirs` it never silently discards a side — and it mirrors reality in that a
human resolution also leaves later commits conflicting against the resolved
state.

It is still a model. Treat the totals as a **cost index for comparing orderings
measured under identical rules**, not as a prediction of exactly how many
conflict markers you'll see. The ranking is the product; the absolute numbers
are supporting evidence.

A hunk is a closed conflict region — an opening marker, and the closing marker
after it — and both are matched exactly, so a line of file content that merely
begins with brackets is not one. `merge.conflictStyle` is pinned beside that
rule, because `diff3` and `zdiff3` put the base version inside the region: the
file the markers are counted in is then the same file on every machine, and two
developers ranking the same branches read the same order.

What it will not do is guess. If git cannot carry a replay out — it refuses to
write a commit because the object database is full or read-only, say — `grist`
fails and tells you which branch and which commit, rather than counting the
dropped work as zero and handing you a ranking of orderings it never measured.
That matters most when the cause is systematic, since a failure that hits every
branch would otherwise make every ordering tie and turn the answer into a
confident "pick whichever you prefer".

## Development

```console
cargo test -p grist          # unit, simulation, CLI and safety suites
cargo test -p gitscratch     # the shared harness and its own safety suite
cargo build --release -p grist
```

Most of the safety guarantees above belong to the shared harness, so that is
where they are pinned. `gitscratch`'s `tests/safety.rs` holds nine of them —
`rebase.updateRefs=false`, the pinned `rebase.backend=merge` that keeps it
falsifiable on a developer who prefers the apply backend, the detached checkout,
`rerere` recording nothing, no hook firing, the real working tree and index
surviving untouched, both halves of teardown, and a replay neither hanging nor
failing under commit signing — and every one has been watched to fail: break the
guard, confirm that specific test goes red for the stated reason, put it back. A
guard nobody has ever seen fail is indistinguishable from one that is quietly
broken, since both report green forever.
[`MUTATIONS.md`](../gitscratch/MUTATIONS.md) is where that evidence
lives, guard by guard, alongside the failure output captured at the time.

`grist` keeps a `tests/safety.rs` of its own for the part `gitscratch` cannot
see: that a full simulation, composed the way `grist` composes it —
`check_out_detached` → `replay_rebase` → `squash_into`, once per branch of every
ordering — leaves every real branch ref where it found it. The composition is
`grist`'s, and so is the detach it depends on, even though the harness now spells
that checkout. `gitscratch`'s own `tests/safety.rs` writes its checkout out by
hand rather than calling `check_out_detached`, because that detach is one of the
guards under test and a guard read through the code it guards proves nothing —
so a `check_out_detached` that lost `--detach` leaves every `gitscratch` test
green and reddens this one. Watched, not assumed: the mutation reddens
`a_full_simulation_never_moves_real_branch_refs` here and two more `grist`
tests, and nothing in `gitscratch`.

What is left over is small and worth naming: `gc.auto=0`, the
`rebase.autoStash`/`autosquash` pair and `gpg.format` are established by
construction in the harness's `safety_config` rather than pinned by a test of
their own.
