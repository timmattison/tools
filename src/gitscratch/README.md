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
| `user.name=gitscratch`, `user.email=gitscratch@localhost` | Scratch commits are throwaway, but they still have to be attributable to the harness that made them rather than to whichever tool is driving it — and a developer's real name and address have no business being stamped on commits that only ever simulated something. |

Teardown removes the scratch worktree **by path** and deliberately never runs
`git worktree prune`. Pruning is repo-wide and immediate: it deletes the
administrative state — including any halted rebase — of every worktree whose
directory is merely *missing right now*, which is the normal condition for a
worktree on an unmounted drive or a sleeping network mount. A dry run must not
cost the developer a worktree.

## Testing

`tests/safety.rs` pins eight properties, each verified by mutation — break the
guard, watch that specific test fail, put it back:

- **`rebase.updateRefs=false`**, asserted with the setting deliberately turned
  *on* in the repository being replayed.
- **The detached checkout**, which is what lets a branch already checked out in
  another worktree be replayed at all. It is spelled out in the test rather than
  hidden behind a library call precisely because it is a guard.
- **The absence of `git worktree prune` in teardown.** This one is mutated in
  the opposite direction — *add* a prune and watch the test fail — because the
  guarantee is that it is not there.
- **`rerere.enabled=false`**, asserted with rerere deliberately turned on: a
  conflicting replay must leave `rr-cache` unwritten.
- **`core.hooksPath`**, asserted by planting `post-checkout`, `pre-rebase`,
  `post-rewrite` and `pre-merge-commit` hooks that each touch a sentinel, and
  proving no sentinel appears.
- **The scratch worktree itself**, asserted by dirtying the real working tree
  and index three ways — a tracked edit, a staged change, and an untracked file
  that no reflog or stash could get back — and requiring every one of them to
  survive a replay unchanged, the two on disk compared byte for byte.
- **`worktree remove --force` in teardown**, asserted after a clean run, after a
  resolved conflict, and after a `Scratch` dropped while a rebase was still
  halted — the path most likely to leak a registration.
- **`commit.gpgsign=false`**, asserted with signing turned on and a key that
  cannot resolve. The replay runs under a timeout, so the test catches a hang on
  a passphrase prompt and not only an outright failure.

A ninth guarantee — **the `user.name`/`user.email` identity**, the last row
above — is pinned by a unit test in `src/git.rs` instead, which reads back
`git var GIT_AUTHOR_IDENT` rather than building a repository to commit into.

The remaining rows of the table above — the editor and prompt environment,
`gpg.format`, `gc.auto`, and the `rebase.autoStash`/`autosquash` pair — are
established by construction in `safety_config` and are not covered by a test of
their own. The editor guard is at least exercised indirectly: every conflict
test above drives a rebase that halts, and a halted rebase without `GIT_EDITOR`
set sits waiting on a commit message.

`gpg.format` looks like the signing test covers it and it does not, which is
worth saying out loud so nobody re-derives the wrong answer. That test's fixture
pins `gpg.format=openpgp` itself, deliberately — the format selects *which*
program config git reads, so without it the fake signing program the fixture
names would go unused on a developer who has `gpg.format = ssh` set globally —
and `openpgp` is the same value `safety_config` pins. Removing the harness's
entry would therefore change nothing that test can observe. The pin earns its
place for the reason the table gives; it is just not what makes that test pass.

[`MUTATIONS.md`](./MUTATIONS.md) records which guard each test pins, where that
guard lives, and the failure output captured when it was removed. It also
records the other half of the question — what keeps each test *honest*: the
start-state control proving the fixture began where the test needs it to, and
the armed control proving the hazard would really have fired without the guard.
That second half is the one that rots, and it rots green. Anyone changing
`safety_config`, `Scratch::create` or the teardown should re-run the relevant
mutation rather than trusting a green suite.

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
