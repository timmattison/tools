# Mutation verification

Every safety test in this crate has been watched to fail.

That is the whole reason this file exists. `CLAUDE.md`'s enforced-helper rule
asks that any guard be proven capable of failing, because a guard nobody has
ever seen fail is indistinguishable from a guard that is quietly broken — both
report green, forever, and the difference only surfaces the day someone needed
the guard. A test that has never been red is not evidence; it is a comment with
a test harness wrapped around it.

The stakes here are higher than usual. `gitscratch` runs git against the
developer's *real* repository — their branches, their index, their
half-finished working tree, their hooks — and the only thing making that
acceptable is `tests/safety.rs`. If those tests can pass while the harness is
unsafe, the crate has no argument left for why it is allowed to do what it
does.

So each guard was mutated one at a time: break it, run the suite, confirm the
specific test that claims to pin it goes red, read the failure to check it went
red *for the stated reason* rather than incidentally, then restore the source
and confirm green again. What follows is the record, so the next person does
not have to re-derive which guard belongs to which test.

## The map

| Test | Guard it pins | Where the guard lives | Direction |
| --- | --- | --- | --- |
| `never_moves_real_branch_refs_even_when_rebase_update_refs_is_enabled` | `rebase.updateRefs=false` | `src/git.rs`, `Git::safety_config()` — drop the `"rebase.updateRefs=false"` entry | remove |
| `works_when_the_branches_are_checked_out_in_other_worktrees` | The detached checkout of the branch under replay | `tests/safety.rs`, the `replay()` helper — drop `--detach` from `checkout -q --detach <branch>` | remove |
| `never_disturbs_other_worktrees_whose_directories_are_temporarily_missing` | The **absence** of `git worktree prune` in teardown | `src/scratch.rs`, `impl Drop for Scratch` — add a `worktree prune` after the `worktree remove --force` | **add** |
| `never_records_a_rerere_preimage_even_when_rerere_is_enabled` | `rerere.enabled=false`, `rerere.autoupdate=false` | `src/git.rs`, `Git::safety_config()` — drop both entries | remove |
| `never_fires_a_hook_from_the_developer_s_repository` | `core.hooksPath` → the scratch's empty hooks directory | `src/git.rs`, `Git::safety_config()` — drop the chained `core.hooksPath=` argument | remove |
| `never_touches_the_real_working_tree_or_index` | The scratch worktree itself | `src/scratch.rs`, `Scratch::create` — have it hand back a `Git` rooted in the real repository instead of adding a worktree | redirect |
| `never_leaves_a_scratch_worktree_registered_in_the_real_repository` | `worktree remove --force` in teardown | `src/scratch.rs`, `impl Drop for Scratch` — drop the removal | remove |
| `replays_without_hanging_or_failing_when_commit_signing_is_enabled` | `commit.gpgsign=false` | `src/git.rs`, `Git::safety_config()` — drop the entry | remove |

## Why one of these runs backwards

Seven of the eight guards are things the crate *does*, so breaking them means
taking something away. The third is different: the guard is something the crate
deliberately **does not do**, and you cannot remove an absence. Teardown removes
the scratch worktree by path and pointedly never runs `git worktree prune`,
because pruning is repo-wide and immediate — it deletes the administrative state
of every worktree whose directory is merely *missing right now*, including any
halted rebase inside it, which is the normal condition for a worktree on an
unmounted drive or a sleeping network mount.

The only way to prove that test can fail is therefore to *introduce* the
destructive operation and watch it do the damage. Adding the prune is the
mutation. Anyone reading the map and reflexively looking for something to delete
will conclude this guard is untestable; it is not, it just runs the other way.

## The record

### `never_moves_real_branch_refs_even_when_rebase_update_refs_is_enabled`

Mutation: removed `"rebase.updateRefs=false"` from `Git::safety_config()`. With
the fixture having set `rebase.updateRefs=true`, the replay rewrote the very
branch ref it was simulating.

```text
thread 'never_moves_real_branch_refs_even_when_rebase_update_refs_is_enabled'
panicked at src/gitscratch/tests/safety.rs:75:9:
assertion `left == right` failed: replay moved branch 'right'
  left: "e1319cd429e5e7ac8661aad46884c1de7af57875"
 right: "1f107d0331fc2cd47da90214ce95bd234e5e9270"

test result: FAILED. 7 passed; 1 failed
```

No collateral: this is the only test the mutation reddens.

### `works_when_the_branches_are_checked_out_in_other_worktrees`

Mutation: removed `--detach` from the `checkout` in the `replay()` helper in
`tests/safety.rs`, so the replay checks the branch out as a branch. The fixture
has already checked `left` and `right` out in other worktrees, and git refuses:

```text
thread 'works_when_the_branches_are_checked_out_in_other_worktrees'
panicked at src/gitscratch/tests/safety.rs:19:10:
check out the branch detached in the scratch worktree: git checkout -q left failed:

fatal: 'left' is already used by worktree at '/private/var/folders/.../T/.tmp1lHsWT/wt-left'

test result: FAILED. 3 passed; 5 failed
```

**Collateral, and it is worth understanding.** Four other tests go red under
this mutation, for two distinct reasons, and neither is noise:

- `never_disturbs_other_worktrees_whose_directories_are_temporarily_missing`
  fails identically (`fatal: 'left' is already used by worktree at ...`), because
  it too stages a worktree holding `left`.
- `never_moves_real_branch_refs_even_when_rebase_update_refs_is_enabled` and
  `never_touches_the_real_working_tree_or_index` fail with
  `replay moved branch 'right'` — a non-detached checkout means the rebase
  advances the real branch ref, which is exactly the damage those two exist to
  catch. The detached checkout is a second line of defence for the same
  property `rebase.updateRefs=false` protects.
- `never_leaves_a_scratch_worktree_registered_in_the_real_repository` fails
  downstream, at `the rebase was supposed to conflict and halt ... HEAD is up to
  date.`: its earlier blocks moved `left` and `right` for real, so by the time
  the third block asks for a conflicting rebase there is nothing left to
  conflict.

That spread is the guard being load-bearing in more than one place, not a badly
scoped test.

The `--detach` in `Scratch::create`'s `worktree add` was mutated separately, to
establish which site the test actually pins. **It is not this one.** Removing it
reddens all eight tests, every one of them at `Scratch::create` itself:

```text
create the scratch worktree: git worktree add -q .../worktree main failed:

fatal: 'main' is already used by worktree at '/private/var/folders/.../T/.tmpev7i3y'

test result: FAILED. 0 passed; 8 failed
```

That proves something real — the harness cannot even construct a scratch when
the branch it is asked to base on is checked out anywhere, which for `main` is
essentially always — but it proves it about `Scratch::create`, not about the
replay. It is an undiscriminating mutation: everything fails, so nothing is
pinned in particular. The `--detach` the test pins is the one in the `replay()`
helper, which is precisely why that helper spells the checkout out in the test
file rather than hiding it behind a library call.

### `never_disturbs_other_worktrees_whose_directories_are_temporarily_missing`

Mutation (**opposite direction — added, not removed**): inserted a
`worktree prune` in `impl Drop for Scratch`, immediately after the existing
`worktree remove --force`. The test parks an unrelated worktree's directory
aside to stand in for an unmounted volume; the prune sees a directory that is
not there and deletes the worktree's administrative state, halted rebase and
all.

```text
thread 'never_disturbs_other_worktrees_whose_directories_are_temporarily_missing'
panicked at src/gitscratch/tests/safety.rs:146:5:
replay deleted an unrelated worktree's administrative state

test result: FAILED. 7 passed; 1 failed
```

No collateral: this is the only test the mutation reddens.

Afterwards, the absence was re-confirmed by search — there is no `"prune"`
string literal anywhere in the repository's Rust sources. Every textual hit for
"prune" under `src/gitscratch/` is prose explaining why the operation is not
there.

### `never_records_a_rerere_preimage_even_when_rerere_is_enabled`

Mutation: removed `rerere.enabled=false` and `rerere.autoupdate=false` from
`Git::safety_config()`. The replay resolves conflicts by staging markers
verbatim, so an unguarded run files a file full of `<<<<<<<` into the repo-wide
cache as the canonical resolution.

```text
test never_records_a_rerere_preimage_even_when_rerere_is_enabled ... FAILED

replay recorded rerere state in the developer's repository at
/var/.../T/.tmpbORwe3/.git/rr-cache:
/var/.../T/.tmpbORwe3/.git/rr-cache/0f48f634b68406f5f0001e69807ea49a89e60d2f
/var/.../T/.tmpbORwe3/.git/rr-cache/0f48f634b68406f5f0001e69807ea49a89e60d2f/preimage
```

No collateral.

### `never_fires_a_hook_from_the_developer_s_repository`

Mutation: removed the pinned `core.hooksPath` from `Git::safety_config()`. The
test plants executable sentinel hooks in the fixture's common directory — and
proves them armed with a control checkout first — so each path in the failure is
a hook that actually ran.

```text
---- never_fires_a_hook_from_the_developer_s_repository stdout ----
assertion `left == right` failed: replay executed the developer's hooks; each path below is a hook that fired
  left: ".../.git/hook-sentinels/post-checkout\n.../.git/hook-sentinels/post-rewrite\n.../.git/hook-sentinels/pre-rebase"
 right: ""
```

No collateral.

### `never_touches_the_real_working_tree_or_index`

Mutation: made `Scratch::create` operate on the real repository instead of a
scratch worktree. The fixture is deliberately dirty in the contested file, so
git refuses before the replay can do any damage:

```text
thread 'never_touches_the_real_working_tree_or_index' panicked at
src/gitscratch/tests/safety.rs:19:10:
check out the branch detached in the scratch worktree:
git checkout -q --detach left failed:

error: Your local changes to the following files would be overwritten by checkout:
	shared.txt
Please commit your changes or stash them before you switch branches.
Aborting
```

No collateral: every other guard is pinned by configuration that survives being
run in the real repository, so this is the only test that notices.

### `never_leaves_a_scratch_worktree_registered_in_the_real_repository`

Mutation: removed the `worktree remove --force` from `Scratch::drop`. The
scratch's own directory is a `TempDir` that deletes itself regardless, so the
leak leaves no files behind — only an entry git starts advertising as
`prunable`, which is the exact shape the failure names.

```text
assertion `left == right` failed: after a clean replay, the real repository's worktree list changed
  before: /private/var/folders/.../T/.tmpQLZnP2 30f5aa1 [main]
   after: /private/var/folders/.../T/.tmpQLZnP2          30f5aa1 [main]
/private/var/folders/.../T/.tmpJFLfi5/worktree ae9d15c (detached HEAD) prunable

test result: FAILED. 6 passed; 1 failed
```

No collateral.

### `replays_without_hanging_or_failing_when_commit_signing_is_enabled`

Mutation: removed `commit.gpgsign=false` from `Git::safety_config()`. This test
guards against two different outcomes, so both branches were proven separately.

The failure branch, from the mutation itself:

```text
test replays_without_hanging_or_failing_when_commit_signing_is_enabled ... FAILED
commit signing broke the replay: a developer with signing enabled cannot get a trustworthy answer out of a dry run
the replay finished without the commit it was replaying - `git log --format=%s left..HEAD` in the scratch worktree reported "". Signing broke the commit and the resolution loop skipped it, so the failure came back disguised as an answer.

test result: FAILED. 7 passed; 1 failed
```

The hang branch cannot be reached by breaking the guard — a hang needs a signing
program that blocks, which no fixture may summon on a developer's machine — so
it was proven live by shrinking `REPLAY_TIMEOUT` to 1ms:

```text
the replay never came back after 1ms - a dry run that inherited commit signing
is sitting on a passphrase prompt nobody asked for
```

No collateral.

## This is not a one-time ritual

The record above describes the code as it stands, and it decays the moment the
code moves. Three places are load-bearing for the whole table:

- **`Git::safety_config()`** — five of the eight guards are entries in that
  list. Adding, reordering, or removing one changes what the suite covers.
- **`Scratch::create`** — the scratch worktree and its detached `worktree add`.
- **The `Drop` teardown** — both the removal that must happen and the prune that
  must not.

Anyone touching those should re-run the relevant mutation and update this file
with what they saw. A guard added without ever being watched to fail is back to
being a comment, and this crate does not get to ship comments where it has
promised guarantees.
