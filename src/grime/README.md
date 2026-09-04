# grime

Git ReadIness for Merging Externally — would merging that branch into HEAD
conflict, and by how much?

`grime` answers by actually doing it: it merges the branch into HEAD in a
detached scratch worktree in a temp directory, counts what collided, and tears
the worktree down before you see the answer. Nothing in your repository moves.

```console
$ grime feature
grime: conflicts - merging feature into HEAD
       2 hunks across 2 files

  other.txt    1 hunk
  src.txt      1 hunk

$ grime sidecar
grime: clean - merging sidecar into HEAD hit no conflicts
```

[`grind`](../grind/README.md) is the same question about the other operation —
would rebasing HEAD onto that branch conflict? It prints the same shape, from
the same renderer, so the two answers compare at a glance.
[`grist`](../grist/README.md) is the one for a different question — given
several branches to land, which *order* costs the least?

## The answer is the exit code

| Code | Meaning |
| --- | --- |
| `0` | Clean. The merge hit no conflicts. |
| `1` | Conflicts, and the report says how many and where. |
| `2` | Error. The question could not be answered at all. |

Three codes rather than two, because "the merge would conflict" and "I could not
tell you" are different answers and a script has to be able to act on the
difference.

The table is about a run that names a `BRANCH`. `--help` and `--version` ask
about the tool rather than about a merge, so each answers and exits `0` with no
replay behind it. A command line the argument parser refuses exits `2`, which is
already the code for "I could not tell you".

Three codes and *only* three, on every path. `grime main | head -1` closes the
pipe before the verdict is finished, and a Rust program meets that as a write
error rather than as `SIGPIPE`. The words are what a vanished reader costs, so a
failed write is discarded instead of becoming a panic and a fourth, undocumented
exit code. The same goes for the note and the error message when stderr is the
stream that went away.

`grime` replaces a zsh function of the same name that could not tell the two
apart. It ran git's merge in a throwaway worktree with none of the pins this
crate applies, and it read any non-zero exit from git as conflicts — so a typo'd
branch name came back as conflicts that did not exist, on a branch that did not
either. Every other way git can fail landed in the same bucket.

The branch ref is now resolved **before** a scratch worktree is created, so a
typo fails in milliseconds and can never be mistaken for a conflict:

```console
$ grime faeture
grime: error: could not resolve 'faeture' to a commit: git rev-parse --verify --end-of-options faeture^{commit} failed:

fatal: Needed a single revision
$ echo $?
2
```

Errors carry git's own explanation through to you, because that is usually the
only part of the message that says what actually went wrong. Git is terse when
it refuses a revision, so `grime` names the revision itself in the sentence
ahead of git's.

The question is put to git in the one form git can refuse. A bare
`git rev-parse faeture^{commit}` prints its argument back and exits **0**,
because rev-parse passes an argument it cannot place through to rev-list rather
than refusing it — so the check that is supposed to catch a typo passed every
one of them. `--verify` makes git answer with one commit id or fail, and
`--end-of-options` ends git's own option position so a branch whose name starts
with a dash arrives as a revision rather than as a flag:

```console
$ grime -- --root
grime: error: could not resolve '--root' to a commit: git rev-parse --verify --end-of-options --root^{commit} failed:

fatal: Needed a single revision
$ echo $?
2
```

**Both** revisions are resolved up front, not just the one you typed. A merge
lands on `HEAD`, so an empty repository or a fresh `git checkout --orphan` has
nothing to merge into — and being told that is worth more than watching a
scratch worktree get built for a merge that could never have started:

```console
$ grime main
grime: error: a merge starts from HEAD, and there is no commit at HEAD to merge into - an empty repository, or a branch nothing has been committed to yet: could not resolve 'HEAD' to a commit: git rev-parse --verify --end-of-options HEAD^{commit} failed:

fatal: Needed a single revision
$ echo $?
2
```

## Why the replay is never a fast-forward

The merge runs as `git merge --no-commit --no-ff --end-of-options <branch>`, and
`--no-ff` is the load-bearing flag.

Git takes a merge whose branch is strictly ahead as a fast-forward, and a
fast-forward merges no trees at all. It moves HEAD to the other tip and stops.
A replay of that measures nothing, and then reports `clean` for an operation it
never performed — which is the identical verdict a genuinely free merge earns,
so nothing downstream tells the two apart. A branch cut from the current one
that nothing has diverged from is exactly this shape, and that is an ordinary
branch to ask about.

`--no-ff` makes git perform the three-way merge the caller asked about.
`a_fast_forwardable_merge_still_runs_a_real_three_way_merge` in
[`src/gitscratch/tests/merges.rs`](../gitscratch/tests/merges.rs) pins it. The
test replays a merge git could take as a fast-forward and asserts that HEAD did
not move and that `MERGE_HEAD` exists, because only a real merge records one. It
fails the day somebody drops the flag.

`--no-commit` is the other half of the pair. The merge stops before the commit,
so the replay writes no commit and moves no ref, and the conflicted files stay
in the scratch worktree with git's markers in them — which is where the hunk
count is read from.

## Why unrelated histories are an error

Git refuses to merge two histories with no commit in common. The refusal leaves
no unmerged path behind, so a replay that counted unmerged paths would count
zero — and a zero read as a verdict is the worst answer available:

```console
$ grime alien
grime: error: the merge failed and left nothing to resolve:

fatal: refusing to merge unrelated histories
$ echo $?
2
```

`clean` would say the merge is free when git will not do it at all. `conflicts`
would invent work nobody can sit down and do, because nothing was merged and
there is nothing to resolve. Only an error says what happened, and git's own
sentence travels with it, because that sentence is the part that says which
refusal this was.

The same guard catches every other way a merge can fail while leaving nothing to
measure. It is the reason the exit code stays honest at the one place a count of
zero looks like good news.

## Usage

```console
grime [-q] <BRANCH>
```

| Argument | Meaning |
| --- | --- |
| `<BRANCH>` | What to merge into HEAD. Anything git resolves to a commit works — a branch, a remote-tracking ref, a tag, a raw sha. |
| `-q`, `--quiet` | Print nothing about the merge. The exit code is still the answer. |

One positional argument and nothing else. `grime` merges into `HEAD`, which is
the only thing it *can* merge into, so there is no second ref to give it.

Unlike `grist`, `grime` has no answer to pipe — the answer *is* the exit code —
so `-q` means silence rather than a terser rendering. It covers both streams:
the verdict on stdout, the uncommitted-work note and any error on stderr. A
caller redirecting stdout to `/dev/null` and getting chatter on the terminal
anyway has not been given a quiet tool.

`-q` silences everything `grime` itself says, and stops there. The argument
parser answers before `grime` starts. So `grime -q` with no `BRANCH` still
prints a usage error and exits `2`, and `grime -q --version` still prints the
version and exits `0`. Both answer about the tool rather than about a merge. The
missing `BRANCH` is the likely one in the script below. A caller left with a
bare `2` and no word about which argument is missing is worse off.

```console
# only start the real merge if the dry run says it is free
$ grime -q feature && git merge feature
```

```bash
grime -q "$target"
case $? in
  0) git merge "$target" ;;
  1) echo "conflicts ahead; book the afternoon" ;;
  *) grime "$target" ;;  # not quiet this time, so it explains itself
esac
```

## The two numbers

| Number | What it counts |
| --- | --- |
| **Hunks** | Conflict regions you would hand-merge. The closest proxy for actual work. |
| **Files** | Distinct files any conflict touched. Blast radius. |

`grind` prints a third one, and `grime` deliberately does not. A merge halts
exactly once: git makes one three-way merge and stops at it, so the count is `1`
for every conflicted merge and `0` for every clean one. A constant dressed up as
a measurement invites a reader to weigh it against `grind`'s stop count, which
is a real measurement, and the comparison would be meaningless.

This is the one place the two tools' output genuinely differs, and it is the one
thing `grind` gives you that `grime` cannot. The stop count is the number a
merge cannot produce, and it is `grind`'s own reason for walking a whole rebase
instead of bailing at the first collision: a branch that rewrote the same region
across three commits stops you three times, and a branch that landed the
identical change in one commit stops you once. A merge reports both of them as
the same single halt. When that difference is the thing you are trying to price,
the tool to reach for is `grind`.

The count is dropped from the *words* and nowhere else. The shared `Conflicts`
value still records the halt, because a caller folding several replays together
adds those halts up, and because the two tools have to measure the same thing to
stay comparable.

The per-file breakdown is part of the answer rather than decoration: "2 hunks
across 2 files" tells you how much work is coming, not where it lands, and those
are different planning problems.

The counts all start in the same terminal column, which is the whole reason the
names are measured in display width rather than in bytes or characters. `grime`
measures your terminal and keeps that column on screen: a path too wide to pad
takes a row of its own and its count takes the next row, still in the shared
column. The path itself is never cut short, because a truncated path opens no
file. The measurement is of your terminal itself rather than of where stdout
goes, so `grime feature | less` lays the breakdown out for the window you are
sitting at, and a run that holds no terminal at all, such as one under a CI job,
lays it out for 80 columns. A control character in a name is spelled out as
`\u{...}`, so a name holding a newline stays one row and a name holding an ESC
cannot write an escape sequence to your terminal. Names are reported from the
repository root, so a run from a subdirectory names the same files the same way
a run from the root does.

You can state the width instead of letting `grime` measure one. A value in
`COLUMNS` wins over the terminal, which is the rule POSIX gives that variable:

```console
$ COLUMNS=40 grime feature
```

Two callers want this. A wrapper such as `viddy(1)` holds the terminal and hands
`grime` a pipe, so the wrapper exports the width it measured. And a test states
a width rather than arranging a terminal to produce one.

## Uncommitted work

```console
$ grime sidecar
grime: note: 1 uncommitted file is not included; simulating from HEAD
grime: clean - merging sidecar into HEAD hit no conflicts
```

A dirty tree is not an error. `grime` simulates from `HEAD`, so it says so and
carries on — the exit code does not move, and the note goes to stderr so a
caller piping stdout gets byte-identical output either way. It exists purely so
a `clean` verdict is never misread as covering work that was never committed.

The count is staged, unstaged and untracked files alike, counted per file, so a
newly created directory is reported as the files in it rather than as one entry.
A tree with nothing uncommitted says nothing at all. A note printed
unconditionally would be noise people learn to ignore.

The note waits for the verdict it qualifies. A run that cannot build a scratch
worktree, or whose merge fails outright, prints its error and no note: a caveat
about an answer that never arrives is a wrong sentence rather than an early one.
The count is still read before the scratch worktree is built, because a scratch
worktree can land inside the repository and be counted as your own work.

Being a caveat, it also cannot take the answer away. Some repositories have no
working tree to take a status of — a bare one, where `git status` simply refuses
— and a merge replay does not need one, so `grime` runs in a bare repository and
answers normally. You just do not get the note. A question the tool asked for
your benefit is not allowed to be the reason it cannot answer the question you
asked.

## What it does to your repo

Nothing. Every replay happens in a detached scratch worktree in a temp
directory, torn down afterwards — no commit is written, branch refs are never
moved, `rerere` never records a simulated resolution into your shared
`rr-cache`, no hook fires, and teardown removes the scratch worktree by path
rather than running the repo-wide `git worktree prune`.

None of that is implemented here. It all belongs to the shared
[`gitscratch`](../gitscratch/README.md) harness, which is the only way to get a
scratch worktree and answers only the operations it names, each of them under
the whole configuration — so `grime`, `grind`, `grist` and anything built later
cannot drift onto a weaker version of it. The git runner that makes those calls
never leaves that crate, because a scratch worktree is a linked worktree of your
real repository and the configuration says nothing about `branch -D` or `push`.
That README has the full table and the reason for every row.

## No shell integration

`grime` has no `--shell-setup`, and should never grow one. It mutates nothing in
the parent shell — it prints a verdict and exits with a number — so by this
repository's own rule ([CLAUDE.md](../../CLAUDE.md)) it does not get to write
into your shell config. That rule is also why `grime` is a binary at all: the
zsh function it replaces ran entirely inside a subshell and returned an exit
code, which is a program rather than a shell function. The tools here that
legitimately install one — `cwt`, `nwt`, `crap` — all have to change the parent
shell's working directory, which a child process physically cannot do. If you
want a shorthand for `grime`, that is an `alias` you own.

## Reading the numbers honestly

Nothing resolves the conflicts. `--no-commit` stops the merge before the commit,
so git's markers stay in the scratch worktree exactly as git wrote them, the
hunks are counted where they lie, and the whole worktree goes away on drop.
Unlike `--ours` or `--theirs`, no side is ever discarded, because no side is
ever chosen.

The numbers are still a model. A hunk is a closed conflict region — an opening
marker, and the closing marker after it — and both are matched exactly, so a
line of file content that merely begins with brackets is not one. An opening
marker that nothing closes is content by the same rule, and a closing marker
with nothing before it closes nothing. A conflict git leaves no markers for at
all — a binary file, an add/add on a blob git will not diff, a delete/modify —
still costs one decision, so it counts as one. `merge.conflictStyle` is pinned
beside those rules, because `diff3` and `zdiff3` put the base version inside the
region, so the same replay measures the same file whatever your own git config
says.

Treat the numbers as a **cost index measured under identical rules** — good for
comparing this merge against that one, or today's against last week's — rather
than as a prediction of exactly how much thinking each region will take. The
verdict and the exit code are the product. The totals are supporting evidence.

## Development

```console
cargo test -p grime          # the CLI suite
cargo test -p gitscratch     # the shared harness and its safety suite
cargo build --release -p grime
```

`grime` has no `tests/safety.rs` of its own, and that is the point: everything it
could get wrong about *safety* is `gitscratch`'s, pinned there by mutation —
remove the guard, watch the test fail, put it back. The `--no-ff` regression test
lives there too, beside the merge it protects.

What `tests/cli.rs` pins is the part `gitscratch` cannot see. Every assertion is
load-bearing on the exit code, because a test that only checked the words on
stdout would pass for a binary that answered every question with the same
number. It covers all three codes individually, `-q` on all three paths and
`--quiet` answering byte for byte as `-q` does, the four-part `--version` line
this repository requires, the first line of `--help` against the sentence the
source carries, a conflicted `日本語.txt` merged from a branch named `right-右`
surviving intact in name, count and column, and a run from a subdirectory naming
both conflicted files by their whole path from the repository root.

Two of them are about the two verdicts a run must never claim. The unrelated
histories are one: git refused the merge, nothing was left to resolve, and the
run has to say neither `clean` nor `conflicts`. The other is the live defect
`grime` was written to kill — an unresolvable branch name is refused *before* any
scratch worktree is built. That one is proved by pointing `TMPDIR` at a directory
that does not exist, so creating a scratch is guaranteed to fail, with a control
run on a resolvable branch to show the poison really does reach the
worktree-building half of `Repo::scratch` rather than being quietly ignored.

One test does nothing but read the whole of stdout for the word `stop`. It lives
apart from the golden verdict beside it because the golden would hide it: a
binary that started printing the stop count again would fail the golden for the
same reason it would fail a change to any other character on that line, and
nobody reading that failure would learn which claim broke.

Several are about a right answer surviving something going wrong around it,
which is where a tool whose answer is a number is most easily robbed of it:

- **A stream nobody is reading.** The pipe's read end is closed *before* the
  child is spawned, so there is nothing to race — the first byte `grime` writes
  fails, and the run still has to exit `0`/`1`/`2` rather than `101`. Both
  streams, so the note and the error message are covered as well as the verdict.
- **A bare repository**, where `git status` cannot run but `git worktree add` can.
  The verdict has to come back byte-identical to the one the same fixture gives
  through its working tree, since the caveat is all that was ever unavailable.
- **A `HEAD` with no commit on it**, from `git checkout --orphan`. The exit code
  was never wrong there, so the assertions are about the message: grime's own
  words, and no scratch path in them. `TMPDIR` is pointed at a directory the test
  knows the name of, which is what makes a leak assertable rather than merely
  unlikely.
- **A leaked git environment**, which is what a hook hands its children. One run
  sets `GIT_DIR`, `GIT_WORK_TREE` and `GIT_PREFIX` at another repository and one
  sets `GIT_INDEX_FILE` at another repository's index. Each asserts twice: the
  answer is about the directory `grime` is standing in, and the repository the
  environment named comes back byte-identical.

Two of the failure tests run over a *dirty* tree, so each has a caveat to hold
back and can be caught printing one — the poisoned-`TMPDIR` control, which dies
building the scratch worktree, and the merge that fails outright.

Every run in that suite starts from one builder, and the builder pins `LC_ALL=C`,
`LANG=C` and `COLUMNS` on top of the environment scrub. One assertion in the file
matches git's own words rather than grime's — the refusal to merge unrelated
histories — and git wraps its words in gettext, so a git built with the
translations answers a developer under a non-C locale in that developer's
language and the assertion then fails for a reason it is not about. `COLUMNS` is
the same shape of problem: a golden with a breakdown in it is laid out for a
width, and a run that measures the developer's window holds in a wide one and
breaks in a narrow one. Both pins are read back off the built command by tests of
their own, because a machine with a wide window and a git that ships no
translations cannot show either failure, and a pin nothing asserts is a pin the
next person deletes.
