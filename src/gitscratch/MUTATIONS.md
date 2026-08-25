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
| `never_leaves_a_scratch_worktree_registered_in_the_real_repository` | `rebase.backend=merge` | `src/git.rs`, `Git::safety_config()` — drop the `"rebase.backend=merge"` entry | remove |
| `replays_without_hanging_or_failing_when_commit_signing_is_enabled` | `commit.gpgsign=false` | `src/git.rs`, `Git::safety_config()` — drop the entry | remove |

## What keeps each test honest

A safety test can be incapable of failing and still look exactly like a safety
test. Every test in this file ends in an assertion that something did *not*
happen — no ref moved, no preimage was recorded, no hook fired, no worktree was
left registered — and an assertion of that shape passes just as cheerfully when
the thing it forbids was never possible in the first place. So each test needs
something else alongside the assertion: evidence that it was ever at risk. There
are two kinds of that evidence, they are easy to confuse, and they are not
interchangeable.

A **start-state control** proves the fixture begins where the test needs it to
begin — the working tree really is dirty, `rr-cache` really is empty, exactly one
worktree is really registered. Without one the closing assertion has no baseline
and is measured against nothing. But a start-state control says nothing about
whether the hazard was live. `rr-cache` can be empty at the end because it was
empty at the start and *nothing ever tried to write to it*, which is precisely
what the test would report if rerere had quietly stopped recording, if the config
key had been renamed out from under the fixture, or if the write had gone
somewhere the test is not looking. The test would be green, permanently, and
green for a reason that has nothing to do with the guard.

An **armed control** is the other half: proof that the hazard really would have
happened had the guard not been there. It is a small deliberate demonstration,
run through plain git before anything under test is involved — check the branch
out and watch the planted hook drop its sentinel, make a commit and watch signing
refuse it, ask git how many worktrees are registered while a live `Scratch` is
holding one — after which the evidence is cleared and the real assertion runs
against a clean slate again. A test carrying one cannot go quietly vacuous: on
the day the hazard stops being armed, the control fails and names the reason,
instead of the test passing and saying nothing.

Arming is the half that rots. Guards get rewritten, git changes what it honours,
fixtures get refactored by someone who did not know which line was load-bearing —
and the failure mode is never a red test, it is a green one that has stopped
meaning anything, in the file whose entire job is to be believed. The table below
records what each test actually carries today. It is deliberately unflattering:
several rows say the arming is missing, unasserted, or structural, because a
registry that reports everything as fine is worth less than no registry at all.

| Test | Start-state control | Armed control |
| --- | --- | --- |
| `never_moves_real_branch_refs_even_when_rebase_update_refs_is_enabled` | `repo.rev_parse` panics on a ref that does not resolve, so `main`, `left` and `right` provably exist before the replay; the baseline is then re-read *after* the control has unwound, so the control's own rebase cannot be charged to the replay. Plus the `conflicts` assertions proving a genuine conflict was replayed. | **Full.** The control builds a branch of its own off `main`, commits a file nothing else touches, checks it out **detached** — `--update-refs` skips a branch checked out somewhere, so detaching is what puts the ref at risk, and is exactly what the replay does — and rebases it onto `left` through plain git, with `-c rebase.backend=merge` pinned on that one command, because `--update-refs` is a merge-backend feature and a control that inherited the apply backend would report a dead config key rather than the backend that silenced it. That ref must have moved: ``"`rebase.updateRefs` is not live in <path>, so this test could only pass vacuously; a plain rebase of a detached branch pointing into the replayed range left 'updaterefs-control' sitting at <sha>"``. The fixture is then put back — `checkout main`, `update-ref -d refs/heads/updaterefs-control` — and the restoration is asserted byte-exact against the pre-control ref snapshot plus an empty `status --porcelain`, for the rerere test's reason: the closing assertion cannot tell the control's damage from the replay's. Non-conflicting on purpose; a rebase that halts never reaches the point of updating refs. The fixture also arms `rebase.backend = apply` before any of this and leaves it standing through the replay: it is the backend that would silence `--update-refs`, which is why the control pins its own, and `Git::safety_config` overrides it for everything the harness runs — so what stays armed is a developer rebase configuration the replay has to tolerate and be unaffected by. |
| `works_when_the_branches_are_checked_out_in_other_worktrees` | The two `repo.add_worktree` calls panic if git refuses, so both branches provably are checked out elsewhere when the replay starts. | **Structural, unasserted.** Those worktrees *are* the arming: git physically refuses a non-detached checkout of a branch held in another worktree, so dropping `--detach` cannot pass. `conflicts.files() == Files::new(1)`, `shared.txt` and `hunks() > Hunks::new(0)` stop an empty replay passing. But no assertion states the branches are held, so a change to `add_worktree` in `src/testing.rs` would disarm this silently. |
| `never_disturbs_other_worktrees_whose_directories_are_temporarily_missing` | `assert!(admin_dir.is_dir(), "fixture must start with worktree state that could be lost")`. | **Partial.** The `expect("park the worktree directory")` rename physically creates the missing-directory condition a prune destroys, and the closing restore-then-`git status` proves what survived is a working worktree rather than a leftover directory. Nothing asserts git regarded the parked worktree as prunable while the replay ran; asserting `prunable` in `git worktree list` at that moment would close it. |
| `never_records_a_rerere_preimage_even_when_rerere_is_enabled` | `assert!(!rr_cache.exists(), "fixture must start with nothing recorded, or this proves nothing")`, plus the `conflicts` assertions proving a genuine conflict was resolved. | **Full.** A `git merge --no-ff left` on `right`, through the fixture's own git, must conflict *and* fill the cache — `"rerere is not recording ... so this test could only pass vacuously; a plain conflicting merge left nothing at <path>"` — before anything under test runs. The `--no-ff` answers the `merge.ff = only` the fixture arms and leaves standing: without it git refuses the diverging merge outright, exit 128 with no merge started and no preimage written, and the control's own assertion asks only that the merge *failed*, so it cannot tell a refusal from a conflict. So the control states the shape of its failure as well as the fact of it — `git diff --name-only --diff-filter=U` must be non-empty, which is what a conflict leaves in the index and what no other merge failure produces. Then the fixture is put back: `merge --abort`, back to `main`, and the whole `rr-cache` directory removed, because `merge --abort` pointedly leaves the recording alone and the closing assertion is `!rr_cache.exists()`. The re-assert after clearing is the hooks test's, for the hooks test's reason: the real assertion must not be able to read the control's evidence as the replay's. |
| `never_fires_a_hook_from_the_developer_s_repository` | The sentinel directory is created empty, and after the control run `describe_tree(&sentinels)` is re-asserted `""` so the real assertion cannot mistake the control's evidence for the replay's. | **Full, and the model for the rest.** A `repo.checkout` pair through the fixture's own git must leave `post-checkout` behind — `"the planted hooks are not armed, so this test could only pass vacuously"` — before anything under test runs. Caveat: only `post-checkout` is proven to fire. `pre-rebase`, `post-rewrite` and `pre-merge-commit` are planted identically but never individually armed, and the last cannot fire from a rebase-only replay at all, which its own doc comment says. Non-Unix skips the test rather than passing it vacuously. |
| `never_touches_the_real_working_tree_or_index` | Three, all explicit: `!before_status.is_empty()`, `!before_index.is_empty()`, and `before_branch == "main"` so a stray detach is visible. Plus the `conflicts` assertions. | **Structural, and un-armable in-test by design.** The file dirtied on purpose is `shared.txt`, the exact file both replayed branches rewrite, so a replay that escaped its scratch would have to collide with it. Arming that in-test means performing the damage the test exists to forbid; the mutation record below is the out-of-band substitute, and it is the reason this row is acceptable rather than merely unfinished. |
| `never_leaves_a_scratch_worktree_registered_in_the_real_repository` | `before.lines().count() == 1` and `describe_tree(&worktrees_dir) == ""` — "or a leak has somewhere to hide". | **Full, twice over.** While the first `Scratch` is alive, `while_alive.lines().count() == 2` and `assert_ne!(describe_tree(&worktrees_dir), "")` — "or this test can only pass vacuously" — prove the harness really registers what teardown must remove. The third scope arms its own harder case separately: `!halted.success` and the `rebase-merge` path existing prove the scratch really was dropped mid-rebase. That `rebase-merge` assertion is now the armed control for a second guard as well — the fixture arms `rebase.backend = apply` and leaves it standing, so the path the assertion names is the one `Git::safety_config`'s `rebase.backend=merge` chooses over the fixture's, and dropping that pin sends the state to `rebase-apply`. Only the first scope proves registration, though all three build a `Scratch` the same way. |
| `replays_without_hanging_or_failing_when_commit_signing_is_enabled` | Implicit: `TestRepo::init` pins `commit.gpgsign=false` while building the fixture and signing is switched on afterwards, so the control below doubles as proof the config took. | **Full.** A plain `git commit --allow-empty` through the fixture must *fail* — `"commit signing is not armed ... a plain commit succeeded"` — and fail for the stated reason, `gpg failed to sign`. `--allow-empty` means arming leaves the fixture exactly as it found it. A second control lives inside `replay_under_signing`: the replayed commit must still be in `left..HEAD`, which catches a signing failure that came back disguised as a plausible answer. The hang branch cannot be armed at all — see the record below. |

### The rule for the next test

A new safety test in this crate is not finished when it goes green. It is
finished when it carries an armed control, or when this table records in plain
words why one cannot exist — and "I could not think of one" is not that reason.
Two rows above are legitimate impossibilities, and they show the shape the reason
has to take: the hang branch of the signing test would need a signing program
that blocks, which no fixture may summon on a developer's machine, and the
working-tree test would have to turn a replay loose in the real repository to
prove it would do damage. Both fall back to the mutation record below, which is
the same proof taken out of band, by hand, once.

Two further things this table asks of you. Quote the assertion, so the next
person can check the claim without re-deriving it from the test. And when the
arming is *structural* — enforced by git or by the fixture's shape rather than by
an assertion — say so rather than calling it armed, because structural arming is
the kind that disappears without a sound: nothing fails at the moment it is lost,
and the test keeps reporting green about a hazard that is no longer there.

## Why one of these runs backwards

Eight of the nine guards are things the crate *does*, so breaking them means
taking something away. Nine, not eight: the map above is one row per guard
rather than one per test, so the `rerere` pair counts once — one mutation
removes both entries — and the worktree test carries two rows, one for each
guard it pins. The third row is different: the guard is something the crate
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
panicked at src/gitscratch/tests/safety.rs:
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
panicked at src/gitscratch/tests/safety.rs:
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
panicked at src/gitscratch/tests/safety.rs:
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
src/gitscratch/tests/safety.rs:
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

test result: FAILED. 7 passed; 1 failed
```

No collateral.

### `rebase.backend=merge`, through `never_leaves_a_scratch_worktree_registered_in_the_real_repository`

Mutation: removed `"rebase.backend=merge"` from `Git::safety_config()`. The
fixture arms `rebase.backend = apply`, so with the harness's pin gone the replay
inherits the developer's backend, and the rebase that halts files its state at
`rebase-apply` instead. The block that drops a `Scratch` mid-rebase looks for
`rebase-merge`, so it reports a scratch that is not mid-rebase at all:

```text
thread 'never_leaves_a_scratch_worktree_registered_in_the_real_repository'
panicked at src/gitscratch/tests/safety.rs:
the scratch should be mid-rebase at .../worktrees/worktree/rebase-merge when it is dropped

test result: FAILED. 7 passed; 1 failed
```

No collateral: this is the only test the mutation reddens.

The pin earns its keep in the sibling test as well, which is the point of adding
it. With the backend pinned, removing `"rebase.updateRefs=false"` reddens
`never_moves_real_branch_refs_even_when_rebase_update_refs_is_enabled` at
`replay moved branch 'right'` even on a machine carrying a global
`[rebase] backend = apply` — the case where that mutation used to come back
green, because the backend the replay inherited ignored `--update-refs`, so the
ref stayed put and the missing guard looked harmless.

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

- **`Git::safety_config()`** — five of the nine guards are entries in that
  list. Adding, reordering, or removing one changes what the suite covers.
- **`Scratch::create`** — the scratch worktree and its detached `worktree add`.
- **The `Drop` teardown** — both the removal that must happen and the prune that
  must not.

Anyone touching those should re-run the relevant mutation and update this file
with what they saw. A guard added without ever being watched to fail is back to
being a comment, and this crate does not get to ship comments where it has
promised guarantees.
