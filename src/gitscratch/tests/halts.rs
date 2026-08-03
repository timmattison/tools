//! What the replay does at each kind of rebase halt.
//!
//! A rebase that stops with nothing unmerged has not told you why. It might be
//! a commit that adds nothing to the new base, which costs nothing to drop —
//! or a commit git could not write, which costs the developer their work if it
//! is dropped and reported as a cheap replay. These tests put a replay in each
//! of those states for real and pin the answer it gives — refuse to answer for
//! the commit that could not be written, drop the one that really is empty and
//! carry on. Both directions need pinning: a guard against the first that also
//! rejects the second would fail every replay of a branch main has caught up
//! with.
//!
//! The unwritable states are reached by making the object database unwritable,
//! which is a Unix permission trick, so the whole suite is Unix-only.
#![cfg(unix)]

use gitscratch::testing::{
    branches_behind_main_repo, branches_behind_main_with_quoted_and_space_led_paths_repo,
    commit_emptied_by_main_repo, modify_delete_repo,
};
use gitscratch::{Files, Hunks, Scratch, Stops};

/// Whether git is sitting in a halted rebase, asked the way the replay asks it:
/// `rev-parse --git-path` resolves the state directory for whichever worktree it
/// runs in, which for a linked worktree is nowhere near the repository's own
/// `.git`. Both backends are checked because the replay checks both.
///
/// # Panics
///
/// Panics if git cannot say where the state directory would live.
fn rebase_in_progress(scratch: &Scratch) -> bool {
    ["rebase-merge", "rebase-apply"]
        .into_iter()
        .any(|state_dir| {
            let path = scratch
                .git()
                .run(&["rev-parse", "--git-path", state_dir])
                .expect("ask git where the rebase state directory would be");
            scratch.path().join(path).exists()
        })
}

/// The replay's whole job is to say what an operation would cost. A commit git
/// could not write costs nothing to `--skip`, and the rebase then finishes
/// happily having thrown that commit away — so the replay would report a
/// plausible number for a branch it never actually replayed. It must refuse to
/// answer instead, and it must say enough for someone to fix it: which commit
/// was about to be dropped, and what git itself said.
#[test]
fn refuses_to_report_a_cost_when_a_staged_resolution_could_not_be_committed() {
    let repo = modify_delete_repo();
    // Read the abbreviation from the same object database the implementation
    // will abbreviate against, so `%h` here and `%h` there agree.
    let dropped_sha = repo.git(&["log", "-1", "--format=%h", "branch"]);
    let dropped_subject = repo.git(&["log", "-1", "--format=%s", "branch"]);
    let objects = std::fs::canonicalize(repo.path().join(".git").join("objects"))
        .expect("canonicalize the object database path");

    let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
    scratch
        .git()
        .run(&["checkout", "-q", "--detach", "branch"])
        .expect("check out the branch detached in the scratch worktree");

    // Sealed only now: adding the worktree and checking out write no objects,
    // but everything before this point would fail against a read-only store.
    let sealed = repo.seal_object_store();
    let result = scratch.replay_rebase("main");
    // Released before a single assertion runs, so a failing one cannot leave a
    // read-only directory behind for the temporary directory to trip over.
    drop(sealed);

    let error = match result {
        Ok(conflicts) => panic!(
            "the replay reported a cost for a commit git never wrote: {conflicts:?}\n\
             a dry run may answer 'expensive' or 'I cannot answer', never 'cheap'"
        ),
        Err(error) => format!("{error:#}"),
    };

    assert!(
        error.contains(&dropped_sha),
        "the error should name the commit that was about to be dropped ({dropped_sha}): {error}"
    );
    assert!(
        error.contains(&dropped_subject),
        "the error should carry the dropped commit's subject ({dropped_subject}): {error}"
    );

    // git says: "error: insufficient permission for adding an object to
    // repository database <path>". The path is interpolated rather than
    // translated, so matching on it is locale-independent - and it is the
    // canonicalized one because macOS resolves a temp dir's /var/... to
    // /private/var/....
    let objects = objects.display().to_string();
    assert!(
        error.contains(&objects),
        "the error should carry git's own message, which names {objects}: {error}"
    );
}

/// The same silent skip, in the shape no amount of looking for uncommitted
/// content can see. When a *clean* pick fails to write its commit, git rolls the
/// index back and reschedules the pick: the index matches HEAD, the worktree
/// matches the index, and there is nothing dirty anywhere to find. The halt is
/// byte-for-byte the one a genuinely empty commit produces. What still separates
/// them is the commit itself — its work is nowhere in the new base — and the
/// replay has to refuse on that basis alone.
#[test]
fn refuses_to_report_a_cost_when_a_clean_pick_could_not_be_committed() {
    let repo = branches_behind_main_repo();
    // Read the abbreviation from the same object database the implementation
    // will abbreviate against, so `%h` here and `%h` there agree.
    let dropped_sha = repo.git(&["log", "-1", "--format=%h", "alpha"]);
    let dropped_subject = repo.git(&["log", "-1", "--format=%s", "alpha"]);
    let objects = std::fs::canonicalize(repo.path().join(".git").join("objects"))
        .expect("canonicalize the object database path");

    let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
    scratch
        .git()
        .run(&["checkout", "-q", "--detach", "alpha"])
        .expect("check out alpha detached in the scratch worktree");

    // Sealed only now: adding the worktree and checking out write no objects,
    // but everything before this point would fail against a read-only store.
    let sealed = repo.seal_object_store();
    let result = scratch.replay_rebase("main");
    // Released before a single assertion runs, so a failing one cannot leave a
    // read-only directory behind for the temporary directory to trip over.
    drop(sealed);

    let error = match result {
        Ok(conflicts) => panic!(
            "the replay reported a cost for a commit git never wrote: {conflicts:?}\n\
             a dry run may answer 'expensive' or 'I cannot answer', never 'cheap'"
        ),
        Err(error) => format!("{error:#}"),
    };

    assert!(
        error.contains(&dropped_sha),
        "the error should name the commit that was about to be dropped ({dropped_sha}): {error}"
    );
    assert!(
        error.contains(&dropped_subject),
        "the error should carry the dropped commit's subject ({dropped_subject}): {error}"
    );

    // git says: "error: insufficient permission for adding an object to
    // repository database <path>". The path is interpolated rather than
    // translated, so matching on it is locale-independent - and it is the
    // canonicalized one because macOS resolves a temp dir's /var/... to
    // /private/var/....
    let objects = objects.display().to_string();
    assert!(
        error.contains(&objects),
        "the error should carry git's own message, which names {objects}: {error}"
    );

    // The two assertions below are what keep this test about *this* shape. The
    // rollback left the repository pristine, so there is no uncommitted content
    // for the other probe to find; the only thing that proves the commit was
    // lost is that alpha.txt's change is absent from the new base. If the
    // evidence ever came back phrased as leftover uncommitted content, this
    // test would silently have become a second copy of the one above.
    assert!(
        error.contains("alpha.txt"),
        "the error should name the file whose change would have been lost: {error}"
    );
    assert!(
        !error.contains("uncommitted"),
        "nothing was left uncommitted - git rolled the index back - so the evidence must come \
         from the commit's content being absent from the new base: {error}"
    );
}

/// The same clean-pick failure again, in the one shape that makes the probe
/// answer backwards instead of not answering at all.
///
/// The probe asks git which paths the stopped commit touched and then asks
/// whether the new base already holds that commit's content *at those paths*, so
/// the paths make a round trip: out of one invocation as output, back into the
/// next as pathspecs. Git does not spell a path the same way in both directions.
/// It C-quotes a non-ASCII name into `"caf\303\251.txt"` when it prints one per
/// line, and a leading space survives git only to be eaten by anything that
/// trims the line — and it dequotes neither on the way back in. A pathspec that
/// no longer names the file matches nothing, so a commit whose work is nowhere
/// in the new base reads as a commit that adds nothing to it — and the replay
/// reaches for `rebase --skip`, which is how the work gets thrown away and a
/// cost of zero gets reported for a branch that was never replayed.
///
/// So the assertions below are about the *classification*, not merely about
/// getting an error out. A sealed object database happens to refuse the skip too,
/// so the replay stops either way; what it stops and says is the whole
/// difference between naming the two files whose work is at stake and blaming a
/// skip for a commit it has already mislabelled as empty.
///
/// Hence a fixture whose commit touches no plainly-spelled path at all: one
/// ordinary file alongside would come back matching and carry the refusal on its
/// own, leaving the mangled names' silence invisible.
#[test]
fn refuses_to_report_a_cost_when_a_clean_pick_of_quoted_paths_could_not_be_committed() {
    let repo = branches_behind_main_with_quoted_and_space_led_paths_repo();
    // Read the abbreviation from the same object database the implementation
    // will abbreviate against, so `%h` here and `%h` there agree.
    let dropped_sha = repo.git(&["log", "-1", "--format=%h", "branch"]);
    let dropped_subject = repo.git(&["log", "-1", "--format=%s", "branch"]);

    let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
    scratch
        .git()
        .run(&["checkout", "-q", "--detach", "branch"])
        .expect("check out the branch detached in the scratch worktree");

    // Sealed only now: adding the worktree and checking out write no objects,
    // but everything before this point would fail against a read-only store.
    let sealed = repo.seal_object_store();
    let result = scratch.replay_rebase("main");
    // Released before a single assertion runs, so a failing one cannot leave a
    // read-only directory behind for the temporary directory to trip over.
    drop(sealed);

    let error = match result {
        Ok(conflicts) => panic!(
            "the replay reported a cost for a commit git never wrote: {conflicts:?}\n\
             a path git quotes is still a path whose work would be thrown away"
        ),
        Err(error) => format!("{error:#}"),
    };

    assert!(
        error.contains(&dropped_sha),
        "the error should name the commit that was about to be dropped ({dropped_sha}): {error}"
    );
    assert!(
        error.contains(&dropped_subject),
        "the error should carry the dropped commit's subject ({dropped_subject}): {error}"
    );

    // The classification itself, pinned separately from the fact that something
    // went wrong. This commit adds two files the new base has never seen, so
    // calling it a commit that adds nothing to the new base is simply false - and
    // it is the false half, not the failed skip that follows from it, that would
    // cost a developer their work in a repository where the skip succeeds.
    assert!(
        !error.contains("adds nothing to the new base"),
        "a commit whose files are absent from the new base is not an empty commit, whatever \
         spelling git reported its paths in: {error}"
    );

    // Both names, in the spelling the developer gave them. Whatever the replay
    // shows a human has to be findable in their own repository, and the C-quoted
    // form is not that - it is the artefact of having read git's output the wrong
    // way, so seeing it here would mean the round trip is still broken and the
    // refusal above happened for some other reason.
    assert!(
        error.contains("café.txt"),
        "the error should name the quoted file whose change would have been lost, as it is \
         actually spelled: {error}"
    );
    assert!(
        error.contains(" leading space.txt"),
        "the error should name the space-led file whose change would have been lost, with its \
         leading space intact: {error}"
    );
    assert!(
        !error.contains("caf\\303\\251"),
        "the error should carry the file's real name, not git's C-quoted rendering of it: {error}"
    );

    // The rollback left the repository pristine, so there is no uncommitted
    // content for the other probe to find; the only thing that proves the commit
    // was lost is that its changes are absent from the new base.
    assert!(
        !error.contains("uncommitted"),
        "nothing was left uncommitted - git rolled the index back - so the evidence must come \
         from the commit's content being absent from the new base: {error}"
    );
}

/// The counterweight to the two tests above, and the reason it is worth having.
/// Both of the probes those tests pin exist to stop a commit git could not write
/// from being dropped — and either of them could start answering "unwritable" for
/// a commit that genuinely became empty, at which point every replay of a branch
/// whose work `main` has independently caught up with fails instead of costing
/// nothing. So: the empty commit is dropped, the rebase runs to the end, the
/// branch's real commit survives, and the whole thing costs zero.
///
/// Reaching a genuine empty halt on git 2.55 takes doing, and the setup below is
/// the only route there. A commit whose patch is already upstream is dropped
/// without halting; a conflict resolution that empties a commit is dropped
/// silently by `rebase --continue`; and the `rebase.empty` and
/// `rebase.reapplyCherryPicks` *config keys* are ignored on this path. Only the
/// `--empty=stop` command-line flag reaches the halt, and `replay_rebase` never
/// passes it.
///
/// Hence the shape: the test starts the rebase itself with that flag, through the
/// scratch's own configured runner, and hands the halted rebase to
/// `replay_rebase`. The `git rebase <onto>` that `replay_rebase` opens with then
/// fails, because a rebase is already in progress — and the loop ignores that
/// outcome precisely *because* a rebase is in progress, going straight to
/// classifying the halt, which is the code under test. Simplify this setup and
/// the classification stops being exercised at all.
#[test]
fn drops_a_commit_that_genuinely_became_empty_and_finishes_the_rebase() {
    let repo = commit_emptied_by_main_repo();
    // Read the subjects from the fixture rather than restating them, so renaming
    // a fixture commit cannot quietly turn either assertion below into a tautology.
    let emptied_subject = repo.git(&["log", "-1", "--format=%s", "branch~1"]);
    let real_subject = repo.git(&["log", "-1", "--format=%s", "branch"]);

    let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
    let git = scratch.git();
    git.run(&["checkout", "-q", "--detach", "branch"])
        .expect("check out the branch detached in the scratch worktree");

    let started = git
        .try_run(&["rebase", "--empty=stop", "main"])
        .expect("run the rebase that should halt on the emptied commit");

    // Asserted, not assumed. A git that stopped halting here would otherwise let
    // this test pass without ever reaching the classification it exists to
    // protect, and the loss would be invisible.
    assert!(
        rebase_in_progress(&scratch),
        "this test only exercises the empty-commit classification if git actually halts on the \
         emptied commit, and it did not. git said:\n{}\n{}",
        started.stdout,
        started.stderr
    );
    let unmerged = git
        .lines(&["diff", "--name-only", "--diff-filter=U"])
        .expect("list unmerged paths at the halt");
    assert!(
        unmerged.is_empty(),
        "the halt this test is about is the one with nothing unmerged; git left {unmerged:?} \
         unmerged, which is a conflict and a different code path"
    );

    let cost = match scratch.replay_rebase("main") {
        Ok(cost) => cost,
        Err(error) => panic!(
            "dropping a commit that adds nothing to the new base loses no work, so the replay \
             should have skipped it and finished: {error:#}"
        ),
    };

    assert_eq!(
        cost.stops(),
        Stops::new(0),
        "an emptied commit halts the rebase but costs a human no decision, so it must not be \
         counted as a stop: {cost:?}"
    );
    assert_eq!(
        cost.hunks(),
        Hunks::new(0),
        "there is nothing to hand-merge in a commit that adds nothing: {cost:?}"
    );
    assert_eq!(
        cost.files(),
        Files::new(0),
        "no file conflicted, so none should be reported: {cost:?}"
    );

    assert!(
        !rebase_in_progress(&scratch),
        "the replay should have walked the rebase all the way to the end, not left it halted"
    );

    let subjects = git
        .lines(&["log", "--format=%s"])
        .expect("read the replayed history");
    assert!(
        subjects.contains(&real_subject),
        "the branch's real commit ({real_subject}) has to survive a replay that drops the \
         emptied one: {subjects:?}"
    );
    assert!(
        !subjects.contains(&emptied_subject),
        "the emptied commit ({emptied_subject}) should have been dropped, not committed onto \
         the new base: {subjects:?}"
    );

    for (name, expected) in [("x.txt", "x3\n"), ("y.txt", "y2\n")] {
        let actual = std::fs::read_to_string(scratch.path().join(name))
            .unwrap_or_else(|e| panic!("read {name} out of the replayed worktree: {e}"));
        assert_eq!(
            actual, expected,
            "{name} should hold {expected:?} once the branch is replayed onto main, not {actual:?}"
        );
    }
}

/// The last way this halt goes wrong once the classification is right: the
/// commit really did become empty, `rebase --skip` really is the answer, and the
/// skip itself fails. Dropping the emptied commit sends git straight on to the
/// branch's *real* commit, and that one has to be written — so a sealed object
/// database fails the skip rather than the classification, which is the one
/// outcome neither probe can see.
///
/// Left to the loop that failure is invisible. The next round finds a rebase
/// still in progress and carries on, re-issuing a skip that cannot start
/// working; whatever it eventually says describes the commit the failed skip
/// left the rebase sitting on rather than the commit the skip was dropping, and
/// never that a skip is what failed. So the replay has to read the skip's
/// outcome the moment it comes back: stop there, name the commit it was
/// dropping, and carry git's own message.
///
/// The setup is the one from the test above — the halt only exists on git 2.55
/// if the test starts the rebase with `--empty=stop` itself, and that test's
/// doc comment says why — with the object database sealed once the halt is in
/// place.
#[test]
fn refuses_to_report_a_cost_when_an_empty_commit_cannot_be_skipped() {
    let repo = commit_emptied_by_main_repo();
    // Read the abbreviation from the same object database the implementation
    // will abbreviate against, so `%h` here and `%h` there agree.
    let dropped_sha = repo.git(&["log", "-1", "--format=%h", "branch~1"]);
    let dropped_subject = repo.git(&["log", "-1", "--format=%s", "branch~1"]);
    // The commit the failed skip moves the rebase on to, which is *not* the one
    // being dropped.
    let next_sha = repo.git(&["log", "-1", "--format=%h", "branch"]);
    let next_subject = repo.git(&["log", "-1", "--format=%s", "branch"]);
    let objects = std::fs::canonicalize(repo.path().join(".git").join("objects"))
        .expect("canonicalize the object database path");

    let scratch = Scratch::create(repo.path(), "main").expect("create the scratch worktree");
    let git = scratch.git();
    git.run(&["checkout", "-q", "--detach", "branch"])
        .expect("check out the branch detached in the scratch worktree");

    let started = git
        .try_run(&["rebase", "--empty=stop", "main"])
        .expect("run the rebase that should halt on the emptied commit");

    // Asserted, not assumed: this test is about a *skip* failing, so it has to
    // begin from the halt where skipping is the right answer. A git that stopped
    // halting here, or that halted for some other reason, would otherwise let it
    // pass without exercising anything.
    assert!(
        rebase_in_progress(&scratch),
        "this test starts from a halt on the emptied commit, and git did not halt. git said:\n{}\n{}",
        started.stdout,
        started.stderr
    );
    let unmerged = git
        .lines(&["diff", "--name-only", "--diff-filter=U"])
        .expect("list unmerged paths at the halt");
    assert!(
        unmerged.is_empty(),
        "the halt this test is about is the one with nothing unmerged; git left {unmerged:?} \
         unmerged, which is a conflict and a different code path"
    );
    let mut left_behind = git
        .lines(&["diff", "--cached", "--name-only", "HEAD"])
        .expect("list staged content at the halt");
    left_behind.extend(
        git.lines(&["diff", "--name-only"])
            .expect("list unstaged content at the halt"),
    );
    assert!(
        left_behind.is_empty(),
        "the emptied commit must leave nothing behind, or the replay would classify this halt as \
         a commit it could not write and never reach the skip at all; git left {left_behind:?}"
    );

    // Sealed only now, so the halt above is reached against a writable store and
    // the skip is the first thing that has to add an object.
    let sealed = repo.seal_object_store();
    let result = scratch.replay_rebase("main");
    // Released before a single assertion runs, so a failing one cannot leave a
    // read-only directory behind for the temporary directory to trip over.
    drop(sealed);

    let error = match result {
        Ok(conflicts) => panic!(
            "git refused to skip the emptied commit, so the rebase never ran to the end - the \
             replay may not report a cost for it: {conflicts:?}"
        ),
        Err(error) => format!("{error:#}"),
    };

    assert!(
        error.contains(&dropped_sha) && error.contains(&dropped_subject),
        "the error should name the commit the skip was dropping ({dropped_sha} \
         {dropped_subject}): {error}"
    );
    // The failed skip leaves the rebase sitting on the branch's real commit, and
    // reporting *that* one is the misattribution this test exists to remove. Git's
    // own hint quotes it as `<full sha> # <subject>` when it says the pick was
    // rescheduled, which is a different shape from the `%h %s` the replay names a
    // stopped commit with - so this only fires when the replay itself presents the
    // wrong commit as the one it was dropping.
    assert!(
        !error.contains(&format!("{next_sha} {next_subject}")),
        "the skip was dropping {dropped_sha} {dropped_subject}, not {next_sha} {next_subject} - \
         the error must not name the commit the failed skip moved on to: {error}"
    );
    // Matched literally: the message the replay produces today already contains
    // the word "Skipping" while describing something else entirely, so anything
    // looser passes without the failure ever being attributed to a skip.
    assert!(
        error.contains("rebase --skip"),
        "the error should say that a rebase --skip is what failed: {error}"
    );

    // git says: "error: insufficient permission for adding an object to
    // repository database <path>". The path is interpolated rather than
    // translated, so matching on it is locale-independent - and it is the
    // canonicalized one because macOS resolves a temp dir's /var/... to
    // /private/var/....
    let objects = objects.display().to_string();
    assert!(
        error.contains(&objects),
        "the error should carry git's own message, which names {objects}: {error}"
    );

    // Half of what this is for is promptness: re-issuing a skip that cannot work
    // burns a thousand rounds of subprocesses and ends on a message that names
    // neither the commit nor anything git said.
    assert!(
        !error.contains("gave up"),
        "a skip git refused cannot start working, so the replay should stop at once rather than \
         spin to the round limit: {error}"
    );
}
