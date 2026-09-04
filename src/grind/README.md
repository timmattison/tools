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

The table is about a run that names a `BRANCH`. `--help` and `--version` ask
about the tool rather than about a rebase, so each answers and exits `0` with no
replay behind it. A command line the argument parser refuses exits `2`, which is
already the code for "I could not tell you".

Three codes and *only* three, on every path. `grind main | head -1` closes the
pipe before the verdict is finished, and a Rust program meets that as a write
error rather than as `SIGPIPE`; the words are what a vanished reader costs, so a
failed write is discarded instead of becoming a panic and a fourth, undocumented
exit code. The same goes for the note and the error message when stderr is the
stream that went away.

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
grind: error: could not resolve 'mian' to a commit: git rev-parse --verify --end-of-options mian^{commit} failed:

fatal: Needed a single revision
$ echo $?
2
```

Errors carry git's own explanation through to you, because that is usually the
only part of the message that says what actually went wrong. Git is terse when
it refuses a revision, so `grind` names the revision itself in the sentence
ahead of git's.

The question is put to git in the one form git can refuse. A bare
`git rev-parse mian^{commit}` prints its argument back and exits **0**, because
rev-parse passes an argument it cannot place through to rev-list rather than
refusing it — so the check that is supposed to catch a typo passed every one of
them. `--verify` makes git answer with one commit id or fail, and
`--end-of-options` ends git's own option position so a branch whose name starts
with a dash arrives as a revision rather than as a flag:

```console
$ grind -- --root
grind: error: could not resolve '--root' to a commit: git rev-parse --verify --end-of-options --root^{commit} failed:

fatal: Needed a single revision
$ echo $?
2
```

That name is the one that cost the most. Git knows `--root` as an option of
`rebase`, so a replay handed it rebased the whole history onto nothing, hit no
conflict, and reported `grind: clean` at exit 0 — a clean verdict for a branch
that does not exist.

**Both** revisions are resolved up front, not just the one you typed. A replay
starts from `HEAD`, so an empty repository or a fresh `git checkout --orphan` has
nothing to replay — and being told that is worth more than watching a scratch
worktree get built for a rebase that could never have started:

```console
$ grind main
grind: error: a replay starts from HEAD, and there is no commit at HEAD to start from - an empty repository, or a branch nothing has been committed to yet: could not resolve 'HEAD' to a commit: git rev-parse --verify --end-of-options HEAD^{commit} failed:

fatal: Needed a single revision
$ echo $?
2
```

## Usage

```console
grind [-q] <BRANCH>
```

| Argument | Meaning |
| --- | --- |
| `<BRANCH>` | What to rebase HEAD onto. Anything git resolves to a commit works — a branch, a remote-tracking ref, a tag, a raw sha. |
| `-q`, `--quiet` | Print nothing about the rebase. The exit code is still the answer. |

One positional argument and no `--onto`. `grind` simulates from `HEAD`, which is
the only thing it *can* simulate from, so there is no second ref to give it.

Unlike `grist`, `grind` has no answer to pipe — the answer *is* the exit code —
so `-q` means silence rather than a terser rendering. It covers both streams:
the verdict on stdout, the uncommitted-work note and any error on stderr. A
caller redirecting stdout to `/dev/null` and getting chatter on the terminal
anyway has not been given a quiet tool.

`-q` silences everything `grind` itself says, and stops there. The argument
parser answers before `grind` starts. So `grind -q` with no `BRANCH` still
prints a usage error and exits `2`, and `grind -q --version` still prints the
version and exits `0`. Both answer about the tool rather than about a rebase.
The missing `BRANCH` is the likely one in the script below. A caller left with a
bare `2` and no word about which argument is missing is worse off.

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

The counts all start in the same terminal column, which is the whole reason the
names are measured in display width rather than in bytes or characters. `grind`
measures your terminal and keeps that column on screen: a path too wide to pad
takes a row of its own and its count takes the next row, still in the shared
column. The path itself is never cut short, because a truncated path opens no
file. The measurement is of your terminal itself rather than of where stdout
goes, so `grind main | less` lays the breakdown out for the window you are
sitting at; a run that holds no terminal at all, such as one under a CI job,
lays it out for 80 columns. A control character in a name is spelled out as
`\u{...}`, so a name holding a newline stays one row and a name holding an ESC
cannot write an escape sequence to your terminal.

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

The note waits for the verdict it qualifies. A run that cannot build a scratch
worktree, or whose replay fails outright, prints its error and no note: a caveat
about an answer that never arrives is a wrong sentence rather than an early one.
The count is still read before the scratch worktree is built, because a scratch
worktree can land inside the repository and be counted as your own work.

Being a caveat, it also cannot take the answer away. Some repositories have no
working tree to take a status of — a bare one, where `git status` simply refuses
— and a replay does not need one, so `grind` runs in a bare repository and
answers normally; you just do not get the note. A question the tool asked for
your benefit is not allowed to be the reason it cannot answer the question you
asked.

## What it does to your repo

Nothing. Every replay happens in a detached scratch worktree in a temp
directory, torn down afterwards — branch refs are never moved, `rerere` never
records a simulated resolution into your shared `rr-cache`, no hook fires, and
teardown removes the scratch worktree by path rather than running the repo-wide
`git worktree prune`.

None of that is implemented here. It all belongs to the shared
[`gitscratch`](../gitscratch/README.md) harness, which is the only way to get a
scratch worktree and answers only the operations it names, each of them under
the whole configuration — so `grind`, `grist` and anything built later cannot
drift onto a weaker version of it. The git runner that makes those calls never
leaves that crate, because a scratch worktree is a linked worktree of your real
repository and the configuration says nothing about `branch -D` or `push`. That
README has the full table and the reason for every row.

## No shell integration

`grind` has no `--shell-setup`, and should never grow one. It mutates nothing in
the parent shell — it prints a verdict and exits with a number — so by this
repository's own rule ([CLAUDE.md](../../CLAUDE.md)) it does not get to write
into your shell config. The tools here that legitimately install one — `cwt`,
`nwt`, `crap` — all have to change the parent shell's working directory, which a
child process physically cannot do. `prcp` was the exception that proved it. It
shipped a `prmv` function that only forwarded to `prcp --rm`, and that flag is
gone now — `prcp --rm` is the whole interface, and the shorthand is an alias the
user writes. If you want a shorthand for `grind`, that is an `alias` you own.

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

A hunk is a closed conflict region — an opening marker, and the closing marker
after it — and both are matched exactly, so a line of file content that merely
begins with brackets is not one. `merge.conflictStyle` is pinned beside that
rule, because `diff3` and `zdiff3` put the base version inside the region, so
the same replay measures the same file whatever your own git config says.

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
number. It covers all three codes individually, `-q` on all three paths and
`--quiet` answering byte for byte as `-q` does, the four-part `--version` line
this repository requires, a conflicted `日本語.txt` replayed onto a branch named
`left-左` surviving intact in name, count and column, and — the live defect
`grind` was written to kill — that an unresolvable branch name is refused
*before* any scratch worktree is built. That last one is proved by pointing
`TMPDIR` at a directory that does not exist, so creating a scratch is guaranteed
to fail, with a control run on a resolvable branch to show the poison really
does reach the worktree-building half of `Repo::scratch` rather than being
quietly ignored.

Three more pin the surface on either side of `grind`'s own writes: the first
line of `--help` against the sentence the source carries, and the usage error
and version line that `-q` never reaches.

Two of the failure tests now run over a *dirty* tree, so each has a caveat to
hold back and can be caught printing one — the poisoned-`TMPDIR` control, which
dies building the scratch worktree, and the replay that fails outright.

Three of them are about a right answer surviving something going wrong around it,
which is where a tool whose answer is a number is most easily robbed of it:

- **A stream nobody is reading.** The pipe's read end is closed *before* the
  child is spawned, so there is nothing to race — the first byte `grind` writes
  fails, and the run still has to exit `0`/`1`/`2` rather than `101`. Both
  streams, so the note and the error message are covered as well as the verdict.
- **A bare repository**, where `git status` cannot run but `git worktree add` can.
  The verdict has to come back byte-identical to the one the same fixture gives
  through its working tree, since the caveat is all that was ever unavailable.
- **A `HEAD` with no commit on it**, from `git checkout --orphan`. The exit code
  was never wrong there, so the assertions are about the message: grind's own
  words, and no scratch path in them. `TMPDIR` is pointed at a directory the test
  knows the name of, which is what makes a leak assertable rather than merely
  unlikely.

Every run in that suite starts from one builder, and the builder pins
`LC_ALL=C` and `LANG=C` on top of the environment scrub. One assertion in the
file matches git's own words rather than grind's — a rebase that fails outright
says `invalid upstream`, and that sentence is the only part that says what went
wrong — and git wraps its words in gettext. A git built with the translations
answers a developer under a non-C locale in that developer's language, and the
assertion then fails for a reason it is not about. The pin is read back off the
built command by a test of its own, because a machine whose git ships no
translations cannot show the failure, and a pin nothing asserts is a pin the
next person deletes.
