# grist

Work out which order to squash-merge branches in, before you commit to one.

`grime` tells you whether a merge conflicts. `grind` tells you whether a rebase
conflicts. `grist` answers the question they can't: when you have several
branches to land and each one makes the next one harder, **which order costs the
least?**

```console
$ grist issue-130 issue-120
Simulating 2 orderings of 2 branches onto HEAD...
  replaying issue-130
  replaying issue-120
  replaying issue-120
  replaying issue-130

┌───┬─────────────────────────┬───────┬───────┬───────┐
│   ┆ Order                   ┆ Hunks ┆ Stops ┆ Files │
╞═══╪═════════════════════════╪═══════╪═══════╪═══════╡
│ ✓ ┆ issue-130 → issue-120   ┆ 2     ┆ 1     ┆ 1     │
│   ┆ issue-120 → issue-130   ┆ 9     ┆ 4     ┆ 3     │
└───┴─────────────────────────┴───────┴───────┴───────┘

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
- **Hooks are disabled** and `gc.auto` is off, so nothing fires and nothing
  collects the simulated commits mid-run.
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

## Development

```console
cargo test -p grist          # unit, simulation, safety and CLI suites
cargo build --release -p grist
```

The safety guarantees above are pinned by `tests/safety.rs`, and each one was
verified by mutation: remove the guard, watch the test fail, put it back.
