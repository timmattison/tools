//! `gitscratch` runs against the developer's real repository. These tests pin
//! the properties that make that acceptable.

use std::path::Path;
use std::process::Command;

use gitscratch::testing::conflicting_repo;
use gitscratch::{Conflicts, Files, Hunks, NoInheritedRepository, Repo, Scratch};

/// Replay `branch` onto `onto` the way a consumer does: check it out detached
/// in the scratch worktree, then rebase.
///
/// Detaching is not incidental — it is the guard test 2 exercises, which is why
/// it is spelled out here in the test rather than hidden behind a library call.
fn replay(scratch: &Scratch, branch: &str, onto: &str) -> Conflicts {
    scratch
        .git()
        .run(&["checkout", "-q", "--detach", branch])
        .expect("check out the branch detached in the scratch worktree");
    scratch
        .replay_rebase(onto)
        .expect("replay the branch onto the simulated base")
}

/// Every path under `dir`, sorted, one per line - so an assertion that says
/// "something was written here" can also say exactly what was written.
///
/// Returns an empty string for a directory that does not exist, which is the
/// case every caller here is hoping for.
fn describe_tree(dir: &Path) -> String {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];

    while let Some(path) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let child = entry.path();
            found.push(child.display().to_string());
            if child.is_dir() {
                pending.push(child);
            }
        }
    }

    found.sort();
    found.join("\n")
}

/// `rebase.updateRefs` rewrites any branch pointing into the range being
/// replayed - which is exactly the branch under replay. A developer who has
/// turned it on must not lose their branches to a dry run.
#[test]
fn never_moves_real_branch_refs_even_when_rebase_update_refs_is_enabled() {
    /// A branch the fixture does not have, created and deleted entirely inside
    /// the control below. It exists so the control has a ref of its own to put
    /// at risk rather than one of the fixture's, and it is gone again before the
    /// baseline is taken, so the `["main", "left", "right"]` snapshot still
    /// covers every ref that outlives it. The name only has to be unique inside
    /// this fixture's own `TempDir`, so a concurrent run of this same test is
    /// creating its namesake in a different repository entirely.
    const CONTROL_BRANCH: &str = "updaterefs-control";
    /// A file nothing else in the fixture touches, so the control's rebase
    /// replays cleanly. That is not a convenience: a rebase that halts on a
    /// conflict never reaches the point of updating refs, so a control that
    /// conflicted would say nothing about `rebase.updateRefs` either way.
    const CONTROL_FILE: &str = "updaterefs-control.txt";

    let repo = conflicting_repo();
    repo.git(&["config", "rebase.updateRefs", "true"]);
    // Armed on purpose, and hostile on purpose. The control below exists to
    // prove `rebase.updateRefs` is live, but `--update-refs` is a merge-backend
    // feature and the apply backend ignores it outright - so a developer with
    // `rebase.backend = apply` in their global config gets a control rebase
    // that exits zero and leaves the control branch exactly where it was. The
    // assertion that follows then names a config key that was set correctly and
    // never mentions the backend that overrode it. Arming the hostile setting
    // here, inside the fixture's own `TempDir` where no concurrent run can see
    // it, turns a fragility that only some machines carry into a property this
    // suite pins on every machine.
    //
    // It stays armed for the rest of the test, and it stays armed precisely
    // because `Git::safety_config` pins `rebase.backend=merge` for every git
    // command a replay runs. The harness picks its own backend rather than
    // inheriting this one, which leaves the hostile setting as exactly what it
    // ought to be here: a developer rebase configuration sitting in the
    // repository that the replay below has to tolerate and be unaffected by.
    // The control keeps a `-c rebase.backend=merge` of its own further down,
    // because the control runs through plain git rather than through the
    // harness, so nothing pins a backend on its behalf.
    repo.git(&["config", "rebase.backend", "apply"]);

    let branch_refs = || -> Vec<(String, String)> {
        ["main", "left", "right"]
            .iter()
            .map(|name| ((*name).to_string(), repo.rev_parse(name)))
            .collect()
    };

    // Control: prove `rebase.updateRefs` is live in this fixture before proving
    // the replay is unaffected by it. Every assertion this test ends in says a
    // ref did not move - and a ref that was never at risk does not move either,
    // so a renamed config key, a git that quietly stopped honouring the setting,
    // or a config write that did nothing would leave this test green forever
    // while pinning nothing at all. The demonstration runs through plain git in
    // the fixture rather than through `gitscratch`, so nothing under test is
    // involved: it is the developer's own repository doing the exact thing the
    // guard exists to prevent.
    let pristine = branch_refs();
    repo.git(&["checkout", "-q", "-b", CONTROL_BRANCH, "main"]);
    repo.commit_file(CONTROL_FILE, "control work\n", "control work");
    let planted = repo.rev_parse(CONTROL_BRANCH);
    // Detached, the way a replay checks a branch out - and not incidentally.
    // `--update-refs` pointedly skips a branch that is checked out somewhere, so
    // a control that rebased this branch while sitting on it would move the ref
    // for the ordinary reason every rebase moves the branch it is on, and would
    // demonstrate nothing. Detaching is what makes the ref eligible, which is
    // also why the replay - which detaches for its own reasons - is exposed.
    repo.git(&["checkout", "-q", "--detach", CONTROL_BRANCH]);
    // The backend is pinned for this one command because `--update-refs` is a
    // merge-backend feature: the apply backend ignores it, and a control that
    // inherited whichever backend the fixture or the developer prefers would
    // report a dead config key rather than the backend that silenced it. A
    // control has to ask for the machinery it means to demonstrate.
    repo.git(&["-c", "rebase.backend=merge", "rebase", "left"]);
    assert_ne!(
        repo.rev_parse(CONTROL_BRANCH),
        planted,
        "`rebase.updateRefs` is not live in {}, so this test could only pass \
         vacuously; a plain rebase of a detached branch pointing into the \
         replayed range left '{CONTROL_BRANCH}' sitting at {planted}",
        repo.path().display()
    );

    // Put the fixture back exactly as it was found. The control has just moved a
    // real ref in the real repository, and the assertion this test ends in
    // cannot tell the control's damage from the replay's - so the undo is by
    // ref: `update-ref -d` deletes precisely the ref the control created, where
    // `branch -D` is a force delete aimed at a name.
    repo.checkout("main");
    let control_ref = format!("refs/heads/{CONTROL_BRANCH}");
    repo.git(&["update-ref", "-d", &control_ref]);
    assert_eq!(
        branch_refs(),
        pristine,
        "the control moved one of the branches this test measures and did not \
         put it back, so the closing assertion would blame the replay for it"
    );
    let status = repo.git(&["status", "--porcelain"]);
    assert!(
        status.is_empty(),
        "the control left the fixture's working tree changed, which is not how \
         it found it:\n{status}"
    );

    // Read only now, after the control has completely unwound: a baseline taken
    // any earlier would carry the control's own rebase into what the replay is
    // held to.
    let before = branch_refs();

    // Scoped so the scratch is torn down before the refs are re-read: teardown
    // is part of what must not move a branch.
    let conflicts = {
        let scratch = repo.scratch("main");
        replay(&scratch, "left", "main");
        // `right` onto `left` is the replay that genuinely conflicts, and the
        // replayed range is what `rebase.updateRefs` would rewrite.
        replay(&scratch, "right", "left")
    };

    // Asserting on the conflict that was resolved, so this cannot pass by having
    // quietly replayed nothing for `rebase.updateRefs` to rewrite a ref into.
    assert_eq!(
        conflicts.files(),
        Files::new(1),
        "the contested file should have conflicted"
    );
    assert!(
        conflicts.file_names().contains("shared.txt"),
        "the contested file should be named in the conflicts: {:?}",
        conflicts.file_names()
    );
    assert!(
        conflicts.hunks() > Hunks::new(0),
        "replaying a contested branch should have hunks to hand-merge"
    );

    for (name, sha) in before {
        assert_eq!(repo.rev_parse(&name), sha, "replay moved branch '{name}'");
    }
}

/// The branches worth comparing are usually the ones already checked out in
/// other worktrees - which is exactly the situation where a plain `git checkout`
/// refuses to run. A replay must detach instead.
#[test]
fn works_when_the_branches_are_checked_out_in_other_worktrees() {
    let repo = conflicting_repo();
    let _left = repo.add_worktree("left");
    let _right = repo.add_worktree("right");

    let scratch = repo.scratch("main");
    replay(&scratch, "left", "main");
    let conflicts = replay(&scratch, "right", "left");

    // Asserting on the conflict the replay had to resolve, so this cannot pass
    // by having quietly replayed nothing at all.
    assert_eq!(
        conflicts.files(),
        Files::new(1),
        "the contested file should have conflicted"
    );
    assert!(
        conflicts.file_hunks().any(|(name, _)| name == "shared.txt"),
        "the contested file should be named in the conflicts: {:?}",
        conflicts.file_hunks().collect::<Vec<_>>()
    );
    assert!(
        conflicts.hunks() > Hunks::new(0),
        "replaying a contested branch should have hunks to hand-merge"
    );
}

/// A worktree's directory can be temporarily unreachable while the worktree
/// itself is perfectly alive - an external drive unmounted, a network mount
/// asleep, a directory moved aside for a minute. Everything that makes it
/// recoverable, including any halted rebase, lives in the real repository under
/// `.git/worktrees/`. Repo-wide cleanup deletes that state on sight and with no
/// grace period, so a replay must only ever tidy up after itself.
#[test]
fn never_disturbs_other_worktrees_whose_directories_are_temporarily_missing() {
    let repo = conflicting_repo();
    let elsewhere = repo.add_worktree("left");

    let common_dir = repo
        .path()
        .join(repo.git(&["rev-parse", "--git-common-dir"]));
    let admin_dir = common_dir
        .join("worktrees")
        .join(elsewhere.file_name().expect("worktree directory name"));
    assert!(
        admin_dir.is_dir(),
        "fixture must start with worktree state that could be lost"
    );

    // Stand in for an unmounted volume: the directory is gone, but it is
    // coming back.
    let parked = elsewhere.with_file_name("parked-while-unmounted");
    std::fs::rename(&elsewhere, &parked).expect("park the worktree directory");

    // Scoped on purpose: this test exists to pin what `Scratch`'s teardown does
    // and does not do, so the drop must have run before anything below is
    // asserted. Do not flatten this block away.
    {
        let scratch = repo.scratch("main");
        replay(&scratch, "left", "main");
        replay(&scratch, "right", "left");
    }

    assert!(
        admin_dir.is_dir(),
        "replay deleted an unrelated worktree's administrative state"
    );
    let listed = repo.git(&["worktree", "list"]);
    assert!(
        listed.contains("wt-left"),
        "replay dropped an unrelated worktree from the repo:\n{listed}"
    );

    // The volume comes back.
    std::fs::rename(&parked, &elsewhere).expect("restore the worktree directory");
    // Scrubbed like every other spawn in this crate: an inherited `GIT_DIR`
    // outranks even `-C`, so this would report on the hook's repository - which
    // is alive and well - and call the restored worktree healthy without ever
    // having looked at it.
    let status = Command::new("git")
        .args([
            "-C",
            elsewhere.to_str().expect("utf-8 worktree path"),
            "status",
        ])
        .without_inherited_repository()
        .output()
        .expect("run git status in the restored worktree");
    assert!(
        status.status.success(),
        "the restored worktree is no longer a working worktree:\n{}",
        String::from_utf8_lossy(&status.stderr)
    );
}

/// `rerere` - "reuse recorded resolution" - is the one git feature that makes a
/// dry run *teach* the repository something. With it enabled, every conflict
/// git hits is filed away as a preimage in `rr-cache`, and the resolution that
/// follows is filed away beside it. That cache lives in the git common
/// directory: it is shared by every worktree, keyed only by the shape of the
/// conflict, and consulted by every later merge and rebase in the repository.
///
/// A replay resolves conflicts by staging the markers verbatim - a deliberate
/// non-answer chosen because it never discards a side. Letting rerere watch
/// that happen would record "the resolution to this conflict is a file full of
/// `<<<<<<<`" and then replay it silently the next time the developer hits the
/// same conflict for real - with `rerere.autoupdate` on, staged for them
/// without so much as a prompt. That is a simulation reaching forward in time
/// to corrupt real work, which is strictly worse than not simulating at all.
///
/// So the guarantee is not "we do not turn rerere on"; it is that a developer
/// who has already turned it on themselves - a perfectly reasonable thing to
/// have done - still gets an `rr-cache` that the replay never touched.
#[test]
fn never_records_a_rerere_preimage_even_when_rerere_is_enabled() {
    let repo = conflicting_repo();
    repo.git(&["config", "rerere.enabled", "true"]);
    // The half that stages a recalled resolution without asking, and therefore
    // the half that would do the damage invisibly.
    repo.git(&["config", "rerere.autoupdate", "true"]);
    // Armed on purpose, and hostile on purpose. The control below has to reach a
    // conflict, because a conflict is the only thing rerere ever records, and
    // `merge.ff = only` makes git refuse a diverging merge outright - "fatal:
    // Not possible to fast-forward, aborting.", exit 128, no merge ever started
    // and no preimage ever written. The assertion that guards the control only
    // asks that the merge *failed*, so it cannot tell a refusal from a conflict
    // and passes for the wrong reason; the recording assertion after it is then
    // the one that fires, and it blames rerere for a merge that never ran. A
    // developer carrying `merge.ff = only` globally - a perfectly ordinary thing
    // to carry - is the one who reads that message. Arming the setting here,
    // inside the fixture's own `TempDir` where no concurrent run can see it,
    // turns a fragility that only some machines carry into a property this suite
    // pins on every machine.
    //
    // Like the hostile backend the sibling `rebase.updateRefs` test arms, this
    // one stays standing for the rest of the test rather than being unwound with
    // the control's other damage - though the two are safe to leave armed for
    // different reasons, and the difference is worth stating. The sibling's
    // backend reaches a replay that overrides it, because `Git::safety_config`
    // pins `rebase.backend=merge`. `Git::safety_config` names no `merge.ff` at
    // all, so this setting reaches the replay unopposed and has to be harmless
    // on its own terms. It is: `merge.ff` is read by `git merge` and by nothing
    // else here - `merge --abort` ends a merge rather than starting one,
    // `gitscratch` never runs `git merge` at all, and a rebase under
    // `merge.ff = only` still conflicts exactly as it does without it - all
    // three checked rather than assumed. Leaving it armed is the stronger
    // reading anyway: the replay below then runs under a developer merge config
    // it has to tolerate.
    repo.git(&["config", "merge.ff", "only"]);

    // `rr-cache` is shared repo-wide, so it is reachable from the common dir
    // rather than from any one worktree's git dir - including the scratch's.
    let common_dir = repo
        .path()
        .join(repo.git(&["rev-parse", "--git-common-dir"]));
    let rr_cache = common_dir.join("rr-cache");
    assert!(
        !rr_cache.exists(),
        "fixture must start with nothing recorded, or this proves nothing:\n{}",
        describe_tree(&rr_cache)
    );

    // Control: prove rerere is actually recording before proving the replay does
    // not make it record. An empty `rr-cache` at the end is exactly what a git
    // that had quietly stopped honouring `rerere.enabled`, or a config write that
    // silently did nothing, would also produce - and every assertion below would
    // then pass for the wrong reason, permanently. This merge runs through plain
    // git in the fixture rather than through `gitscratch`, so nothing under test
    // is involved: it is the developer's repository behaving the way it normally
    // would. `merge` rather than `rebase` because it reaches the conflict in one
    // command and unwinds in one more.
    //
    // Not `repo.git`, which panics on a non-zero exit - conflicting is the whole
    // point of this merge, so its failure has to be inspected rather than raised.
    //
    // `--no-ff` for the same reason the sibling control pins its rebase backend:
    // a control has to ask for the merge it means to demonstrate rather than
    // inherit whichever one the fixture or the developer prefers. These two
    // branches diverge, so the merge git performs is the same one either way -
    // the flag only settles whether git agrees to perform it.
    repo.checkout("right");
    let control = Command::new("git")
        .args(["merge", "--no-ff", "left"])
        .current_dir(repo.path())
        .output()
        .expect("run the control merge in the fixture");
    assert!(
        !control.status.success(),
        "the control merge was supposed to conflict, and rerere only ever records \
         a conflict, so this test could only pass vacuously:\n{}\n{}",
        String::from_utf8_lossy(&control.stdout),
        String::from_utf8_lossy(&control.stderr)
    );
    // The assertion above asks only that the merge *failed*, and git has more
    // ways to fail a merge than to conflict one - a refused fast-forward, a
    // dirty working tree, an unknown ref - every one of which exits non-zero
    // with an empty index and no preimage. Read against that, the recording
    // assertion below is the first thing to fire, and it names rerere for a
    // merge that never ran. So the control states the shape of its failure and
    // not merely the fact of it: unmerged paths in the index are what a conflict
    // leaves behind and what no other failure produces, which keeps the blame
    // where it belongs when some future setting breaks this merge a new way.
    let unmerged = repo.git(&["diff", "--name-only", "--diff-filter=U"]);
    assert!(
        !unmerged.is_empty(),
        "the control merge failed without conflicting, so it left the index \
         clean and rerere with nothing to record; the recording assertion below \
         would blame rerere for a merge that never got that far:\n{}\n{}",
        String::from_utf8_lossy(&control.stdout),
        String::from_utf8_lossy(&control.stderr)
    );
    assert!(
        rr_cache.exists(),
        "rerere is not recording in {}, so this test could only pass vacuously; \
         a plain conflicting merge left nothing at {}",
        repo.path().display(),
        rr_cache.display()
    );

    // Put the fixture back exactly as it was found: no merge in flight, `main`
    // checked out, and - because `merge --abort` pointedly leaves the recording
    // alone - no `rr-cache` at all. The whole directory goes, not just its
    // contents: the closing assertion is `!rr_cache.exists()`, so an emptied
    // `rr-cache` left standing here would fail the test for the control's reasons
    // rather than the replay's.
    repo.git(&["merge", "--abort"]);
    repo.checkout("main");
    std::fs::remove_dir_all(&rr_cache).expect("clear the control's recording");
    assert!(
        !rr_cache.exists(),
        "the control run must leave no recording behind, or the real assertion \
         cannot tell the two apart:\n{}",
        describe_tree(&rr_cache)
    );

    // Scoped on purpose: teardown is another chance for git to flush state into
    // the real repository, so the drop must have run before the cache is
    // inspected. Do not flatten this block away.
    let conflicts = {
        let scratch = repo.scratch("main");
        replay(&scratch, "left", "main");
        // `right` onto `left` is the replay that genuinely conflicts, and a
        // conflict is the only thing rerere ever has to record.
        replay(&scratch, "right", "left")
    };

    // Asserting on the conflict that was resolved, so this cannot pass by
    // having quietly replayed nothing for rerere to learn from.
    assert_eq!(
        conflicts.files(),
        Files::new(1),
        "the contested file should have conflicted"
    );
    assert!(
        conflicts.file_names().contains("shared.txt"),
        "the contested file should be named in the conflicts: {:?}",
        conflicts.file_names()
    );
    assert!(
        conflicts.hunks() > Hunks::new(0),
        "replaying a contested branch should have hunks to hand-merge"
    );

    assert!(
        !rr_cache.exists(),
        "replay recorded rerere state in the developer's repository at {}:\n{}",
        rr_cache.display(),
        describe_tree(&rr_cache)
    );
}

/// The hooks a replay would trip if `core.hooksPath` were not redirected.
/// `pre-merge-commit` cannot fire from a rebase-only replay today; it is planted
/// anyway, because the guard is not "rebase does not fire hooks" - it is "no
/// replay fires anything" - and the merge replay a sibling tool will add must
/// inherit a test that is already watching for it.
#[cfg(unix)]
const PLANTED_HOOKS: [&str; 4] = [
    "post-checkout",
    "pre-rebase",
    "post-rewrite",
    "pre-merge-commit",
];

/// Hooks are the developer's own code, and a replay runs git in the developer's
/// own repository, so by default git would happily execute them. They are
/// written for a real workflow, not for a simulation: they sign commits, push
/// to remotes, notify chat, rewrite commit messages, regenerate lockfiles, kick
/// off builds. `post-rewrite` in particular exists precisely to react to a
/// rebase having happened - which is exactly what a replay looks like from the
/// outside, and exactly the wrong conclusion for it to draw.
///
/// So a dry run that fires hooks stops being a question and becomes an action.
/// "Tell me whether this would conflict" must not be able to post to a channel,
/// touch a remote, or start a build on the developer's machine, and it must not
/// be able to do so *invisibly* - hooks are the one part of a git operation
/// whose side effects live entirely outside git's own state, so none of the
/// other guarantees in this file would notice them.
///
/// The guarantee is therefore about the developer's *existing* hooks, installed
/// long before this crate showed up: they stay installed, they stay armed for
/// real work, and a replay simply never reaches them.
///
/// Unix-only because git ignores a hook without the executable bit, and setting
/// that bit needs Unix permissions. On a platform where the hooks cannot be
/// armed there is no way for this test to be anything but vacuous, so it does
/// not pretend to run.
#[cfg(unix)]
#[test]
fn never_fires_a_hook_from_the_developer_s_repository() {
    use std::os::unix::fs::PermissionsExt;

    let repo = conflicting_repo();

    // Hooks and their evidence both hang off the common dir: hooks because that
    // is where git looks by default in every worktree of the repo, and the
    // sentinels because the common dir is inside the fixture's `TempDir` (so
    // concurrent runs cannot collide) while being outside any working tree (so
    // a sentinel can never be mistaken for a dirty file the replay left behind).
    let common_dir = repo
        .path()
        .join(repo.git(&["rev-parse", "--git-common-dir"]));
    let hooks_dir = common_dir.join("hooks");
    let sentinels = common_dir.join("hook-sentinels");
    std::fs::create_dir_all(&hooks_dir).expect("create the repo's default hooks directory");
    std::fs::create_dir_all(&sentinels).expect("create the hook sentinel directory");

    for hook in PLANTED_HOOKS {
        let script = hooks_dir.join(hook);
        // Deliberately `exit 0`. A `pre-*` hook that failed would abort the
        // operation, and an aborted replay proves nothing about hooks - the
        // sentinel has to be the only trace, so the replay is free to run to
        // completion and still be caught.
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ntouch \"{}/{hook}\"\nexit 0\n",
                sentinels.display()
            ),
        )
        .unwrap_or_else(|e| panic!("plant the {hook} hook: {e}"));
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|e| panic!("make the {hook} hook executable: {e}"));
    }

    // Control: prove the hooks are actually armed before proving the replay
    // leaves them alone. A typo in the sentinel path or a missing executable
    // bit would make git skip them silently, and every assertion below would
    // then pass for the wrong reason. This checkout runs through the fixture's
    // own git rather than through `gitscratch`, so nothing under test is
    // involved - it is the real repository behaving normally.
    repo.checkout("left");
    repo.checkout("main");
    let control = sentinels.join("post-checkout");
    assert!(
        control.is_file(),
        "the planted hooks are not armed, so this test could only pass vacuously; \
         a plain checkout in {} left nothing at {}",
        repo.path().display(),
        control.display()
    );
    std::fs::remove_file(&control).expect("clear the control sentinel");
    assert_eq!(
        describe_tree(&sentinels),
        "",
        "the control run must leave no evidence behind, or the real assertion \
         cannot tell the two apart"
    );

    // Scoped on purpose: teardown removes the scratch worktree from the real
    // repository, which is another git operation with hooks of its own to fire,
    // so the drop must have run before the sentinels are inspected. Do not
    // flatten this block away.
    let conflicts = {
        let scratch = repo.scratch("main");
        replay(&scratch, "left", "main");
        // `right` onto `left` genuinely conflicts, so the rebase halts, resolves
        // and continues - several times more hook-triggering machinery than a
        // clean replay walks through.
        replay(&scratch, "right", "left")
    };

    // Asserting on the conflict that was resolved, so this cannot pass by
    // having quietly replayed nothing for a hook to react to.
    assert_eq!(
        conflicts.files(),
        Files::new(1),
        "the contested file should have conflicted"
    );
    assert!(
        conflicts.file_names().contains("shared.txt"),
        "the contested file should be named in the conflicts: {:?}",
        conflicts.file_names()
    );
    assert!(
        conflicts.hunks() > Hunks::new(0),
        "replaying a contested branch should have hunks to hand-merge"
    );

    assert_eq!(
        describe_tree(&sentinels),
        "",
        "replay executed the developer's hooks; each path below is a hook that fired"
    );
}

/// Half-finished work in a tracked file, deliberately full of multi-byte
/// characters. "Unchanged" has to mean byte-identical, not merely
/// line-for-line: a replay that round-tripped this file through a lossy read,
/// a re-encode, or a line-ending normalisation would look untouched to a
/// textual comparison while having quietly mangled the developer's text.
const UNCOMMITTED_EDIT: &str =
    "作業中 — do not lose this 🚧\nthe café patch, half written\nline3\n";

/// A change that exists only in the index: the shape of a developer who has
/// staged part of their work and is still deciding about the rest.
const STAGED_ONLY: &str = "staged, never committed — 保存されていない 🧪\n";

/// A file git has never heard of, which is also the file with no recovery story
/// whatsoever if something deletes it.
const UNTRACKED_ONLY: &str = "untracked scratch notes ✍️ notes-café\n";

/// The whole premise of a dry run is that a developer can ask "would this
/// conflict?" *from the middle of their own half-finished work* and get an
/// answer back without paying for it. That premise dies the instant a replay
/// reaches into their working tree or their index, because uncommitted work is
/// the least recoverable thing a repository holds. A clobbered commit is one
/// `reflog` away. A clobbered branch is one `reset --hard` away. A working tree
/// git has overwritten, or an index git has reset, is simply gone - there is no
/// reflog for a working tree, and no amount of expertise gets it back.
///
/// The dirty tree is not the awkward edge case here, it is *the* case. Nobody
/// runs "would this rebase hurt?" from a pristine checkout; they run it because
/// they are elbow-deep in a branch and trying to decide what to do next. So the
/// three kinds of uncommitted state this test builds - a tracked file edited but
/// not committed, a change staged but not committed, and a file git has never
/// seen - are the normal condition of the repository a replay will be pointed
/// at, and all three have to survive it untouched.
///
/// The contested file is the one that gets dirtied on purpose: `shared.txt` is
/// exactly the file both replayed branches rewrite, so it is the file a replay
/// that escaped its scratch worktree would check out over, merge into, and stage
/// conflict markers into. If any of that leaked into the real repository, this
/// is where it would land first.
#[test]
fn never_touches_the_real_working_tree_or_index() {
    let repo = conflicting_repo();

    // Everything below is relative to the fixture's own `TempDir`, so a
    // concurrent run of this same test cannot share a path with this one.
    let dirty_tracked = repo.path().join("shared.txt");
    std::fs::write(&dirty_tracked, UNCOMMITTED_EDIT).expect("leave uncommitted work in the tree");
    std::fs::write(repo.path().join("staged.txt"), STAGED_ONLY).expect("write the staged file");
    repo.git(&["add", "staged.txt"]);
    let untracked = repo.path().join("untracked.txt");
    std::fs::write(&untracked, UNTRACKED_ONLY).expect("write the untracked file");

    // Bytes, not `String`s: these snapshots are the evidence, so they must not be
    // taken through any decoding step that could hide a difference.
    //
    // Two of the three files get a byte-compare, and it takes two because the
    // porcelain status below cannot stand in for either. A tracked file a replay
    // rewrote is still reported ` M` - it was already modified and it stays
    // modified - and an untracked file is reported `?? untracked.txt` whatever is
    // inside it, so its status line comes back byte-identical from a replay that
    // rewrote every byte of the file. That makes the one file with no recovery
    // story whatsoever the one file nothing else here can speak for, which is
    // exactly backwards, so it is snapshotted too.
    //
    // `staged.txt` is deliberately not snapshotted, because it is the one of the
    // three porcelain genuinely does cover: rewriting its working-tree copy flips
    // its status from `A ` to `AM`, and what its index copy holds is covered by
    // `diff --cached`. A third byte-compare here would assert nothing the two
    // status assertions below do not already assert.
    let tracked_before_bytes =
        std::fs::read(&dirty_tracked).expect("snapshot the uncommitted work");
    let untracked_before_bytes = std::fs::read(&untracked).expect("snapshot the untracked file");
    let before_status = repo.git(&["status", "--porcelain"]);
    let before_index = repo.git(&["diff", "--cached"]);
    let before_head = repo.rev_parse("HEAD");
    let before_branch = repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]);
    let before_refs: Vec<(String, String)> = ["main", "left", "right"]
        .iter()
        .map(|name| ((*name).to_string(), repo.rev_parse(name)))
        .collect();

    // Control: if the fixture were somehow clean, every assertion below would
    // pass by having nothing to lose.
    assert!(
        !before_status.is_empty(),
        "the repository must start dirty, or this test proves nothing"
    );
    assert!(
        !before_index.is_empty(),
        "the index must start carrying a staged change, or nothing below covers it"
    );
    assert_eq!(
        before_branch, "main",
        "the fixture must start on a branch, so a stray detach is visible"
    );

    // Scoped on purpose: teardown removes the scratch worktree from the real
    // repository, and a removal that resolved to the wrong path is exactly the
    // failure that would eat the developer's work. The drop must have run
    // before anything below is read. Do not flatten this block away.
    let conflicts = {
        let scratch = repo.scratch("main");
        replay(&scratch, "left", "main");
        // `right` onto `left` genuinely conflicts, so the replay checks out,
        // merges, writes conflict markers and stages them - the full set of
        // operations that rewrite a working tree and an index.
        replay(&scratch, "right", "left")
    };

    // Asserting on the conflict that was resolved, so this cannot pass by
    // having quietly replayed nothing at all into anywhere at all.
    assert_eq!(
        conflicts.files(),
        Files::new(1),
        "the contested file should have conflicted"
    );
    assert!(
        conflicts.file_names().contains("shared.txt"),
        "the contested file should be named in the conflicts: {:?}",
        conflicts.file_names()
    );
    assert!(
        conflicts.hunks() > Hunks::new(0),
        "replaying a contested branch should have hunks to hand-merge"
    );

    let tracked_after_bytes = std::fs::read(&dirty_tracked).expect("re-read the uncommitted work");
    assert_eq!(
        tracked_after_bytes,
        tracked_before_bytes,
        "replay rewrote the developer's uncommitted work in {}\n  before: {}\n   after: {}",
        dirty_tracked.display(),
        String::from_utf8_lossy(&tracked_before_bytes),
        String::from_utf8_lossy(&tracked_after_bytes),
    );
    let untracked_after_bytes = std::fs::read(&untracked).expect("re-read the untracked file");
    assert_eq!(
        untracked_after_bytes,
        untracked_before_bytes,
        "replay rewrote a file git has never heard of, and no reflog, stash or \
         branch gets it back: {}\n  before: {}\n   after: {}",
        untracked.display(),
        String::from_utf8_lossy(&untracked_before_bytes),
        String::from_utf8_lossy(&untracked_after_bytes),
    );
    assert_eq!(
        repo.git(&["status", "--porcelain"]),
        before_status,
        "replay changed what the developer's working tree and index contain"
    );
    assert_eq!(
        repo.git(&["diff", "--cached"]),
        before_index,
        "replay changed what the developer had staged"
    );
    assert_eq!(
        repo.rev_parse("HEAD"),
        before_head,
        "replay moved the developer's HEAD"
    );
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]),
        before_branch,
        "replay left the developer's HEAD somewhere other than the branch they were on"
    );
    for (name, sha) in before_refs {
        assert_eq!(repo.rev_parse(&name), sha, "replay moved branch '{name}'");
    }

    // A halted rebase left behind in the real repository is its own kind of
    // damage: every later git command in that repo refuses or behaves oddly
    // until someone runs `rebase --abort`, and the developer has no reason to
    // suspect a dry run put it there.
    let common_dir = repo
        .path()
        .join(repo.git(&["rev-parse", "--git-common-dir"]));
    for state_dir in ["rebase-merge", "rebase-apply"] {
        let path = common_dir.join(state_dir);
        assert!(
            !path.exists(),
            "replay left the developer's repository mid-rebase at {}:\n{}",
            path.display(),
            describe_tree(&path)
        );
    }
}

/// Adding a scratch worktree is not a purely local act. `git worktree add`
/// writes administrative state into the *developer's* repository - a directory
/// under `.git/worktrees/` holding that worktree's HEAD, its index, and any
/// operation it is in the middle of - and registers an entry that
/// `git worktree list` reports from then on. Deleting the worktree's directory
/// undoes none of that. And the directory here is a `TempDir` that deletes
/// itself unconditionally, so a harness that forgot to deregister produces
/// exactly the failure nobody goes looking for: no leftover files, no error
/// message, and a repository quietly accumulating one dead worktree per dry run.
///
/// The consequences all land on the developer rather than on the harness. A
/// stale entry keeps its recorded HEAD reachable, so the commits a replay
/// created stay pinned against collection forever. It makes the developer's own
/// `git worktree add` refuse a path that git believes is still taken, over a
/// directory that has not existed since the run that made it. And it puts
/// worktrees the developer never created into `git worktree list`, where they
/// have to be understood before they can be dismissed.
///
/// Then comes the part that makes this the sharpest guarantee in the file. The
/// remedy git offers for a stale entry - the one every answer on the subject
/// reaches for - is `git worktree prune`. Pruning is precisely the repo-wide,
/// no-grace-period operation this crate refuses to run, because it also deletes
/// the administrative state of every *healthy* worktree whose directory is
/// merely unreachable right now: an unmounted drive, a sleeping network mount,
/// a directory moved aside for a minute, together with any halted rebase inside
/// it. That is the guarantee the test above this one pins. So a leak here would
/// not merely leave litter behind - it would hand the developer a mess whose
/// obvious fix is the destructive command the harness goes out of its way to
/// protect them from. The two guarantees are the same guarantee seen from
/// opposite ends, and this crate has to hold both or neither.
///
/// The three scopes below are the three ways a `Scratch` reaches its `Drop`,
/// hardest last: a clean replay, a replay that had to resolve a genuine
/// conflict, and a scratch dropped while a rebase is still halted mid-flight.
/// The last one is the one most likely to leak, because a worktree git
/// considers busy is exactly the worktree a cautious removal would decline to
/// take - and it is also the state a consumer reaches for real the moment a
/// replay errors out partway through and unwinds.
#[test]
fn never_leaves_a_scratch_worktree_registered_in_the_real_repository() {
    let repo = conflicting_repo();
    // Armed on purpose, and hostile on purpose. The third block below halts a
    // rebase inside the scratch and then asserts that git really is sitting on
    // rebase state, at the `rebase-merge` path the merge backend writes. The
    // apply backend writes `rebase-apply` instead, so a developer carrying
    // `rebase.backend = apply` in their global config watches that assertion
    // fire over a rebase that halted exactly as it was asked to, reading a
    // message that names a path and never mentions the backend that moved it.
    // Which backend a replay runs on is the harness's business rather than the
    // developer's - it is the harness that decides where a consumer inspecting
    // a halted replay has to look - so arming the hostile setting here, inside
    // the fixture's own `TempDir` where no concurrent run can see it, turns a
    // fragility that only some machines carry into a property this suite pins
    // on every machine.
    repo.git(&["config", "rebase.backend", "apply"]);

    // `worktrees/` under the common dir is where `git worktree add` files the
    // administrative state that outlives the worktree's directory. Everything
    // here hangs off the fixture's own `TempDir`, so a concurrent run of this
    // same test never shares a path with this one.
    let common_dir = repo
        .path()
        .join(repo.git(&["rev-parse", "--git-common-dir"]));
    let worktrees_dir = common_dir.join("worktrees");

    // Anchored to what the fixture actually starts with rather than to a
    // hardcoded listing: the claim being pinned is "the replay added nothing",
    // which only means something if "nothing" is measured against the real
    // starting state.
    let before = repo.git(&["worktree", "list"]);
    assert_eq!(
        before.lines().count(),
        1,
        "the fixture must start with only the real repository registered, \
         or a leak has somewhere to hide:\n{before}"
    );
    assert_eq!(
        describe_tree(&worktrees_dir),
        "",
        "the fixture must start with no worktree administrative state, \
         or the assertions below cannot tell new state from old"
    );

    let assert_nothing_registered = |stage: &str| {
        let listed = repo.git(&["worktree", "list"]);
        assert_eq!(
            listed, before,
            "after {stage}, the real repository's worktree list changed\n  \
             before: {before}\n   after: {listed}"
        );
        // Called out separately from the equality above because `prunable` is
        // the specific shape a leak takes: the entry survives, the directory
        // does not, and git starts advertising that a prune would tidy it up.
        assert!(
            !listed.contains("prunable"),
            "after {stage}, the real repository has a worktree entry git wants \
             pruned:\n{listed}"
        );
        assert_eq!(
            describe_tree(&worktrees_dir),
            "",
            "after {stage}, worktree administrative state is still filed in the \
             developer's repository under {}",
            worktrees_dir.display()
        );
    };

    // Scoped on purpose: `Drop` is the entire subject of this test, so it must
    // have run before anything is asserted. Do not flatten these blocks away.
    {
        let scratch = repo.scratch("main");

        // Control: prove the harness really does register a worktree in the
        // real repository while the `Scratch` is alive. Without it, a `Scratch`
        // that quietly registered nothing at all would satisfy every assertion
        // below by never having had anything to clean up.
        let while_alive = repo.git(&["worktree", "list"]);
        assert_eq!(
            while_alive.lines().count(),
            2,
            "a live scratch should be registered in the real repository, \
             or this test can only pass vacuously:\n{while_alive}"
        );
        assert_ne!(
            describe_tree(&worktrees_dir),
            "",
            "a live scratch should have administrative state under {}, \
             or this test can only pass vacuously",
            worktrees_dir.display()
        );

        replay(&scratch, "left", "main");
    }
    assert_nothing_registered("a clean replay");

    {
        let scratch = repo.scratch("main");
        replay(&scratch, "left", "main");
        // `right` onto `left` genuinely conflicts, so this scratch halts,
        // resolves and continues a rebase before being dropped - it reaches
        // teardown having done real work, with a rewritten index and a rewritten
        // working tree behind it.
        let conflicts = replay(&scratch, "right", "left");

        // Asserting on the conflict that was resolved, so this block cannot
        // pass by having quietly replayed nothing.
        assert_eq!(
            conflicts.files(),
            Files::new(1),
            "the contested file should have conflicted"
        );
        assert!(
            conflicts.file_names().contains("shared.txt"),
            "the contested file should be named in the conflicts: {:?}",
            conflicts.file_names()
        );
        assert!(
            conflicts.hunks() > Hunks::new(0),
            "replaying a contested branch should have hunks to hand-merge"
        );
    }
    assert_nothing_registered("a replay that had to resolve a conflict");

    {
        let scratch = repo.scratch("main");
        let git = scratch.git();
        git.run(&["checkout", "-q", "--detach", "right"])
            .expect("check out the branch detached in the scratch worktree");

        // Deliberately not resolved. `try_run` hands back the failure instead of
        // raising it, which is the only way to leave the scratch sitting in a
        // halted rebase and then drop it - the shape a consumer hits whenever a
        // replay gives up partway through and unwinds.
        let halted = git
            .try_run(&["rebase", "left"])
            .expect("run the rebase that conflicts");
        assert!(
            !halted.success,
            "the rebase was supposed to conflict and halt, so this block cannot \
             pin teardown-from-mid-rebase unless it did:\n{}\n{}",
            halted.stdout, halted.stderr
        );

        // And prove the halt is real rather than merely a non-zero exit: git is
        // sitting on rebase state, in the worktree that is about to be dropped.
        let state = git
            .run(&["rev-parse", "--git-path", "rebase-merge"])
            .expect("locate the scratch worktree's rebase state");
        let state = scratch.path().join(state);
        assert!(
            state.exists(),
            "the scratch should be mid-rebase at {} when it is dropped",
            state.display()
        );
    }
    assert_nothing_registered("a scratch dropped while a rebase was still halted");
}

/// Commit signing is the one git setting people turn on once, globally, and
/// then forget about - which means it applies to every repository the developer
/// owns, including the one a dry run is about to be pointed at. A replay commits
/// for real inside its scratch worktree: every resolved conflict ends in
/// `rebase --continue`, and `rebase --continue` writes a commit object. If the
/// replay inherits the developer's signing configuration, that commit gets sent
/// to their signing program.
///
/// Both ways that can go are unacceptable, and they are unacceptable for
/// different reasons. The good case is that signing simply fails - a key that is
/// not on this machine, a smartcard that is not plugged in, an agent that is not
/// running - and the developer gets no answer at all to a question that was
/// supposed to be free. The bad case is that signing *works*, and the developer
/// is handed a passphrase prompt, or a pinentry dialog, or a smartcard touch
/// request that they never asked for and cannot connect to anything they did.
/// A dry run is a question, not an action, and it must not sit there waiting on
/// a human. A hang is strictly worse than a failure: a failure tells you what
/// happened and hands the terminal back, while a hang wedges the calling tool
/// with no diagnosis and no obvious culprit.
///
/// So this test has to be able to tell those two apart, which is why the replay
/// runs on its own thread behind a timeout instead of being called directly. A
/// plain call would catch only the failure; the hang - the outcome that actually
/// matters more - would take the whole test binary down with it and report
/// nothing about why.
#[test]
fn replays_without_hanging_or_failing_when_commit_signing_is_enabled() {
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    /// Long enough that a loaded machine running several `cargo test`
    /// invocations at once never trips it by being slow, short enough that a
    /// genuinely stuck replay is reported in about a minute instead of hanging
    /// the suite until someone notices. The replay it bounds takes well under a
    /// second, so the gap between "slow" and "stuck" is not a close call.
    const REPLAY_TIMEOUT: Duration = Duration::from_secs(60);

    /// A key id no keyring can resolve. This is what makes the guarantee
    /// testable on *any* machine rather than only on one without a working gpg:
    /// with a real key, whether an attempted signature succeeds would depend on
    /// whether the developer running the tests happens to have an unlocked agent
    /// sitting there, so "the replay tried to sign" would be observable on some
    /// machines and invisible on others. With a key that cannot resolve,
    /// attempting to sign is deterministically fatal everywhere.
    const UNRESOLVABLE_SIGNING_KEY: &str = "0xDEADBEEFDEADBEEF";

    /// Drive both replays and hand back what the contested one cost, reporting
    /// failure instead of raising it.
    ///
    /// The shared `replay` helper at the top of this file `expect`s, which is
    /// right for every other test here and wrong for this one: a panic on this
    /// thread would reach the main thread only as a dropped channel, stripped of
    /// the git output that says *why* - and "why" is the entire subject of this
    /// test. So this is a local, non-panicking twin. Do not merge the two; the
    /// other tests want the panic.
    fn replay_under_signing(repo: &Path) -> anyhow::Result<Conflicts> {
        let scratch = Repo::open(repo)?.scratch("main")?;

        let git = scratch.git();
        git.run(&["checkout", "-q", "--detach", "left"])?;
        scratch.replay_rebase("main")?;

        // The replay that matters. `right` onto `left` genuinely conflicts, so
        // the rebase halts, the markers get staged, and the replay finishes the
        // commit with `rebase --continue` - which is the exact moment a signing
        // configuration would be consulted.
        git.run(&["checkout", "-q", "--detach", "right"])?;
        let conflicts = scratch.replay_rebase("left")?;

        // A signing failure does not have to arrive as an error, and that is the
        // subtlest way this guarantee could rot. When `rebase --continue` cannot
        // write the commit, git leaves the rebase halted with nothing unmerged
        // left in the index - which the resolution loop reads as "a commit that
        // became empty" and answers with `rebase --skip`. The rebase then
        // finishes successfully, having thrown away the very commit it was
        // asked to replay, and the caller gets a plausible-looking cost for work
        // that was never actually done. So "no error" is not enough: the
        // replayed commit has to still be there.
        let replayed = git.run(&["log", "--format=%s", "left..HEAD"])?;
        anyhow::ensure!(
            replayed.contains("right work"),
            "the replay finished without the commit it was replaying - \
             `git log --format=%s left..HEAD` in the scratch worktree reported \
             {replayed:?}. Signing broke the commit and the resolution loop \
             skipped it, so the failure came back disguised as an answer."
        );

        Ok(conflicts)
    }

    let repo = conflicting_repo();

    // Signing has to be switched on *after* the fixture is built: `TestRepo`
    // pins `commit.gpgsign=false` while creating the commits, precisely so a
    // developer whose global config signs everything can still build fixtures.
    repo.git(&["config", "commit.gpgsign", "true"]);
    repo.git(&["config", "user.signingkey", UNRESOLVABLE_SIGNING_KEY]);
    // Pinned here as well as in the harness, so the fixture stays deterministic
    // on its own terms: `gpg.format` selects *which* program config git reads
    // (`gpg.program`, `gpg.ssh.program`, `gpg.x509.program`), so leaving it to
    // whatever the developer running these tests has set globally would leave
    // the signing program below unused on their machine.
    repo.git(&["config", "gpg.format", "openpgp"]);
    // A signing program that does not exist, at a path inside this fixture's own
    // `TempDir` so concurrent runs cannot share it. Naming a real gpg would drag
    // the developer's actual `~/.gnupg` into a test run - creating it, locking
    // it, starting an agent - and would put a pinentry prompt one misconfigured
    // machine away from wedging the suite for real. A path that cannot be
    // executed makes "the replay tried to sign" fail instantly, identically,
    // everywhere, and with nothing outside the fixture involved.
    let signing_program = repo.path().join("no-such-signing-program");
    repo.git(&[
        "config",
        "gpg.program",
        signing_program.to_str().expect("utf-8 fixture path"),
    ]);

    // Control: prove the signing configuration above is actually armed before
    // proving the replay is unaffected by it. A typo in a config key, or a git
    // that ignored the setting, would leave every assertion below passing
    // because nothing was ever asked to sign. This commit runs through plain
    // git rather than through `gitscratch`, so nothing under test is involved -
    // it is the developer's repository behaving the way it normally would. It
    // is `--allow-empty` so that failing, which is what it is here to do, leaves
    // the fixture exactly as it found it.
    //
    // The locale is pinned because the assertion below matches git's own words.
    // "gpg failed to sign the data" is wrapped in gettext, so a git built with
    // NLS - Homebrew's is - answers a developer running under, say, `de_DE` with
    // "gpg konnte die Daten nicht signieren", and this control then fails for a
    // reason that has nothing to do with signing. `LC_ALL` is the one that
    // decides; `LANG` is set alongside it because it costs nothing and spares
    // the next reader from having to remember the precedence.
    let control = Command::new("git")
        .args(["commit", "--allow-empty", "-q", "-m", "control"])
        .current_dir(repo.path())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .output()
        .expect("run the control commit in the fixture");
    let control_stderr = String::from_utf8_lossy(&control.stderr);
    assert!(
        !control.status.success(),
        "commit signing is not armed in {}, so this test could only pass \
         vacuously: a plain commit succeeded",
        repo.path().display()
    );
    assert!(
        control_stderr.contains("gpg failed to sign"),
        "the control commit failed for some reason other than signing, so the \
         fixture is not testing what it claims to:\n{control_stderr}"
    );

    // The replay goes on its own thread so the main thread keeps a clock on it.
    // Only the path is moved across; `repo` stays here, alive, because its
    // `TempDir` is the repository the thread is working in.
    let repo_path = repo.path().to_path_buf();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        // The receiver is gone if the main thread already gave up waiting, and
        // that is a normal end to this thread rather than an error.
        let _ = sender.send(replay_under_signing(&repo_path));
    });

    // Three outcomes, three different bugs, three different messages. Collapsing
    // them would report the wrong one.
    let conflicts = match receiver.recv_timeout(REPLAY_TIMEOUT) {
        Ok(Ok(conflicts)) => conflicts,
        Ok(Err(error)) => panic!(
            "commit signing broke the replay: a developer with signing enabled \
             cannot get a trustworthy answer out of a dry run\n{error:?}"
        ),
        Err(RecvTimeoutError::Timeout) => panic!(
            "the replay never came back after {REPLAY_TIMEOUT:?} - a dry run \
             that inherited commit signing is sitting on a passphrase prompt \
             nobody asked for"
        ),
        Err(RecvTimeoutError::Disconnected) => panic!(
            "the replay panicked instead of returning; see the thread's own \
             panic message above for what commit signing did to it"
        ),
    };

    // Asserting on the conflict that was resolved, so this cannot pass by having
    // quietly replayed nothing for signing to have been consulted about.
    assert_eq!(
        conflicts.files(),
        Files::new(1),
        "the contested file should have conflicted"
    );
    assert!(
        conflicts.file_names().contains("shared.txt"),
        "the contested file should be named in the conflicts: {:?}",
        conflicts.file_names()
    );
    assert!(
        conflicts.hunks() > Hunks::new(0),
        "replaying a contested branch should have hunks to hand-merge"
    );
}
