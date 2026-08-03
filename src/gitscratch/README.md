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

println!(
    "{} hunks across {} files, {} stops",
    conflicts.hunks(),
    conflicts.files(),
    conflicts.stops()
);
```

`Scratch` is the only door in. It hands out a `Git` that already carries the
whole safety configuration, so there is no way to get a worktree from here
without also getting the hardening — which is the point.

A replay walks the *whole* operation rather than bailing at the first collision,
resolving as it goes by staging the conflict markers verbatim. That is the
conservative auto-resolution: unlike `--ours` or `--theirs` it never silently
discards a side. It does mean a later commit touching the same region conflicts
again, which is faithful to reality, since a human resolution also leaves later
commits conflicting against the resolved state. Treat a `Conflicts` as a cost
index measured under identical rules, not as an exact prediction.

## Three ways a rebase halts, and why the third one matters

A halted rebase with **no unmerged paths** is a classification point, not a
single known case. Git stops there for a commit that adds nothing to the new
base — free to drop — and it stops there for a commit it could not *write*,
where dropping it throws the work away and answers with a cost for a branch
that was never replayed. Signing, hooks, a full or read-only object database, an
unusable editor all land in that same state, and git exits non-zero for the
harmless case too, so nothing about the invocation separates them.

| Halt | What it means | What the replay does |
| --- | --- | --- |
| Unmerged paths | A human would hand-merge these | Count them, stage the markers, continue |
| Nothing unmerged, commit adds nothing | The work is already in the new base | `rebase --skip`, costs nothing |
| Nothing unmerged, commit could not be written | git refused to create the commit | **Fail**, naming the commit and quoting git |

The third row is separated from the second by two probes, both of them
repository state rather than a match on git's wording, since wording changes and
state does not:

1. **Anything left uncommitted.** A commit that truly became empty leaves the
   index matching `HEAD` and the worktree matching the index. Content left
   behind is content that failed to be committed.
2. **Whether the stopped commit's work is already in the new base.** A failed
   commit write on a *clean* pick leaves nothing behind at all — git rolls the
   index back and reschedules the pick — so probe 1 has nothing to see. What
   still separates the two is the commit itself: for every path `REBASE_HEAD`
   touches, does `HEAD` already hold exactly that content? If so the commit is
   genuinely empty, and airtight rather than heuristic — applying `C` onto
   `HEAD` is a three-way merge (base `C^`, ours `HEAD`, theirs `C`), and on a
   path where both sides already agree the merge changes nothing, while a path
   `C` never touched cannot change either.

A refused `git rebase --skip` fails the replay immediately, carrying git's own
message, rather than being re-issued until the round limit runs out.

Both probes err toward the loud answer, which is the safe direction: a dry run
may say "this is expensive" or "I cannot answer", but never "this is cheap"
because it quietly discarded the work.

## What it guarantees

| Guard | Why |
| --- | --- |
| `rebase.updateRefs=false` | Without it, rebasing a detached HEAD still rewrites every branch ref pointing into the replayed range — including the branch being simulated. Not paranoia: with the setting enabled and the guard removed, a dry run *destroys the branch it is replaying*. |
| `rerere.enabled=false`, `rerere.autoupdate=false` | A simulated resolution would otherwise land in the shared `rr-cache` and silently pre-resolve the developer's real merges later. |
| `core.hooksPath` → an empty directory | No hook fires. An empty *value* is not "hooks off" — git still resolves lookups against it — so the path is a real, empty, temporary directory, validated once at creation. |
| `GIT_EDITOR`, `GIT_SEQUENCE_EDITOR`, `GIT_TERMINAL_PROMPT` | A halted rebase would otherwise open an editor and hang forever. |
| `commit.gpgsign=false` | A signing config in the developer's global gitconfig would otherwise prompt or fail mid-replay. |
| `gpg.format=openpgp` | Belt to `commit.gpgsign`'s braces. `gpg.format = ssh` is a different signing backend entirely, with its own key and helper program; pinning the format back to git's default means that configuration is never consulted, so signing cannot be attempted through it. |
| `gc.auto=0` | Simulated commits are loose and nothing references them yet; an opportunistic gc could collect one out from under the run. |
| `rebase.autoStash=false`, `rebase.autosquash=false` | The replay must be the operation as written, not a rewritten variant of it. |
| The inherited git environment, shed | Configuration is only as strong as the environment it runs in, because git reads the environment *first*. A tool built on this crate can be invoked from inside a git hook, and git hands its hooks the commit it is making: `GIT_AUTHOR_NAME`/`GIT_AUTHOR_EMAIL`/`GIT_AUTHOR_DATE` naming the developer, and `GIT_INDEX_FILE` — often the *relative* `.git/index`, which silently re-anchors on whichever directory each command runs in. `GIT_DIR` and `GIT_WORK_TREE` travel the same way. Every one is stripped, so a replay cannot be re-attributed or aimed at the repository the developer was committing to. `shed_inherited_git_environment` is public: the danger is not this crate's alone, and the list belongs in one place. |
| `user.name=gitscratch`, `user.email=gitscratch@localhost` | Scratch commits are throwaway, but they still have to be attributable to the harness that made them rather than to whichever tool is driving it — and a developer's real name and address have no business being stamped on commits that only ever simulated something. |

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

A fourth guarantee — **the `user.name`/`user.email` identity**, the last row
above — is pinned by a unit test in `src/git.rs` instead, which reads back
`git var GIT_AUTHOR_IDENT` rather than building a repository to commit into.

`tests/halts.rs` covers the other half of telling the truth: not that the
harness leaves the repository alone, but that it does not report a cheap number
for work it dropped. It puts a replay in each halt state *for real* — a resolved
conflict whose commit cannot be written, a clean pick whose commit cannot be
written, a commit that genuinely became empty, and a `--skip` git refuses — by
making the object database unwritable, which is the only cause of a failed
commit write still reachable through the harness once signing, hooks and the
editor are pinned off. It is Unix-only for that reason.

`tests/hook_environment.rs` runs a whole replay under the environment `git
commit` actually exports to a pre-commit hook — the everyday way a consumer is
invoked, since a pre-commit hook running a test suite runs whatever that suite
drives. It is a test binary of its own because the environment is process-wide,
and cargo gives every integration test file its own process.

The genuinely-empty halt is driven explicitly rather than assumed: on git 2.55 a
patch already upstream is dropped without halting, a resolution that empties a
commit is dropped silently by `rebase --continue`, and the `rebase.empty` config
key is ignored on this path — only `--empty=stop` on git's command line reaches
that stop. The test starts that rebase itself and asserts the halt happened, so
a git that stops halting fails the test instead of quietly passing without
exercising anything.

The remaining rows of the table above — the `rerere` pair, `core.hooksPath`, the
editor and prompt environment, `commit.gpgsign`, `gpg.format`, `gc.auto`, and
the `rebase.autoStash`/`autosquash` pair — are established by construction in
`safety_config` and are **not yet covered by a test**. Issue #329 tracks growing
the suite to eight guarantees and mutation-verifying every guard; the `rerere`
pair, `core.hooksPath` and `commit.gpgsign` are the rows it reaches, so this
paragraph shrinks rather than disappears when it lands.

Consumers pin what they compose on top of the harness. `grist`'s own
`tests/safety.rs` asserts that a full simulation — its `checkout --detach` →
`replay_rebase` → `squash_into` sequence, which this crate's tests cannot see —
leaves every real branch ref where it found it.

The `testing` feature exposes `gitscratch::testing`: throwaway git repositories
with known conflict shapes, shared by every crate built on the harness so the
fixtures exist once rather than once per test binary. Every fixture lives in its
own `TempDir`, so concurrent `cargo test` runs never share a path.

```toml
[dev-dependencies]
gitscratch = { workspace = true, features = ["testing"] }
```

## Used by

- [`grist`](../grist/README.md) — ranks squash-merge orderings by conflict cost
