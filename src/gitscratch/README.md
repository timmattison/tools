# gitscratch

The hardened harness for dry-running a git operation without touching anything
real.

Answering "would this rebase conflict, and how badly?" means actually performing
the rebase, and performing it means running git against the developer's own
repository. That is only safe because of a specific set of pinned settings. This
crate owns them, so that every tool asking that question inherits the same
guarantees instead of each reimplementing a weaker version.

## The interface

```rust
use gitscratch::Scratch;

// A detached worktree at `main`, in a temp directory, torn down on drop.
let scratch = Scratch::create(repo_path, "main")?;

// Check the candidate out detached, then replay it.
scratch.git().run(&["checkout", "-q", "--detach", "feature"])?;
let conflicts = scratch.replay_rebase("main")?;

if conflicts.is_clean() {
    // Nothing conflicted.
} else {
    for (file, hunks) in conflicts.file_hunks() {
        println!("{file}: {hunks}");
    }
}
```

A `Conflicts` records how many times the replay halted and, for every file that
conflicted, how many hunks it contributed. The headline totals — `hunks()`,
`files()`, `stops()` — are summaries of that breakdown rather than numbers
tracked beside it, so the total and the list underneath it cannot tell a reader
two different stories.

`Scratch` is the only way to get a worktree. It hands out a `Git` that already
carries the whole safety configuration, so there is no way to get a worktree
from here without also getting the hardening — which is the point.

## The pre-flight

Not every question is worth a worktree. `Repo` answers the cheap ones first, so
a mistyped branch name fails in milliseconds with a message naming it, rather
than arriving later disguised as a failed simulation:

```rust
use gitscratch::Repo;

let repo = Repo::open(cwd)?;          // errors if `cwd` is not inside a repository
let onto = repo.resolve("main")?;     // errors naming the revision that did not resolve
let dirty = repo.uncommitted_files()?; // staged + unstaged + untracked, counted per file

let scratch = Scratch::create(repo.path(), &onto)?;
```

These live here rather than in each consuming tool for the same reason as
everything else: `Git::new` is crate-private, so a repository-rooted runner can
only be built from inside this crate. The queries are all reads, which fire no
hooks, so unlike `Scratch` the pre-flight creates nothing at all — no temporary
directory, no worktree, nothing to clean up if it rejects.

## The report

`Report` turns a `Conflicts` into the words a developer reads. It lives here,
not in the binaries, because `grind` (rebase) and `grime` (merge) ask different
questions and have to print the same shape — and two renderers would drift apart
on exactly the details that make the two answers comparable at a glance:

```rust
use gitscratch::Report;

let report = Report::new("grind", "replaying HEAD onto main");

if let Some(note) = report.dirty_note(repo.uncommitted_files()?) {
    eprintln!("{note}");
}
println!("{}", report.render(&conflicts));
```

```console
grind: conflicts - replaying HEAD onto main
       4 hunks across 2 files, 3 stops

  src/lib.rs     3 hunks
  src/main.rs    1 hunk
```

The only variation the two tools get is `Report::without_stops()`, which drops
the stop count for `grime`: a merge halts exactly once, so the number would be a
constant dressed up as a measurement. Everything else is fixed — the indent is
measured from the tool's own name, the counts are padded in *display* width so a
CJK filename still lines its column up, and every noun is pluralised by the
metric newtype that owns it rather than by whoever is printing it.

This is a deliberate, spec-sanctioned acceptance of a little presentation logic
in a library crate. The alternative is two copies of it.

A replay walks the *whole* operation rather than bailing at the first collision,
resolving as it goes by staging the conflict markers verbatim. That is the
conservative auto-resolution: unlike `--ours` or `--theirs` it never silently
discards a side. It does mean a later commit touching the same region conflicts
again, which is faithful to reality, since a human resolution also leaves later
commits conflicting against the resolved state. Treat a `Conflicts` as a cost
index measured under identical rules, not as an exact prediction.

## What it guarantees

| Guard | Why |
| --- | --- |
| `rebase.updateRefs=false` | Without it, rebasing a detached HEAD still rewrites every branch ref pointing into the replayed range — including the branch being simulated. Not paranoia: with the setting enabled and the guard removed, a dry run *destroys the branch it is replaying*. |
| `rerere.enabled=false`, `rerere.autoupdate=false` | A simulated resolution would otherwise land in the shared `rr-cache` and silently pre-resolve the developer's real merges later. |
| `core.hooksPath` → an empty directory | No hook fires. An empty *value* is not "hooks off" — git still resolves lookups against it — so the path is a real, empty, temporary directory, validated once at creation. `Repo`'s read-only pre-flight points it at a relative path this crate never creates instead: reads fire no hooks, and rejecting a typo must not be able to fail for want of a writable temp directory. |
| `GIT_EDITOR`, `GIT_SEQUENCE_EDITOR`, `GIT_TERMINAL_PROMPT` | A halted rebase would otherwise open an editor and hang forever. |
| `commit.gpgsign=false` | A signing config in the developer's global gitconfig would otherwise prompt or fail mid-replay. |
| `gpg.format=openpgp` | Belt to `commit.gpgsign`'s braces. `gpg.format = ssh` is a different signing backend entirely, with its own key and helper program; pinning the format back to git's default means that configuration is never consulted, so signing cannot be attempted through it. |
| `gc.auto=0` | Simulated commits are loose and nothing references them yet; an opportunistic gc could collect one out from under the run. |
| `rebase.autoStash=false`, `rebase.autosquash=false` | The replay must be the operation as written, not a rewritten variant of it. |
| `user.name=gitscratch`, `user.email=gitscratch@localhost` | Scratch commits are throwaway, but they still have to be attributable to the harness that made them rather than to whichever tool is driving it — and a developer's real name and address have no business being stamped on commits that only ever simulated something. |
| `core.quotePath=false` | Correctness, not cosmetics. By default git C-quotes and octal-escapes any path outside ASCII, so `日本語.txt` comes back from `diff --name-only` as `"\346\227\245\346\234\254\350\252\236.txt"`. That breaks a caller twice: it reports a name nobody typed, *and* the escaped string names no file on disk, so reading it fails and the hunk counter floors that file at 1 — a plausible-looking wrong total. Pinned here rather than fixed with `-z` per call site, because `-z` is per-invocation and the call site that forgets it fails silently. |

Teardown removes the scratch worktree **by path** and deliberately never runs
`git worktree prune`. Pruning is repo-wide and immediate: it deletes the
administrative state — including any halted rebase — of every worktree whose
directory is merely *missing right now*, which is the normal condition for a
worktree on an unmounted drive or a sleeping network mount. A dry run must not
cost the developer a worktree.

## Testing

`tests/safety.rs` pins three properties today, each verified by mutation —
remove the guard, watch that specific test fail, put it back:

- **`rebase.updateRefs=false`**, the first row above, asserted with the setting
  deliberately turned *on* in the repository being replayed.
- **The detached checkout**, which is what lets a branch already checked out in
  another worktree be replayed at all. It is spelled out in the test rather than
  hidden behind a library call precisely because it is a guard.
- **The absence of `git worktree prune` in teardown.** This one is mutated in
  the opposite direction — *add* a prune and watch the test fail — because the
  guarantee is that it is not there.

A fourth guarantee — **the `user.name`/`user.email` identity** — is pinned by a
unit test in `src/git.rs` instead, which reads back `git var GIT_AUTHOR_IDENT`
rather than building a repository to commit into.

**`core.quotePath=false`**, the last row above, is pinned from the other
direction: `tests/conflicts.rs` asserts the *answer* a non-ASCII path produces,
which is the only place the escaping is observable. Both halves of the defect
are asserted together, because they break together — the name and the count.

The remaining rows of the table above — the `rerere` pair, `core.hooksPath`, the
editor and prompt environment, `commit.gpgsign`, `gpg.format`, `gc.auto`, and
the `rebase.autoStash`/`autosquash` pair — are established by construction in
`safety_config` and are **not yet covered by a test**. Issue #329 tracks growing
the suite to eight guarantees and mutation-verifying every guard; the `rerere`
pair, `core.hooksPath` and `commit.gpgsign` are the rows it reaches, so this
paragraph shrinks rather than disappears when it lands.

`tests/repo.rs` covers the pre-flight separately, since what it must get right
is the *cheap rejection*: a directory that is not a repository and a revision
that does not resolve both have to fail there, by name.

`tests/conflicts.rs` covers the answer rather than the safety of getting it:
whether a replay conflicted at all, that the per-file breakdown accumulates
across stops and adds up to the total it explains, and that a conflicted
`日本語.txt` comes back by its real name carrying its real hunk count. That last
one is deliberately built on a file contested in *two* regions — with one, the
undercount and the truth would both be 1 and the defect would pass. `Report`'s
own tests sit beside it in `src/report.rs`, because rendering a `Conflicts` is
pure string work that needs no repository at all.

Consumers pin what they compose on top of the harness. `grist`'s own
`tests/safety.rs` asserts that a full simulation — its `checkout --detach` →
`replay_rebase` → `squash_into` sequence, which this crate's tests cannot see —
leaves every real branch ref where it found it.

The `testing` feature exposes `gitscratch::testing`: throwaway git repositories
with known conflict shapes, shared by every crate built on the harness so the
fixtures exist once rather than once per test binary. Every fixture lives in its
own `TempDir`, so concurrent `cargo test` runs never share a path.

| Fixture | Shape |
| --- | --- |
| `contested_region_repo()` | `iterated` rewrites one region across three commits, `single` touches it once — the asymmetry that makes a stop count worth printing. |
| `stacked_branches_repo()` | `built-on-top` branched from `groundwork`, not from main. |
| `equal_hunks_unequal_stops_repo()` | Two branches making the same two edits, packaged as one commit and as two, so they tie on hunks and differ on stops. |
| `independent_branches_repo()` | Two branches that each add a file of their own, so nothing can conflict. |
| `conflicting_repo()` | Two branches rewriting the same line, so a replay is guaranteed to conflict and resolve. |
| `multi_byte_names_repo()` | Branches `left-左` and `right-右` colliding in `readme.md` and `日本語.txt` — a name git would escape, a hunk count that collapses when it does, and two names whose byte, character and column widths disagree. |
| `not_a_repository()` | A directory outside every repository, which checks its own premise and says so if `TMPDIR` turns out to sit inside one. |

```toml
[dev-dependencies]
gitscratch = { workspace = true, features = ["testing"] }
```

## Used by

- [`grist`](../grist/README.md) — ranks squash-merge orderings by conflict cost
- [`grind`](../grind/README.md) — would rebasing HEAD onto this branch conflict,
  and by how much?
