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
| `a_full_simulation_never_moves_real_branch_refs` (`grist`, `tests/safety.rs`) | The detached checkout inside `Scratch::check_out_detached`, which is the one every consumer uses | `src/scratch.rs`, `Scratch::check_out_detached` — drop `--detach` | remove |
| `never_disturbs_other_worktrees_whose_directories_are_temporarily_missing` | The **absence** of `git worktree prune` in teardown | `src/scratch.rs`, `impl Drop for Scratch` — add a `worktree prune` after the `worktree remove --force` | **add** |
| `never_records_a_rerere_preimage_even_when_rerere_is_enabled` | `rerere.enabled=false`, `rerere.autoupdate=false` | `src/git.rs`, `Git::safety_config()` — drop both entries | remove |
| `never_fires_a_hook_from_the_developer_s_repository` | `core.hooksPath` → the scratch's empty hooks directory | `src/git.rs`, `Git::safety_config()` — drop the chained `core.hooksPath=` argument | remove |
| `never_touches_the_real_working_tree_or_index` | The scratch worktree itself | `src/scratch.rs`, `Scratch::create` — have it hand back a `Git` rooted in the real repository instead of adding a worktree | redirect |
| `never_leaves_a_scratch_worktree_registered_in_the_real_repository` | `worktree remove --force` in teardown | `src/scratch.rs`, `impl Drop for Scratch` — drop the removal | remove |
| `never_leaves_a_scratch_worktree_registered_in_the_real_repository` | `rebase.backend=merge` | `src/git.rs`, `Git::safety_config()` — drop the `"rebase.backend=merge"` entry | remove |
| `replays_without_hanging_or_failing_when_commit_signing_is_enabled` | `commit.gpgsign=false` | `src/git.rs`, `Git::safety_config()` — drop the entry | remove |
| `the_path_check_flags_the_work_tree_and_the_directory_above_it` | `path_at_or_above`, the matcher `gitnuke`, `nodenuke` and `repotidy` read a run's output with | `src/testing.rs`, `candidate_paths()` — keep the first candidate of each start instead of every one | narrow |
| `the_ancestor_check_finds_the_repository_a_directory_sits_inside` | `ancestor_repository`, the precondition `DetachedGitDirRepo::init` refuses a fixture inside a repository with | `src/testing.rs`, `ancestor_repository()` — answer `None` for every directory | narrow |
| `an_argument_cannot_re_pin_a_setting_the_safety_config_fixed` | The subcommand parameter, which keeps a caller's arguments out of git's own option position | `src/git.rs`, `Git::command` — take the subcommand back inside the argument slice, so a caller supplies it and can put arguments ahead of it | widen |
| `an_argument_cannot_aim_the_runner_at_another_repository` | The same parameter, against `-C` rather than against `-c` | `src/git.rs`, `Git::command` — the same mutation | widen |
| `resolves_a_revision_that_names_a_commit_to_its_full_id` | `--verify`, which makes git answer with one commit id or fail | `src/git.rs`, `Git::rev_parse` — drop the `"--verify"` argument | remove |
| `refuses_a_revision_that_starts_with_a_dash_rather_than_echoing_it_back` | The pair `--verify --end-of-options`, which is how the pre-flight asks a question git can refuse | `src/git.rs`, `Git::rev_parse` — drop both arguments | remove |
| `scratch_refuses_a_revision_that_starts_with_a_dash_rather_than_building_one_at_head` (`tests/repo.rs`) | `--end-of-options` ahead of the two positionals of `worktree add` | `src/scratch.rs`, `Scratch::create` — drop the argument | remove |
| `refuses_an_upstream_that_starts_with_a_dash_rather_than_replaying_onto_the_root` | `--end-of-options` ahead of the upstream of `rebase` | `src/scratch.rs`, `Scratch::replay_rebase_within` — drop the argument | remove |
| `pins_automatic_maintenance_off_even_when_the_repository_turns_it_on` | `maintenance.auto=false`, the switch on automatic maintenance that `gc.auto=0` does not reach | `src/git.rs`, `Git::safety_config()` — drop the entry | remove |
| `pins_the_filesystem_monitor_off_even_when_the_repository_names_one` | `core.fsmonitor=false`, the one program git runs that the redirected `core.hooksPath` cannot take away | `src/git.rs`, `Git::safety_config()` — drop the entry | remove |
| `pins_merge_preserving_rebase_off_even_when_the_repository_turns_it_on` | `rebase.rebaseMerges=false`, which keeps a merge commit off the replay's todo list | `src/git.rs`, `Git::safety_config()` — drop the entry | remove |
| `refuses_a_merge_commit_at_a_halt_rather_than_reading_it_as_a_commit_that_changes_nothing` (`src/scratch.rs`) | The parent count read ahead of both probes, which refuses a merge commit at a halt whatever the configuration says | `src/scratch.rs`, `stopped_commit_is_already_in_head` — drop the `stopped_commit_parent_count` call and the `ensure!` under it | remove |
| `refuses_to_report_a_cost_when_a_clean_pick_of_a_submodule_pointer_could_not_be_committed` (`tests/halts.rs`) | `--ignore-submodules=none` on the porcelain half of the empty-commit probe, which is what makes both halves read one tree under one set of rules | `src/scratch.rs`, `stopped_commit_is_already_in_head` — drop the argument from the `git diff` invocation | remove |
| `refuses_to_report_a_cost_when_a_clean_pick_of_a_root_commit_could_not_be_committed` (`tests/halts.rs`) | `--root` on the plumbing half of the empty-commit probe, which is what makes `diff-tree` name the paths a commit with no parent adds | `src/scratch.rs`, `stopped_commit_is_already_in_head` — drop the argument from the `diff-tree` invocation | remove |
| Nothing — see the record below | The round charged for a `--skip`, which no reachable replay can be shown to spend | `src/scratch.rs`, `Scratch::replay_rebase_within` — move `rounds += 1` from the top of the loop into the `Halt::Conflict` arm | narrow |
| `a_path_that_ends_in_whitespace_comes_back_with_that_whitespace_intact` | `Git::path`, the byte-for-byte read of the one path git printed | `src/git.rs`, `Git::path` — read the answer the way `Git::run` reads one, `String::from_utf8_lossy(&output.stdout).trim()` in place of the one-newline strip | redirect |
| `uncommitted_files_counts_a_staged_copy_as_the_one_file_it_is` (`tests/repo.rs`) | The copy letter in the pairing that skips the second field of a copy record | `src/repo.rs`, `moved_from_elsewhere` — drop the `b'C'` arm from the `any` predicate | remove |
| `uncommitted_files_counts_a_working_tree_rename_and_copy_as_the_files_they_are` (`tests/repo.rs`) | The second status byte, the working-tree column, which carries the letter as readily as the index one | `src/repo.rs`, `moved_from_elsewhere` — reduce `[record.first(), record.get(1)]` to `[record.first()]` | narrow |
| The ` ```compile_fail ` doc-test on `Scratch` (`src/scratch.rs`) | The runner staying inside the crate — a consumer is never *handed* one, which is the half `Git::new` being crate-private does not cover | `src/scratch.rs`, `Scratch::git` — put the `pub` back | remove |
| The ` ```compile_fail ` doc-test on `Conflicts` (`src/scratch.rs`) | A cost being measured rather than stated — a released binary has no ungated route to a `Conflicts` that claims one, `Conflicts::nothing_replayed` being a seed for a fold and clean because nothing has run yet | `src/scratch.rs`, the `Conflicts` derive — put `Default` back | **add** |

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
| `an_argument_cannot_re_pin_a_setting_the_safety_config_fixed` | The pinned setting is read back unmodified first and must be `false` — "the safety configuration has to pin `rebase.updateRefs=false`, or there is nothing here for an argument to undo and the assertion below is measured against nothing". | **Full.** Plain git, through the fixture, is handed two `-c` pairs naming one key and must answer with the second — "git no longer lets the last `-c` pair win, so this test could only pass vacuously". That is the hazard itself, demonstrated before the runner is asked anything. |
| `an_argument_cannot_aim_the_runner_at_another_repository` | The runner is asked which repository it is rooted in, and that answer is what the closing assertion compares against, so a runner that could not answer at all fails here rather than passing below. | **Full.** Plain git, run in the first fixture with `-C` naming the second, must answer about the second — the two answers must differ — "`-C` no longer moves git to another directory, so this test could only pass vacuously". Both paths are spelled by git itself, so neither side has to canonicalise a path to compare it. |
| `resolves_a_revision_that_names_a_commit_to_its_full_id` | The fixture commits a file, so HEAD provably names a commit, and the expected id is read back through the fixture's own git rather than written down. | **Structural.** The hazard is the reader answering *wrongly* rather than answering at all, so there is nothing to arm: the assertion compares the reader's answer with git's own, and any extra line, any missing line, and any other commit all fail it. Dropping `--verify` is what makes git prepend `--end-of-options` to its own output, which this test is the only thing in the suite that sees. |
| `refuses_a_revision_that_starts_with_a_dash_rather_than_echoing_it_back` | The fixture commits a file, so the repository is one a revision could resolve in, and the refusal cannot be a refusal of everything - `resolves_a_revision_that_names_a_commit_to_its_full_id` holds that side. | **Full.** Plain git, through the fixture, is asked for `--root^{commit}` and must print that argument straight back — "git no longer prints a dash-leading argument back at exit 0, so this test could only pass vacuously". That echo *is* the hazard: it exits 0, and the pre-flight reads an exit of 0 as a commit. |
| `scratch_refuses_a_revision_that_starts_with_a_dash_rather_than_building_one_at_head` | None beyond the fixture, which `conflicting_repo` builds with a `main` that resolves. | **Missing.** The hazard is that `git worktree add -q --detach <path> --force` succeeds and checks out HEAD, and arming it in-test means building the wrong scratch on purpose and then removing it. Asserting the wrong-scratch HEAD under a deliberately unseparated call closes it. The mutation record below is the out-of-band substitute. |
| `refuses_an_upstream_that_starts_with_a_dash_rather_than_replaying_onto_the_root` | The scratch worktree is checked out at `iterated`, through a call that panics if git refuses, so the replay provably starts somewhere a rebase can run. | **Full, as a control on the other side.** The same scratch then replays onto `single` and must cost `CONTESTED_ROUNDS` stops, so a replay that refused every upstream, or one that could not replay this fixture at all, fails there instead of passing on the refusal above. What is not armed is the halt itself: nothing states that plain `git rebase --root` succeeds on this fixture, and the mutation record below is what stands in for it. |
| `refuses_to_report_a_cost_when_a_clean_pick_of_a_submodule_pointer_could_not_be_committed` | The stopped commit is asked of `diff-tree` first and must touch exactly one path — "the stopped commit has to touch the submodule pointer and nothing else; an ordinary path beside it comes back from the porcelain whatever the setting says, carries the refusal on its own, and leaves what the pointer costs invisible". An ordinary path alongside is what would let this test pass without the pointer ever mattering. | **Full.** Plain git, through the fixture, is asked for `diff --name-only branch~1 branch` and must answer with nothing — "`diff.ignoreSubmodules=all` is not hiding the pointer from `git diff`, so this test could only pass vacuously; the porcelain reported {porcelain:?} for a commit the plumbing reports {bumped:?} for". That silence *is* the hazard: it is the second probe's whole answer under the developer's own configuration, and it is demonstrated before the replay is asked anything. The fixture arms the setting itself rather than reading it out of `~/.gitconfig`, so the control holds on a machine whose developer has never set the key. |
| `refuses_to_report_a_cost_when_a_clean_pick_of_a_root_commit_could_not_be_committed` | The branch's only commit is read back through plain git and must list exactly one field — its own id, and no parent — "the branch's only commit has to be a root commit ... or there is nothing here for `--root` to be load-bearing about". A commit with a parent is a commit `--root` changes no answer for. | **Full, in both directions.** `git diff-tree`, asked through the fixture with the arguments the probe really uses and `--root` left off, must answer with nothing for that commit — "`diff-tree` no longer stays silent about a root commit, so this test could only pass vacuously; that silence is what makes a probe without `--root` read a whole history as a commit that changes nothing". The same call *with* the flag must then name the path the commit adds, so the control proves what the flag buys as well as what its absence costs. The silence is the hazard, demonstrated before the replay is asked anything. |
| `pins_automatic_maintenance_off_even_when_the_repository_turns_it_on` | The fixture sets `maintenance.auto true` in its own repository, and the value is read back before the runner is asked anything. | **Full.** That read-back *is* the arming: plain git, through the fixture, must answer `true` — "the fixture does not hold `maintenance.auto=true`, so there is nothing here for the runner to override and the assertion below is measured against nothing". A key the fixture never took is a key the runner cannot be shown to override. What is **not** armed, and cannot be here, is the damage: the chain from git's `run_auto_maintenance` to a prefetch that writes `refs/prefetch/*` is read from git's source, and arming it would need a developer who has run `git maintenance start` and a remote to fetch from. This test pins the pin, not the consequence, and says so. |
| `pins_the_filesystem_monitor_off_even_when_the_repository_names_one` | The fixture sets `core.fsmonitor .git/hooks/fsmonitor-watchman`, the classic watchman spelling, and the value is read back first. | **Full, for the pin.** Plain git must answer with that path before the runner is asked. The same limit as the row above applies to the *consequence*: proving git would execute the named program means letting a replay execute a program on the developer's machine, which no fixture here may do. `tests/safety.rs` cannot cover this route either — its planted hooks all live under `core.hooksPath`, and this program is executed directly — which is the reason the pin is asserted here at all. |
| `pins_merge_preserving_rebase_off_even_when_the_repository_turns_it_on` | The fixture sets `rebase.rebaseMerges true` and the value is read back first. | **Full, and the consequence was executed out of band.** The read-back arms the pin. The hazard behind it was watched by hand on git 2.55: a branch carrying a merge, rebased onto a moved base under `-c rebase.rebaseMerges=true`, comes out still carrying the merge (`git rev-list --min-parents=2 --count` answers 1), so a developer's own configuration really does put a merge commit on a replay's todo list. That demonstration is a shell session rather than an assertion, because a merge on the todo list only becomes a halt in a repository built to conflict at it, and the reviewer who found this could not construct one. |
| `a_path_that_ends_in_whitespace_comes_back_with_that_whitespace_intact` | The fixture repository is built at a directory whose own name ends in the whitespace under test, and `git init` through the runner panics if git refuses, so the repository provably sits at a path whose last character is the one at stake. | **Full.** The same answer is read back through `Git::run` first and must be missing exactly that character — `assert_eq!(format!("{through_run}{trailing}"), expected.to_string_lossy())`, "`run` no longer eats a space off the end of git's answer, so the assertion below could only pass vacuously". The trimming *is* the hazard, demonstrated before the new reader is asked anything. Both spellings `str::trim` eats get their own fixture: a space, and U+3000, which a Unicode-aware trimmer takes just as readily. |
| `refuses_a_merge_commit_at_a_halt_rather_than_reading_it_as_a_commit_that_changes_nothing` | The stopped commit is read back through plain git and must list three fields — its own id and two parents — "or there is nothing here to refuse". | **Full.** `git diff-tree`, asked through the runner with the arguments the probe really uses, must answer with nothing for that merge — "`diff-tree` no longer stays silent about a merge commit, so this test could only pass vacuously; that silence is what makes an unguarded probe read a merge as a commit that changes nothing". The silence *is* the hazard, demonstrated before the probe is asked anything. A closing control runs the other way: the same probe, pointed at a single-parent commit, must answer rather than refuse, so a probe that refused everything cannot pass. What the test does not build is a real halt — see the record below, and the row above it. |
| `uncommitted_files_counts_a_staged_copy_as_the_one_file_it_is` | The fixture commits `big.txt`, so a copy has a source, and it stages the modification of that source that copy detection needs. Two untracked files sit beside the copy, so the count fails from both directions: pair nothing and the answer is 5, pair every record and it is 3, and only a count that pairs exactly the copy gives 4. | **Full.** Plain git, through the fixture, must report `C  copy.txt`, NUL, `big.txt` — "copy detection is not armed, so this test could only pass vacuously". That control is not a formality: git reports an undetected copy as `A  copy.txt`, one field for one file, so the closing count comes out right while the pairing never runs. The fixture arms `status.renames = copies` in its own repository rather than reading it out of `~/.gitconfig`, so the control holds on a machine whose developer has never set the key. |
| `uncommitted_files_counts_a_working_tree_rename_and_copy_as_the_files_they_are` | The two files the fixture commits hold content of their own, because git pairs a copy with whichever source matches it best and two files spelled alike let it report the rename and the copy against one name — which is what the first draft of this fixture did. Two untracked files sit beside the pair, so the count fails from both directions: 7 with no pairing, 4 with every record paired, 5 only for a count that pairs exactly the two working-tree records. | **Full.** Plain git, through the fixture, must report ` R moved.txt`, NUL, `big.txt` and ` C other-copy.txt`, NUL, `other.txt` — "git no longer reports that in the working-tree column, so this test could only pass vacuously". Without the detection an undetected move is a delete beside an untracked file, which is two fields for two files and never pairs, so the control is what proves the second status byte is under test at all. The `git add -N` that arms it is the everyday route: `git add -p` records the same intent-to-add entry for a new file. |

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

### `the_path_check_flags_the_work_tree_and_the_directory_above_it`

The one guard here that protects other crates rather than this one.
`path_at_or_above` reads the output of `gitnuke`, `nodenuke` or `repotidy` for a
path at or above the work tree of a `DetachedGitDirRepo`, which stands in for
`$HOME`. All three of those tests assert that it answers `None`, and a matcher
that finds nothing answers `None` for every input, so the matcher needs the same
treatment it gives the tools. Two mutations, because the matcher can fail in two
directions.

Mutation one: made `candidate_paths` read no candidate at all. Every plant goes
unseen, and the first assertion reports it.

```text
assertion `left == right` failed: the check must flag the work tree itself
  left: None
 right: Some("/private/var/folders/.../T/.tmpNDd1t2/home")

test result: FAILED. 0 passed; 1 failed
```

Mutation two: made `candidate_paths` keep the first candidate of each start
rather than every one, which is the rule the three copies of this matcher used
before it moved here. A path of one word still matches, and a path that holds a
space does not.

```text
assertion `left == right` failed: the check must flag a path whose name holds a space
  left: None
 right: Some("/private/var/folders/.../T/.tmpXGF3Sa/directory with a space")

test result: FAILED. 0 passed; 1 failed
```

No collateral in either direction: this is the only test the mutations redden.

### `the_ancestor_check_finds_the_repository_a_directory_sits_inside`

The other guard here that protects other crates rather than this one, and the
one that runs before anything else does. `DetachedGitDirRepo` leaves no `.git`
entry, so `repowalker::find_git_repo` walks past the whole fixture and finds
whatever stands above the temporary directory. A repository up there becomes the
root the tool under test works in, and `nodenuke` takes no `--dry-run`: it
deletes every `node_modules`, `.next`, `.open-next` and `.turbo` directory below
its root, plus the lock files beside them. `TempDir` reads `TMPDIR`, and a
`TMPDIR` inside a checkout is a configuration some machines carry, so the fixture
asks the question itself and panics before it builds anything.

`path_at_or_above` reads the output of a run that already happened, which makes
it a post-mortem. This check runs first, so the deletion never starts. Two
mutations, because this matcher also fails in two directions.

Mutation one: made `ancestor_repository` answer `None` for every directory. The
plant above the fixture goes unseen.

```text
assertion `left == right` failed: the check must find the repository above the directory
  left: None
 right: Some("/var/folders/.../T/.tmp1utEXU")

test result: FAILED. 11 passed; 1 failed
```

No collateral: this is the only test that mutation reddens, and that is the
finding rather than a footnote. A check nothing calls and a check that answers
`None` are the same green suite everywhere else.

Mutation two: made `ancestor_repository` answer `Some` for every directory,
which proves the other half — that `init` reads the answer, and reads it before
it makes a git directory of its own. Every fixture in every crate refuses, and
the refusal names the panic site inside `init`:

```text
thread 'a_nested_git_directory_holds_no_repository_nodenuke_can_find'
panicked at src/gitscratch/src/testing.rs:869:13:
the temporary directory /var/folders/.../T/.tmpgVzMgM sits inside the git
repository at /var/folders/.../T/.tmpgVzMgM. A tool built on this fixture walks
upward for a .git entry, finds that repository, and works there. Some of those
tools delete files. Point TMPDIR at a directory that no repository holds.

test result: FAILED. 0 passed; 2 failed
```

Collateral, and intended: every test that builds a `DetachedGitDirRepo` reddens,
in `gitnuke`, `nodenuke` and `repotidy` alike. That spread is the point. The
precondition belongs to the fixture rather than to one guard, so a destructive
tool that takes the fixture next inherits it.

### `an_argument_cannot_re_pin_a_setting_the_safety_config_fixed` and `an_argument_cannot_aim_the_runner_at_another_repository`

Mutation: the shape `Git::command` had before these tests existed. It took one
argument slice, spliced it in after the `-c` pairs and ahead of any subcommand,
and left the caller to supply the subcommand as the first element. So every
argument a caller passed landed in git's own option position. Both tests were
written against that code and watched to fail there, which is why the mutation
is recorded as a widening rather than as a removal: there is no line to delete,
only a parameter to fold back into the slice.

```text
thread 'git::tests::an_argument_cannot_re_pin_a_setting_the_safety_config_fixed'
panicked at src/gitscratch/src/git.rs:
assertion `left == right` failed: an argument reached the position ahead of the
subcommand, where git reads it as one of its own options, and the last `-c` pair
wins.
  left: "OVERRIDDEN"
 right: "false"

thread 'git::tests::an_argument_cannot_aim_the_runner_at_another_repository'
panicked at src/gitscratch/src/git.rs:
assertion `left == right` failed: an argument reached the position ahead of the
subcommand, where `-C` moves git to another directory.
  left: "/private/var/.../T/.tmpWFywDV/.git"
 right: "/private/var/.../T/.tmpOFo44Y/.git"

test result: FAILED. 31 passed; 2 failed
```

No collateral: the other 31 unit tests and every integration suite stayed green
under the old shape, which is the point. The hole was open for the whole life of
the crate and nothing else in the suite could see it.

### The dash-leading revision, across `Git::rev_parse`, `Scratch::create` and `Scratch::replay_rebase`

One defect in three places: a revision arrives from a caller, reaches a git
argv with nothing between it and git's option position, and git reads it as an
option of its own. Each site was mutated on its own.

**`--verify`, removed from `Git::rev_parse`.** Git then prints its own
`--end-of-options` back as a flag ahead of the commit id, so the reader hands
its caller two lines where one was asked for:

```text
thread 'git::tests::resolves_a_revision_that_names_a_commit_to_its_full_id'
panicked at src/gitscratch/src/git.rs:
assertion `left == right` failed: the reader has to agree with git about where
HEAD points
  left: "--end-of-options\n1959a5be9e7225501d6b1de7b4732ae1c7d885e8"
 right: "1959a5be9e7225501d6b1de7b4732ae1c7d885e8"

test result: FAILED. 35 passed; 1 failed
```

**`--end-of-options`, removed from `Git::rev_parse`.** Nothing goes red, and
the row stays in the table anyway. `--verify` catches every dash-leading
revision git recognises today, because an option prints no object id and
`--verify` demands exactly one. That is a fact about today's option list rather
than a rule, and the rule is what this crate takes — the same reasoning that
makes the environment scrub match a prefix instead of a list of names. Recorded
here as unfalsifiable rather than dropped, because a guard nobody can watch fail
is exactly what this file exists to say out loud.

**`--end-of-options`, removed from `Scratch::create`.** `git worktree add -q
--detach <path> --force` is then a complete and valid command: git reads
`--force` as its own flag, finds no commit-ish left, and builds the worktree at
HEAD at exit 0. So the caller gets a scratch of a revision it never asked for,
and every number measured in it is about another branch:

```text
thread 'scratch_refuses_a_revision_that_starts_with_a_dash_rather_than_building_one_at_head'
panicked at src/gitscratch/tests/repo.rs:
a revision that names no commit has to be refused, or the scratch is checked out
somewhere the caller never asked about and every measurement taken in it is
about another branch: ()

test result: FAILED. 0 passed; 1 failed
```

**`--end-of-options`, removed from `Scratch::replay_rebase_within`.** Git knows
`--root` as an option of `rebase`, so the replay rebases the whole history onto
nothing, finishes without a single conflict, and reports the cheapest answer
there is for a revision that names no commit:

```text
thread 'scratch::tests::refuses_an_upstream_that_starts_with_a_dash_rather_than_replaying_onto_the_root'
panicked at src/gitscratch/src/scratch.rs:
an upstream that names no commit has to stop the replay. Git knows `--root` as
an option of `rebase`, so reading it as one replays the whole history onto
nothing, hits no conflict, and reports a cost of zero for a revision nobody
has: "Conflicts { stops: 0, files: {} }"

test result: FAILED. 35 passed; 1 failed
```

No collateral on any of the four: each mutation reddened its own test and left
every other unit test and every integration suite green. That is the finding
rather than a footnote. The whole class was open for the life of the crate, and
`grind -- --root` printed `grind: clean - replaying HEAD onto --root hit no
conflicts` at exit 0 while every one of these suites passed.

### `refuses_to_report_a_cost_when_a_clean_pick_of_a_submodule_pointer_could_not_be_committed`

Mutation: removed `"--ignore-submodules=none"` from the `git diff` invocation in
`stopped_commit_is_already_in_head`. The probe then reads which paths the
stopped commit touched with `diff-tree`, which is plumbing, and whether the new
base holds them with `git diff`, which is porcelain and reads
`diff.ignoreSubmodules` out of the repository's configuration. The fixture sets
that key to `all`, so a commit that moves a submodule pointer and touches
nothing else is one path to the first command and nothing at all to the second:
the touched set is non-empty, so the empty-set guard stays quiet, and the
missing set comes back empty. The replay calls the commit empty and reaches for
`rebase --skip`.

```text
thread 'refuses_to_report_a_cost_when_a_clean_pick_of_a_submodule_pointer_could_not_be_committed'
panicked at src/gitscratch/tests/halts.rs:
a commit that moves a submodule the new base has at another commit is not an
empty commit, whatever `diff.ignoreSubmodules` hides from the porcelain: the
rebase halted on a commit that adds nothing to the new base, but git would not
`rebase --skip` it: 988d57e branch moves the submodule on

test result: FAILED. 6 passed; 1 failed
```

The sealed object database refuses that skip, so the run ends in an error either
way. That is why the assertion is on the wording of the classification rather
than on there being an error: in a repository where the skip succeeds, the same
misclassification finishes the rebase, drops the pointer, and reports a cost of
zero for a branch that was never replayed.

No collateral: this is the only test the mutation reddens, in this crate and in
`grind` and `grist`.

**The plumbing half was mutated separately, and it reddens nothing.** Removing
`"--ignore-submodules=none"` from the `diff-tree` invocation — and from the
`TOUCHED_PATHS` constant in `src/git.rs` that spells the same call for the unit
tests — leaves every suite green. Git documents `diff.ignoreSubmodules` as
reaching the porcelain alone, and git 2.55 was watched to agree: under
`-c diff.ignoreSubmodules=all`, `git diff-tree --name-only` still prints the
moved gitlink. So that argument is unfalsifiable today and it is recorded as
unfalsifiable rather than claimed as a guard. It stays because the rule the
probe rests on is that both of its halves read one tree under one set of rules,
and which of the two consults a config key is a fact about this version of git.

### `refuses_to_report_a_cost_when_a_clean_pick_of_a_root_commit_could_not_be_committed`

Mutation: removed `"--root"` from the `diff-tree` invocation in
`stopped_commit_is_already_in_head`, and from the `TOUCHED_PATHS` constant in
`src/scratch.rs` that spells the same call for the unit tests. `diff-tree`
compares a commit against its parent, so without the flag it prints no path at
all for a commit that has none: the touched set comes back empty, the empty-set
guard answers `Halt::EmptyCommit`, and the replay reaches for `rebase --skip` on
the first commit of a whole history.

```text
thread 'refuses_to_report_a_cost_when_a_clean_pick_of_a_root_commit_could_not_be_committed'
panicked at src/gitscratch/tests/halts.rs:
a root commit whose file is absent from the new base is not an empty commit,
whatever `diff-tree` says about a commit it was not asked for `--root` about:
the rebase halted on a commit that adds nothing to the new base, but git would
not `rebase --skip` it: 76ee41e the unrelated history's own work

test result: FAILED. 7 passed; 1 failed
```

Red for the stated reason: the misclassification is the whole of the failure,
and the message under it is the skip arm being reached at all. The sealed object
database refuses that skip, so the run ends in an error either way — which is why
the assertion is on the classification rather than on there being an error. In a
repository where the skip succeeds, the same misclassification finishes the
rebase, drops the root commit, and reports a cost of zero for a branch that was
never replayed.

No collateral: the run was `cargo test --no-fail-fast -p gitscratch -p grind -p
grist`, and this is the only test of the three crates the mutation reddens. Every
other fixture starts from a base commit both branches share, so no other test
puts a commit with no parent at a halt.

**The `TOUCHED_PATHS` half reddens nothing on its own, and it is a test constant
rather than a guard.** It spells the probe's invocation for
`refuses_a_merge_commit_at_a_halt_rather_than_reading_it_as_a_commit_that_changes_nothing`,
whose control asks `diff-tree` about a *merge* commit — silent with the flag and
without it alike. The constant moved with the probe so that it kept spelling the
same call, and not because anything watched it fail.

**The parent count ahead of the probe was checked against this shape rather than
assumed.** `git rev-list --no-walk --parents` prints one field for a root commit
— its own id, and nothing after it — so `stopped_commit_parent_count` answers
zero and the refusal of a merge commit passes a root commit through. That is the
right answer: a root commit is measurable and a merge commit is not. The two
guards sit on the same line of the probe and answer opposite ways, so the test
carries a start-state control proving which of the two it is exercising.

### The round charged for a `--skip`, which nothing can redden

Mutation: moved `rounds += 1` out of the top of `replay_rebase_within`'s loop and
into the `Halt::Conflict` arm, so a `--skip` round costs nothing. Every test in
`gitscratch`, `grind` and `grist` stayed green.

That is the finding, not a failure to find one, and the reason is structural. The
loop has three halt arms and only one of them comes round again:

- `Halt::Conflict` stages the markers, runs `rebase --continue`, and returns to
  the top of the loop.
- `Halt::UnwritableCommit` stops the replay outright.
- `Halt::EmptyCommit` runs `rebase --skip` and reads the outcome at once,
  stopping the replay unless git exited zero. And `git rebase --skip` exits zero
  only when it has finished the rebase.

So a skip round is always the last round, and a charge on the last round decides
nothing: the budget is read at the *top* of a round, against the rounds charged
before it. Charging the skip and not charging it produce the same answer for
every sequence the loop can reach.

The third bullet is the load-bearing one and it was executed rather than argued.
Git 2.55 was watched twice, in a throwaway repository built the way the fixtures
here are built:

```text
initial exit=1     # git rebase --empty=stop main, halted on an emptied commit
skip1   exit=1     # the skip worked - REBASE_HEAD advanced to the next commit -
                   # and the rebase halted on a second emptied commit
skip2   exit=1     # the skip worked again, and the rebase halted on a conflict
```

`REBASE_HEAD` names a different commit after each of those, so each skip did its
job and git still exited 1. `git rebase --skip` reports the rebase being
unfinished, and cannot report anything narrower.

**Which makes the doc comment this charge used to carry wrong, and it has been
corrected.** It said that a `--skip` leaving the rebase halted and still empty is
the runaway the bound exists to catch, and that an uncharged one spins for ever.
Neither holds: the skip arm's own refusal stops that replay on the first such
skip, long before any bound is reached, and
`refuses_to_report_a_cost_when_an_empty_commit_cannot_be_skipped` in
`tests/halts.rs` pins exactly that — it asserts the error does *not* say "gave
up". The bound catches a resolution that makes no progress, and only that, which
`a_replay_that_outruns_its_budget_still_gives_up` pins.

The charge stays at the top of the loop. It costs nothing, it states the rule
that a round of work costs a round, and an arm added under a charge written into
one branch alone starts uncounted. It is recorded here as unfalsifiable rather
than dropped, for the reason this file gives elsewhere: a guard nobody can watch
fail is exactly what this file exists to say out loud.

**A related defect turned up while this was being measured, and it is written
down here rather than fixed.** The skip arm reads `outcome.success`, and that
exit code cannot separate a skip git refused from a skip that worked and left the
rebase halted on something else. Both of the runs above are the second kind, and
so is the one inside
`refuses_to_report_a_cost_when_an_empty_commit_cannot_be_skipped`, where
`REBASE_HEAD` was watched to advance from the emptied commit to the branch's real
commit before the object write failed. The replay says "git would not
`rebase --skip` it" for all three. The error is loud rather than cheap, so
nothing is thrown away by it, and `Halt::EmptyCommit` is unreachable in
production on git 2.55 in any case — only `--empty=stop` on git's command line
reaches that halt, and `replay_rebase` never passes it. Separating the two needs
a state-based reading of what the skip did, and it changes what
`refuses_to_report_a_cost_when_an_empty_commit_cannot_be_skipped` asserts, so it
belongs to a decision of its own.

### `--literal-pathspecs`, which nothing can redden any more

Mutation: removed `arguments.push("--literal-pathspecs".to_string())` from
`Git::safety_config()`. Every test in `gitscratch`, `grind` and `grist` stayed
green.

That is the finding, not a failure to find one. The pin used to be load-bearing:
the empty-commit probe read a list of paths out of one invocation and handed it
back to the next as pathspecs, where `:/foo.txt` — a `foo.txt` in a directory
named `:` — reads as *from the top of the working tree* and answers about the
root `foo.txt` instead.
`refuses_to_report_a_cost_when_a_clean_pick_of_a_pathspec_magic_path_could_not_be_committed`
reddened on exactly that. The probe now intersects the two path lists in Rust,
so no name this crate reads is ever spelled back to git, and that test passes
with the pin gone.

The pin stays anyway. It costs one argument at the single door every git call in
this crate goes through, and the next call site that hands a path list back is
one edit away — at which point the hazard returns with no warning of its own.
What must not happen is this file claiming a mutation nobody watched, so the
absence is recorded here instead.

### The three settings that let git act on its own

Three entries were removed from `Git::safety_config()` one at a time, and each
one reddened exactly one test. Every run was `cargo test --no-fail-fast -p
gitscratch -p grind -p grist`, so the other two crates really ran rather than
being skipped after the first failure.

Mutation: removed `"maintenance.auto=false"`. `gc.auto=0` stops the gc task
alone, so with this entry gone the fixture's own `maintenance.auto = true`
stands, and git's `run_auto_maintenance` starts the rest of the maintenance
tasks on every commit a replay makes.

```text
thread 'git::tests::pins_automatic_maintenance_off_even_when_the_repository_turns_it_on'
panicked at src/gitscratch/src/git.rs:
assertion `left == right` failed: `maintenance.auto=false` is not pinned, so
git reads `maintenance.auto` out of the developer's own configuration and acts
on it for the length of a replay
  left: "true"
 right: "false"

test result: FAILED. 39 passed; 1 failed
```

Mutation: removed `"core.fsmonitor=false"`. The fixture's own
`core.fsmonitor = .git/hooks/fsmonitor-watchman` then stands, and git executes
that path directly rather than resolving it through `core.hooksPath`.

```text
thread 'git::tests::pins_the_filesystem_monitor_off_even_when_the_repository_names_one'
panicked at src/gitscratch/src/git.rs:
assertion `left == right` failed: `core.fsmonitor=false` is not pinned, so git
reads `core.fsmonitor` out of the developer's own configuration and acts on it
for the length of a replay
  left: ".git/hooks/fsmonitor-watchman"
 right: "false"

test result: FAILED. 39 passed; 1 failed
```

Mutation: removed `"rebase.rebaseMerges=false"`. The fixture's own
`rebase.rebaseMerges = true` then stands, and a replay of a branch carrying a
merge puts that merge on the rebase's todo list.

```text
thread 'git::tests::pins_merge_preserving_rebase_off_even_when_the_repository_turns_it_on'
panicked at src/gitscratch/src/git.rs:
assertion `left == right` failed: `rebase.rebaseMerges=false` is not pinned, so
git reads `rebase.rebaseMerges` out of the developer's own configuration and
acts on it for the length of a replay
  left: "true"
 right: "false"

test result: FAILED. 39 passed; 1 failed
```

No collateral in any of the three: each mutation reddens its own test and
nothing else, in this crate and in `grind` and `grist`.

**What these three pin is the pin, and the record says so rather than claiming
more.** For `maintenance.auto` the chain from `run_auto_maintenance` to a
prefetch that fetches from every remote and writes `refs/prefetch/*` into the
developer's repository is read from git's source; for `core.fsmonitor` the
resolution that runs the named program without consulting `core.hooksPath` is
read from git's settings code. Neither was executed, because executing either
means letting a dry run reach the network or run a program on a developer's
machine, which is the damage the pins exist to prevent. `rebase.rebaseMerges`
is the one of the three whose consequence *was* watched, out of band: on git
2.55 a branch carrying a merge, rebased onto a moved base under
`-c rebase.rebaseMerges=true`, came out still carrying the merge.

### `refuses_a_merge_commit_at_a_halt_rather_than_reading_it_as_a_commit_that_changes_nothing`

Mutation: removed the parent count and the refusal under it from
`stopped_commit_is_already_in_head`, leaving the two probes to answer about a
merge commit on their own. `git diff-tree` reports no changed path for a merge
unless it is asked for `-c`, `--cc` or `-m`, and the probe asks for none of
them, so the touched set comes back empty, the empty-set guard answers
`Halt::EmptyCommit`, and the replay reaches for `rebase --skip`.

```text
thread 'scratch::tests::refuses_a_merge_commit_at_a_halt_rather_than_reading_it_as_a_commit_that_changes_nothing'
panicked at src/gitscratch/src/scratch.rs:
a merge commit at a halt has to stop the replay. `diff-tree` reports no changed
path for one, so classifying it hands back `EmptyCommit`, and the replay skips
a whole side of history and reports a cost for a branch it never replayed: ()

test result: FAILED. 39 passed; 1 failed
```

No collateral: this is the only test the mutation reddens, in this crate and in
`grind` and `grist`. The mutated build also warns that
`stopped_commit_parent_count` and `STOPPED_COMMIT` are never used, which is the
compiler saying the same thing a second way.

**The test drives the probe rather than a replay, and that is a limit worth
stating.** A real merge halt needs `rebase.rebaseMerges` on *and* a conflict at
the merge itself; the reviewer who found this could produce a merge halt only
with unmerged paths, which the code already classifies correctly, and the pin
above now closes that route for the harness anyway. So the fixture builds the
state a halt leaves behind — a merge commit, with `REBASE_HEAD` pointing at it
— and asks the probe the question a halt asks it. What that cannot cover is the
path from `replay_rebase_within` to the probe. That path is exercised by every
other halt test in `tests/halts.rs`, none of which this refusal changes.

### `a_path_that_ends_in_whitespace_comes_back_with_that_whitespace_intact`

Mutation: `Git::path` was made to read its answer the way `Git::run` reads one —
`String::from_utf8_lossy(&output.stdout).trim()` in place of the one-newline
strip and the byte-for-byte conversion.

```text
thread 'git::tests::a_path_that_ends_in_whitespace_comes_back_with_that_whitespace_intact'
panicked at src/gitscratch/src/git.rs:
assertion `left == right` failed: a reader for one path has to hand back the
bytes git printed. A repository directory named with a space on the end spells
that character as the last character of its own path, and a trimmed answer
names a directory nothing holds.
  left: ".../T/.tmpZ4IVds/repository"
 right: ".../T/.tmpZ4IVds/repository "

test result: FAILED. 40 passed; 1 failed
```

Red for the stated reason: the trailing space is the whole difference between
the two paths. No collateral either — the run was `cargo test --no-fail-fast -p
gitscratch -p grind -p grist`, and this is the only test of the three crates
that the mutation reddens. Every other fixture sits at a temporary directory
whose generated name ends in no whitespace at all.

**The call site cannot be mutated on this machine, and that is worth writing
down rather than leaving implied.** Second mutation: `rebase_in_progress` in
`src/scratch.rs` was put back the way the finding found it, reading the rebase
state directory through `Git::run`. The same three-crate run stayed entirely
green.

That is the finding, not a failure to find one. `rev-parse --git-path <name>`
glues the state directory name onto the end of its answer, so the repository's
own last character lands in the *middle* of the path and no trim reaches it
there. What reaches it there is the other half of the same defect — the lossy
decode, which replaces a byte outside UTF-8 wherever it sits — and no fixture on
this machine can arm that: APFS refuses such a name outright, with `EILSEQ`,
before git is involved. The same repository is ordinary on a Linux filesystem,
which is where the call site is reachable.

So the guard is pinned at the reader, where both halves of the loss live and one
of them can be built here, and the call sites are held to it by review. There is
one reader for a path list and one for a single path, and `Git::run` is
neither of them.

### The two letters and the two columns of `moved_from_elsewhere`

One pairing rule, two arms, and neither arm had a test. `git status
--porcelain -z` spends a *second* field on a record that names where the content
came from, so the count skips that field rather than calling one file two. The
rule reads two status bytes for two letters, and the one rename test reached
exactly one of the four spellings: `git mv`, which writes `R` in the index
column. Each arm was mutated on its own. Both runs were `cargo test
--no-fail-fast -p gitscratch -p grind -p grist`, on git 2.50.1 (Apple
Git-155).

**The copy letter, dropped from the predicate.** Under
`status.renames=copies` git writes `C  copy.txt`, NUL, `big.txt` for a copy it
detects beside the modification of its source. Without the arm the count spends
one on each field:

```text
thread 'uncommitted_files_counts_a_staged_copy_as_the_one_file_it_is'
panicked at src/gitscratch/tests/repo.rs:
assertion `left == right` failed: a copy is one uncommitted file, not one per
name its content sits under
  left: Uncommitted(5)
 right: Uncommitted(4)

test result: FAILED. 10 passed; 2 failed
```

Collateral, and it belongs to the same guard: the working-tree test below
reddens too, at `left: Uncommitted(6)` against `right: Uncommitted(5)`, because
its fixture holds a working-tree copy record beside the working-tree rename.
Nothing else in `gitscratch`, `grind` or `grist` notices.

**The second status byte, narrowed away.** Reducing
`[record.first(), record.get(1)]` to `[record.first()]` leaves the rule reading
the index column alone. Both working-tree records open with a space there, so
the pairing stops firing at all:

```text
thread 'uncommitted_files_counts_a_working_tree_rename_and_copy_as_the_files_they_are'
panicked at src/gitscratch/tests/repo.rs:
assertion `left == right` failed: a move and a copy reported in the working-tree
column are one file each
  left: Uncommitted(7)
 right: Uncommitted(5)

test result: FAILED. 11 passed; 1 failed
```

No collateral: this is the only test that mutation reddens, in this crate and in
`grind` and `grist`.

**The working-tree column was probed before it was tested, because the honest
alternative was deleting the arm.** An arm that cannot fire cannot be trusted,
and git's own short-format table lists `R` and `C` under the working-tree column
without saying how one gets there. Renaming a tracked file in the working tree
does *not* produce one: git reports ` D big.txt` beside `?? moved.txt`, since an
untracked file is not in the diff the detection runs over. The route is an
intent-to-add entry — `git add -N`, and `git add -p` on a new file — which puts
the destination in the index with no content behind it and so into that diff.
Git 2.50.1 was watched to answer ` R moved.txt`, NUL, `big.txt` for it, and
` C other-copy.txt`, NUL, `other.txt` for the copy beside it. The arm stays, and
the doc comment on `moved_from_elsewhere` now names all four spellings rather
than saying that either column carries the letter.

**Not every wrong count fails this test, which is why both fixtures carry an
armed control.** Copy detection is off unless the developer turns it on, and
`Git::safety_config` pins nothing about `status.renames`, so the setting arrives
out of the developer's own configuration. An undetected copy comes back as
`A  copy.txt`: one field, one file, and the closing count is right while the
pairing never runs. The control reads the record back through plain git and
requires the two fields, so a git that stopped detecting the copy fails the test
rather than quietly emptying it.


### The ` ```compile_fail ` doc-test on `Scratch`

This guard is not a test in `tests/` and cannot be one. What it pins is what a
consumer can *compile*, and a test that runs has already compiled. Rustdoc
builds a doc-test as a program outside this crate, which is the seat a consumer
sits in, so the block is the only place the property can be stated.

Mutation: `Scratch::git` was made `pub` again — the state the crate was in when
the finding was written.

```text
running 3 tests
test src/gitscratch/src/git.rs - git::shed_inherited_git_environment (line 118) - compile ... ok
test src/gitscratch/src/scratch.rs - scratch::Scratch (line 82) - compile ... ok
test src/gitscratch/src/scratch.rs - scratch::Scratch (line 98) - compile fail ... FAILED

---- src/gitscratch/src/scratch.rs - scratch::Scratch (line 98) stdout ----
Test compiled successfully, but it's marked `compile_fail`.

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

Red for the stated reason: the block compiled, and compiling is the whole of
what it forbids. No collateral either — the run was `cargo test --no-fail-fast
-p gitscratch -p grind -p grist`, and this doc-test is the only thing in the
three crates the mutation reddens. That is expected rather than lucky: no other
test reaches for the runner through `Scratch::git`, because the test suites go
through `Scratch::testing_git`.

**A ` ```compile_fail ` block passes on *any* compile error, so which error it
gets was read rather than assumed.** A block that failed over a typo, a renamed
method or a missing import would report exactly the same green, forever. Second
check, with the source in its shipping state: a temporary test target under
`tests/` — out-of-crate in the same way a doc-test is — was given the same three
lines.

```text
error[E0624]: method `git` is private
   --> src/gitscratch/tests/zz-probe.rs:7:27
    |
  7 |     let _runner = scratch.git();
    |                           ^^^ private method
    |
   ::: src/gitscratch/src/scratch.rs:191:5
    |
191 |     pub(crate) fn git(&self) -> Git {
    |     ------------------------------- private method defined here
```

The refusal is the visibility of `Scratch::git` and nothing else. That target
also had the `testing` feature on, since a test target of this crate always
does, so it records a second fact: `Scratch::testing_git` opens a door of its
own and does not reopen this one.

**The control is a start-state control, and it is worth naming as one.** The
` ```no_run ` block beside the guard carries the same two setup lines with the
named operations in place of the reach, and it has to compile. Without it a
guard that fails over the setup — a `Repo::open` that changed shape, a `scratch`
that stopped returning a `Result` — reads exactly like a guard doing its job.
What the control cannot do is arm the hazard: only the mutation above shows the
block is capable of going red, which is why both are recorded here.

Two more routes to a runner were closed at the same time, and each was watched
to fail from a consumer's own crate rather than argued for. A `grind` binary
that names the type at the crate root is refused with E0425, "cannot find type
Git in crate gitscratch", and a note that the item is gated behind the `testing`
feature. One that names it through the module is refused with E0603, "module git
is private". Neither refusal is a doc-test, because neither is a guard this
crate has to keep on its own: they are the compiler reporting on a module and a
re-export, and anything that reopened either would have to hand a runner back
through `Scratch` as well, which is what the block above forbids.

### The ` ```compile_fail ` doc-test on `Conflicts`

The same kind of guard as the one above, for the same reason: what it pins is
what a consumer can *compile*, and a test that runs has already compiled.

Mutation: `Default` was put back on the `Conflicts` derive — the state the crate
was in when the finding was written.

```text
test src/gitscratch/src/scratch.rs - scratch::Conflicts (line 492) - compile ... ok
test src/gitscratch/src/scratch.rs - scratch::Scratch (line 95) - compile ... ok
test src/gitscratch/src/metrics.rs - metrics (line 37) - compile fail ... ok
test src/gitscratch/src/scratch.rs - scratch::Scratch (line 111) - compile fail ... ok
test src/gitscratch/src/scratch.rs - scratch::Conflicts (line 503) - compile fail ... FAILED

---- src/gitscratch/src/scratch.rs - scratch::Conflicts (line 503) stdout ----
Test compiled successfully, but it's marked `compile_fail`.

test result: FAILED. 6 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

Red for the stated reason: the block compiled, and compiling is the whole of
what it forbids. No collateral — the run was `cargo test --no-fail-fast -p
gitscratch -p grind -p grist`, and exactly one test target of the three crates
reported `FAILED`, the doc-tests, with this one test inside it.

**Which error the block gets was read rather than assumed**, for the reason the
section above gives. A temporary target under `tests/` — out-of-crate in the
same way a doc-test is — was given the line with the source in its shipping
state.

```text
error[E0599]: no associated function or constant named `default` found for struct `Conflicts` in the current scope
 --> src/gitscratch/tests/zz-probe.rs:5:39
  |
5 |     let cost = gitscratch::Conflicts::default();
  |                                       ^^^^^^^ associated function or constant not found in `Conflicts`
  |
note: if you're trying to build a new `Conflicts` consider using one of the following associated functions:
      Conflicts::nothing_replayed
      Conflicts::from_files
```

The refusal is the absence of the derive and nothing else, and the compiler's
own note names the two constructors that remain. That target had the `testing`
feature on, since a test target of this crate always does, so it records a
second fact: `from_files` is not a quiet second route to the same value. It
states a breakdown, and the assertion inside it refuses a breakdown and a stop
count that disagree about whether anything conflicted.

**The control is a start-state control.** The ` ```no_run ` block beside the
guard carries the same setup with a measured `replay_rebase` in place of the
derive, and it has to compile. Without it a guard that fails over the setup — a
`Repo::open` that changed shape, a `scratch` that stopped returning a `Result` —
reads exactly like a guard doing its job. What the control cannot do is arm the
hazard, which is why the mutation above is recorded beside it.

### The ` ```compile_fail ` doc-test on the counters, and why it is not in the map

`src/metrics.rs` carries a third block of the same shape. It writes
`format!("{hunks}")` and passes only while the counters have no `Display`. It
was watched to fail the same way. The `Display` impl was put back on the counter
macro, `test src/gitscratch/src/metrics.rs - metrics (line 37) - compile fail
... FAILED` came back with `Test compiled successfully, but it's marked
compile_fail`, and the same three-crate run reported exactly one failing test
target. The refusal was read from the same kind of temporary target.

```text
error[E0277]: `Hunks` doesn't implement `std::fmt::Display`
 --> src/gitscratch/tests/zz-probe.rs:4:25
  |
4 |     assert_eq!(format!("{hunks}"), "4");
  |                         ^^^^^^^ `Hunks` cannot be formatted with the default formatter
  |
  = help: the trait `std::fmt::Display` is not implemented for `Hunks`
```

It is deliberately absent from the map above. That map is for a guard whose
failure is a plausible wrong answer or damage nobody sees — a runner in a
consumer's hands, a clean verdict for a replay that never happened, a miscounted
rename that reads as an ordinary number. What a `Display` on a counter costs is
`4 across 2` printed on a developer's screen, which is the same visibility that
keeps the two render-boundary tests in `src/report.rs` out of this map. The
README carries the record for it, beside the account of the other three.

### The ` ```compile_fail ` doc-test on the unworded report, and why it is not in the map

`src/report.rs` carries a fourth block of the same shape. It writes
`Report::for_tool("grind").render(…)` and passes only while `for_tool` hands
back an `UnwordedReport`, which owns `describing` and `dirty_note` and no
renderer at all.

Mutation: a `render` was put on `UnwordedReport`, wording the report with an
empty action — the behaviour the crate had when the finding was written, where
`for_tool` seeded `action: ""` and returned a report that could already print.

```text
running 9 tests
test src/gitscratch/src/scratch.rs - scratch::Conflicts (line 492) - compile ... ok
test src/gitscratch/src/git.rs - git::shed_inherited_git_environment (line 118) - compile ... ok
test src/gitscratch/src/scratch.rs - scratch::Scratch (line 95) - compile ... ok
test src/gitscratch/src/metrics.rs - metrics (line 37) - compile fail ... ok
test src/gitscratch/src/scratch.rs - scratch::Scratch (line 111) - compile fail ... ok
test src/gitscratch/src/scratch.rs - scratch::Conflicts (line 503) - compile fail ... ok
test src/gitscratch/src/report.rs - report::Report (line 177) - compile fail ... FAILED
test src/gitscratch/src/metrics.rs - metrics (line 29) ... ok
test src/gitscratch/src/report.rs - report::Report (line 168) ... ok

---- src/gitscratch/src/report.rs - report::Report (line 177) stdout ----
Test compiled successfully, but it's marked `compile_fail`.

test result: FAILED. 8 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

Red for the stated reason: the block compiled, and compiling is the whole of
what it forbids. No collateral — the run was `cargo test --no-fail-fast -p
gitscratch -p grind -p grist`, and exactly one test target of the three crates
reported `FAILED`, the doc-tests, with this one test inside it. The control
beside the guard stayed green through the mutation, which is what says the two
blocks differ by the one line.

**Which error the block gets was read rather than assumed**, for the reason the
sections above give. A temporary target under `tests/` — out-of-crate in the
same way a doc-test is — was given the same two lines with the source in its
shipping state.

```text
error[E0599]: no method named `render` found for struct `UnwordedReport<'a>` in the current scope
 --> src/gitscratch/tests/zz-probe.rs:4:26
  |
4 |     let verdict = report.render(&gitscratch::Conflicts::nothing_replayed());
  |                          ^^^^^^ method not found in `UnwordedReport<'_>`
```

The refusal is the type `for_tool` hands back and nothing else.

The same target read the second half of the finding, which needed no guard of
its own. Wording one report twice used to keep the last action in silence, and
after the split it names a method the worded type does not have.

```text
error[E0599]: no method named `describing` found for struct `gitscratch::Report<'a>` in the current scope
 --> src/gitscratch/tests/zz-probe.rs:5:10
  |
3 |       let report = gitscratch::Report::for_tool("grind")
  |                    -------------------------------------
  |                    |
  |  __________________method `describing` is available on `UnwordedReport<'_>`
  | |
4 | |         .describing("replaying HEAD onto main")
5 | |         .describing("merging feature into HEAD");
  | |         -^^^^^^^^^^ method not found in `gitscratch::Report<'_>`
```

That refusal comes free with the split, so nothing pins it separately. A caller
that genuinely wants two wordings of one tool calls `describing` twice on the
same `UnwordedReport`, which is `Copy`, and holds both results.

It is deliberately absent from the map above, on the same basis as the counter
guard. That map is for a guard whose failure is a plausible wrong answer or
damage nobody sees. What an unworded report costs is
`grind: clean -  hit no conflicts` printed on a developer's screen — two spaces
and a missing phrase, in the one line the tool exists to print — which is the
visibility that keeps the two render-boundary tests in `src/report.rs` out of
this map as well. The README carries the record for it, beside the account of
the other three.

## This is not a one-time ritual

The record above describes the code as it stands, and it decays the moment the
code moves. Every place below is load-bearing for the whole table:

- **`Git::safety_config()`** — five of the nine guards `tests/safety.rs` pins
  are entries in that list, and the unit tests in `src/git.rs` pin four more of
  its entries directly. Adding, reordering, or removing one changes what the
  suite covers.
- **`Scratch::create`** — the scratch worktree and its detached `worktree add`.
- **The `Drop` teardown** — both the removal that must happen and the prune that
  must not.
- **`path_at_or_above` and its `candidate_paths` scan** — the one guard here that
  three other crates rest on, and the one that reddens nothing in this crate
  when it goes narrow.
- **`ancestor_repository` and the `init` that reads it** — the precondition every
  fixture in three other crates starts with, and the only one that runs before a
  destructive tool does.
- **`Git::command`'s argument shape** — the subcommand is a parameter of its own
  so that a caller's arguments land after it. Folding it back into the slice
  reopens git's option position to the caller and undoes every row above it.
- **The separator ahead of every caller-supplied revision** — `--verify` and
  `--end-of-options` in `Git::rev_parse`, and `--end-of-options` in
  `Scratch::create` and `Scratch::replay_rebase_within`. Drop one and git reads
  a revision as an option of its own, which is how a name that names no commit
  buys a clean verdict.
- **`stopped_commit_is_already_in_head`'s two invocations** — they answer the
  same question about one tree, so anything that makes them read it under
  different rules turns a commit git could not write into a commit to skip.
  `--ignore-submodules=none` on both is that rule today. A change that adds a
  filter to one of them, or that hands a path list back to git instead of
  intersecting the two lists here, needs its own mutation and its own row.
  `--root` on the plumbing half belongs to the same line: `diff-tree` compares a
  commit against its parent, so it names nothing at all for a commit that has
  none, and an empty touched set is what the guard below reads as a commit that
  changes nothing.
- **`replay_rebase_within`'s three halt arms** — the round budget is charged once
  at the top of the loop, and which arms that charge can decide anything about is
  a fact about the arms rather than about the charge. Two of the three stop the
  replay, so only a resolution comes round again, and the charge for a `--skip`
  is unfalsifiable today. An arm that starts coming round again after a skip
  makes it falsifiable, and needs its own mutation and its own row.
- **`Scratch::check_out_detached`** — the detached checkout every consumer now
  makes. `tests/safety.rs` spells its own checkout out by hand rather than
  calling this, on purpose: that detach is a guard under test, and a guard read
  through the code it guards proves nothing. So this method's `--detach` is
  covered by `grist`'s `tests/safety.rs` alone. The mutation was run: dropping
  `--detach` here reddens `a_full_simulation_never_moves_real_branch_refs` and
  two more `grist` tests, and nothing in `gitscratch`.
- **`moved_from_elsewhere`'s two status bytes and two letters** — the pairing
  that keeps a moved file from counting as two uncommitted files. It reads both
  status columns for both letters, all four spellings are reachable, and each
  arm has a test in `tests/repo.rs`. A change that narrows either dimension
  needs its own mutation and its own row: the wrong count reads as a plausible
  number in a note about work a replay cannot see.
- **`Scratch`'s public surface** — the named operations are the whole of what a
  consumer may ask a scratch worktree to do, and `Scratch::git` is crate-private
  so nothing else is reachable. A new `pub fn` that hands back a runner, under
  any name, reopens the door whatever the doc-test says about `Scratch::git`;
  `Scratch::testing_git` is behind the `testing` feature for exactly that
  reason, and the feature is how this crate marks everything a test target may
  have and a consumer may not.
- **`Conflicts`'s constructors** — a released binary may seed a fold with
  `nothing_replayed`, and it may hold what a replay measured. There is no third
  route, and the doc-test forbids one spelling of a third route rather than the
  idea of one. A `Default` derive, a `From<usize>`, or any other ungated
  constructor that states a cost reopens the door whatever the block says about
  `default()`, in the way a new `pub fn` returning a runner would reopen the one
  above. `from_files` is behind the `testing` feature for the same reason
  `testing_git` is.
- **`Report`'s two types** — `Report::for_tool` hands back an `UnwordedReport`,
  and `describing` is the only thing that turns one into a `Report`. The
  doc-test forbids one spelling of a report that renders an action nobody gave
  it. Any renderer added to `UnwordedReport`, any second constructor of a
  `Report` that takes no action, or a `describing` on `Report` itself reopens
  the door whatever the block says about `render`, in the way a new `pub fn`
  returning a runner would reopen the one above.
- **The parent count ahead of those two invocations** — both of them answer
  about a single-parent commit, and `git diff-tree` answers about a merge with
  silence rather than with a refusal. The count is what turns that silence into
  a refusal instead of into `EmptyCommit`. It reads the shape of the commit
  rather than a setting, so it is the half that survives a configuration this
  crate has not thought of; `rebase.rebaseMerges=false` is the other half, and
  it closes the one route into it that exists today.

Anyone touching those should re-run the relevant mutation and update this file
with what they saw. A guard added without ever being watched to fail is back to
being a comment, and this crate does not get to ship comments where it has
promised guarantees.
