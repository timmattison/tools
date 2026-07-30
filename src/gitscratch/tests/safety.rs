//! `gitscratch` runs against the developer's real repository. These tests pin
//! the properties that make that acceptable.

use std::path::Path;
use std::process::Command;

use gitscratch::testing::conflicting_repo;
use gitscratch::{Conflicts, Files, Hunks, Scratch};

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
            if child.is_dir() {
                pending.push(child.clone());
            }
            found.push(child.display().to_string());
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
    let repo = conflicting_repo();
    repo.git(&["config", "rebase.updateRefs", "true"]);

    let before: Vec<(String, String)> = ["main", "left", "right"]
        .iter()
        .map(|name| ((*name).to_string(), repo.rev_parse(name)))
        .collect();

    // Scoped so the scratch is torn down before the refs are re-read: teardown
    // is part of what must not move a branch.
    {
        let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
        replay(&scratch, "left", "main");
        // `right` onto `left` is the replay that genuinely conflicts, and the
        // replayed range is what `rebase.updateRefs` would rewrite.
        replay(&scratch, "right", "left");
    }

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

    let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
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
        conflicts.file_names().contains("shared.txt"),
        "the contested file should be named in the conflicts: {:?}",
        conflicts.file_names()
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
        let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
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
    let status = Command::new("git")
        .args([
            "-C",
            elsewhere.to_str().expect("utf-8 worktree path"),
            "status",
        ])
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

    // Scoped on purpose: teardown is another chance for git to flush state into
    // the real repository, so the drop must have run before the cache is
    // inspected. Do not flatten this block away.
    let conflicts = {
        let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
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
        let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
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
    std::fs::write(repo.path().join("untracked.txt"), UNTRACKED_ONLY)
        .expect("write the untracked file");

    // Bytes, not a `String`: this snapshot is the evidence, so it must not be
    // taken through any decoding step that could hide a difference.
    let before_bytes = std::fs::read(&dirty_tracked).expect("snapshot the uncommitted work");
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
        let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
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

    let after_bytes = std::fs::read(&dirty_tracked).expect("re-read the uncommitted work");
    assert_eq!(
        after_bytes,
        before_bytes,
        "replay rewrote the developer's uncommitted work in {}\n  before: {}\n   after: {}",
        dirty_tracked.display(),
        String::from_utf8_lossy(&before_bytes),
        String::from_utf8_lossy(&after_bytes),
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
