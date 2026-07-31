# grind

Git Rebase In aNother Dimension — would rebasing HEAD onto that branch conflict,
and by how much?

`grind` answers by actually doing it: it replays your commits onto the branch in
a detached scratch worktree in a temp directory, counts what collided, and tears
the worktree down before you see the answer. Nothing in your repository moves.

```console
$ grind main
grind: conflicts - replaying HEAD onto main
       4 hunks across 2 files, 3 stops

  src/lib.rs     3 hunks
  src/main.rs    1 hunk

$ grind origin/main
grind: clean - replaying HEAD onto origin/main hit no conflicts
```

`grime` (does a *merge* conflict?) is a planned sibling and does not exist yet.
[`grist`](../grist/README.md) is the one for the other question — given several
branches to land, which *order* costs the least?

## The answer is the exit code

| Code | Meaning |
| --- | --- |
| `0` | Clean. The rebase hit no conflicts. |
| `1` | Conflicts, and the report says how many and where. |
| `2` | Error. The question could not be answered at all. |

Three codes rather than two, because "the rebase would conflict" and "I could not
tell you" are different answers and a script has to be able to act on the
difference.

`grind` replaces a zsh function of the same name that could not. It ran a bare
`git rebase "$branch"` and read any non-zero exit as conflicts — so
`grind nonexistetn-branch` exited 128 and got reported as
`grind: conflicts … (rc=128)`: conflicts that did not exist, on a branch that
did not either. Every other way git can fail — not a repository, unrelated
histories, a repo in a state the replay cannot enter — landed in the same
bucket.

The branch ref is now resolved **before** a scratch worktree is created, so a
typo fails in milliseconds and can never be mistaken for a conflict:

```console
$ grind mian
grind: error: could not resolve 'mian' to a commit: git rev-parse mian^{commit} failed:
mian^{commit}
fatal: ambiguous argument 'mian^{commit}': unknown revision or path not in the working tree.
Use '--' to separate paths from revisions, like this:
'git <command> [<revision>...] -- [<file>...]'
$ echo $?
2
```

Errors carry git's own explanation through to you, because that is usually the
only part of the message that says what actually went wrong.

## Usage

```console
grind [-q] <BRANCH>
```

| Argument | Meaning |
| --- | --- |
| `<BRANCH>` | What to rebase HEAD onto. Anything git resolves to a commit works — a branch, a remote-tracking ref, a tag, a raw sha. |
| `-q`, `--quiet` | Print nothing whatsoever. The exit code is still the answer. |

One positional argument and no `--onto`. `grind` simulates from `HEAD`, which is
the only thing it *can* simulate from, so there is no second ref to give it.

Unlike `grist`, `grind` has no answer to pipe — the answer *is* the exit code —
so `-q` means silence rather than a terser rendering. It covers both streams:
the verdict on stdout, the uncommitted-work note and any error on stderr. A
caller redirecting stdout to `/dev/null` and getting chatter on the terminal
anyway has not been given a quiet tool.

```console
# only start the real rebase if the dry run says it is free
$ grind -q main && git rebase main
```

```bash
grind -q "$target"
case $? in
  0) git rebase "$target" ;;
  1) echo "conflicts ahead; book the afternoon" ;;
  *) grind "$target" ;;  # not quiet this time, so it explains itself
esac
```

## The three numbers

| Number | What it counts |
| --- | --- |
| **Hunks** | Conflict regions you would hand-merge. The closest proxy for actual work. |
| **Stops** | Times the rebase halts and waits for you. Fixed overhead per interruption. |
| **Files** | Distinct files any conflict touched. Blast radius. |

Stops is the number a merge cannot give you and the reason `grind` walks the
whole rebase instead of bailing at the first collision. A branch that rewrote
the same region across three commits stops you three times; a branch that landed
the identical change in one commit stops you once. Measured at the first
collision, or measured as a merge, those two branches look like the same
afternoon.

The per-file breakdown is part of the answer rather than decoration: "4 hunks
across 2 files" tells you how much work is coming, not where it lands, and those
are different planning problems.

## Uncommitted work

```console
$ grind origin/main
grind: note: 3 uncommitted files are not included; simulating from HEAD
grind: clean - replaying HEAD onto origin/main hit no conflicts
```

A dirty tree is not an error. `grind` simulates from `HEAD`, so it says so and
carries on — the exit code does not move, and the note goes to stderr so a
caller piping stdout gets byte-identical output either way. It exists purely so
a `clean` verdict is never misread as covering work that was never committed.

The count is staged, unstaged and untracked files alike, counted per file, so a
newly created directory is reported as the files in it rather than as one entry.
A tree with nothing uncommitted says nothing at all; a note printed
unconditionally would be noise people learn to ignore.

## What it does to your repo

Nothing. Every replay happens in a detached scratch worktree in a temp
directory, torn down afterwards — branch refs are never moved, `rerere` never
records a simulated resolution into your shared `rr-cache`, no hook fires, and
teardown removes the scratch worktree by path rather than running the repo-wide
`git worktree prune`.

None of that is implemented here. It all belongs to the shared
[`gitscratch`](../gitscratch/README.md) harness, which is the only way to get a
scratch worktree and only hands out a git runner already carrying the whole
configuration — so `grind`, `grist` and anything built later cannot drift onto a
weaker version of it. That README has the full table and the reason for every
row.

## No shell integration

`grind` has no `--shell-setup`, and should never grow one. It mutates nothing in
the parent shell — it prints a verdict and exits with a number — so by this
repository's own rule ([CLAUDE.md](../../CLAUDE.md)) it does not get to write
into your shell config. The tools here that do install a function, `cwt`, `nwt`
and `crap`, all have to change the parent shell's working directory, which a
child process physically cannot do. If you want `gr` as a shorthand, that is an
`alias` you own.

## Reading the numbers honestly

When a replay conflicts, `grind` resolves it by staging the conflict markers
verbatim and carrying on, which is how it reaches the *end* of the rebase and
can report three stops rather than one. That is the conservative choice — unlike
`--ours` or `--theirs` it never silently discards a side — and it mirrors
reality, in that a human resolution also leaves later commits conflicting
against the resolved state.

It is still a model, and it runs slightly hot on hunks. A file that conflicts at
several stops is counted with the earlier stops' markers still in it, so its
hunk total climbs faster than the number of fresh collisions. Treat the numbers
as a **cost index measured under identical rules** — good for comparing this
rebase against that one, or today's against last week's — not as a prediction of
exactly how many conflict markers you will meet. The verdict and the exit code
are the product; the totals are supporting evidence.

## Development

```console
cargo test -p grind          # the CLI suite
cargo test -p gitscratch     # the shared harness and its safety suite
cargo build --release -p grind
```

`grind` has no `tests/safety.rs` of its own, and that is the point: everything it
could get wrong about *safety* is `gitscratch`'s, pinned there by mutation —
remove the guard, watch the test fail, put it back.

What `tests/cli.rs` pins is the part `gitscratch` cannot see. Every assertion is
load-bearing on the exit code, because a test that only checked the words on
stdout would pass for a binary that answered every question with the same
number. It covers all three codes individually, `-q` on all three paths, the
four-part `--version` line this repository requires, a conflicted `日本語.txt`
replayed onto a branch named `left-左` surviving intact in name, count and
column, and — the live defect `grind` was written to kill — that an unresolvable
branch name is refused *before* any scratch worktree is built. That last one is
proved by pointing
`TMPDIR` at a directory that does not exist, so creating a scratch is guaranteed
to fail, with a control run on a resolvable branch to show the poison really
does reach `Scratch::create` rather than being quietly ignored.
