use gitscratch::testing::{
    contested_region_repo, not_a_repository, numbered_lines, stacked_branches_repo, TestRepo,
};
use grist::{orderings_to_simulate, BranchName, Files, Hunks, Simulator, Stops};

fn order(names: &[&str]) -> Vec<BranchName> {
    names.iter().map(|n| BranchName::new(*n)).collect()
}

/// Two branches that never touch the same file cost nothing in either order.
#[test]
fn scores_an_ordering_with_no_overlap_as_free() {
    let repo = TestRepo::init();
    repo.commit_file("base.txt", &numbered_lines(30), "base");

    repo.branch("alpha");
    repo.commit_file("alpha.txt", "alpha owns this\n", "alpha work");

    repo.checkout("main");
    repo.branch("beta");
    repo.commit_file("beta.txt", "beta owns this\n", "beta work");

    repo.checkout("main");

    let simulator = Simulator::new(repo.path(), "main").expect("open the fixture repository");
    let score = simulator
        .score(&order(&["alpha", "beta"]))
        .expect("simulation runs");

    assert_eq!(score.stops(), Stops::new(0));
    assert_eq!(score.files(), Files::new(0));
    assert_eq!(score.hunks(), Hunks::new(0));
}

/// The whole premise of the tool: the same two branches cost different amounts
/// depending on which one is squashed in first.
#[test]
fn charges_more_when_the_heavily_iterated_branch_lands_second() {
    let repo = contested_region_repo();
    let simulator = Simulator::new(repo.path(), "main").expect("open the fixture repository");

    let iterated_first = simulator
        .score(&order(&["iterated", "single"]))
        .expect("simulation runs");
    let iterated_second = simulator
        .score(&order(&["single", "iterated"]))
        .expect("simulation runs");

    // Replaying one commit against the squashed result stops once.
    assert_eq!(iterated_first.stops(), Stops::new(1));
    // Replaying three commits over the same contested line stops for each.
    assert_eq!(iterated_second.stops(), Stops::new(3));
    assert!(
        iterated_second.hunks() > iterated_first.hunks(),
        "expected landing the iterated branch second to cost more hunks, got {} vs {}",
        iterated_second.hunks(),
        iterated_first.hunks()
    );
}

/// Squash merging a stacked branch strands the branch underneath it: the
/// groundwork commits are inside the squash as content but not as history, so
/// replaying them collides instead of being recognised as already applied.
///
/// This is what distinguishes a squash merge from a real merge, and it only
/// shows up on stacked branches.
#[test]
fn landing_a_stacked_branch_first_strands_the_branch_beneath_it() {
    let repo = stacked_branches_repo();
    let simulator = Simulator::new(repo.path(), "main").expect("open the fixture repository");

    let groundwork_first = simulator
        .score(&order(&["groundwork", "built-on-top"]))
        .expect("simulation runs");
    let stacked_first = simulator
        .score(&order(&["built-on-top", "groundwork"]))
        .expect("simulation runs");

    // Bottom-up: groundwork lands, then its own follow-up applies cleanly.
    assert_eq!(groundwork_first.stops(), Stops::new(0));
    assert_eq!(groundwork_first.hunks(), Hunks::new(0));

    // Top-down: groundwork's commit now collides with the squashed content.
    assert_eq!(stacked_first.stops(), Stops::new(1));
    assert_eq!(stacked_first.files(), Files::new(1));
}

/// End to end: given the branches in the *expensive* order, grist still ranks
/// the cheap order first. Passing the losing order in guarantees a pass cannot
/// be an artefact of input ordering.
#[test]
fn ranks_the_cheaper_ordering_first_regardless_of_input_order() {
    let repo = contested_region_repo();
    let simulator = Simulator::new(repo.path(), "main").expect("open the fixture repository");

    let ranked = simulator
        .evaluate(&order(&["single", "iterated"]))
        .expect("evaluation runs");

    assert_eq!(ranked.len(), 2, "both orderings should be evaluated");
    assert_eq!(
        ranked[0].order(),
        order(&["iterated", "single"]).as_slice(),
        "the heavily iterated branch should be recommended to land first"
    );
    assert!(ranked[0].hunks() < ranked[1].hunks());
}

/// Memoising shared prefixes must be a pure optimisation: every ranked score
/// has to match what an independent, uncached run of that ordering produces.
#[test]
fn memoised_evaluation_agrees_with_scoring_each_ordering_independently() {
    let repo = contested_region_repo();
    let simulator = Simulator::new(repo.path(), "main").expect("open the fixture repository");

    let ranked = simulator
        .evaluate(&order(&["iterated", "single"]))
        .expect("evaluation runs");

    for memoised in &ranked {
        let independent = simulator.score(memoised.order()).expect("simulation runs");
        assert_eq!(
            &independent, memoised,
            "memoised score disagrees with an independent replay of the same ordering"
        );
    }
}

/// Orderings grow factorially, so grist refuses a branch list it would never
/// finish. The refusal has to survive counts whose factorial does not fit in a
/// `usize`: deriving the ordering count before checking the limit makes the very
/// guard that exists to produce a friendly error the thing that blows up.
///
/// Asked of `orderings_to_simulate` rather than of a `Simulator`, because that
/// function *is* the guard, and because asking it directly is the only way left
/// to show what rejecting an over-limit list costs: no repository, no scratch
/// worktree, not one git process. This test used to make that point by handing a
/// `Simulator` a path that does not exist — if any git work had preceded the
/// check, the error would have been about the missing path instead. That
/// tripwire has moved: `Simulator::new` now opens the repository, so a
/// nonexistent path is refused by the constructor and never reaches a branch
/// list at all (see the pre-flight test below). Pointing straight at the guard
/// keeps the property pinned somewhere it is still visible.
#[test]
fn refuses_more_branches_than_the_limit_instead_of_overflowing_the_ordering_count() {
    let names: Vec<String> = (1..=25).map(|n| format!("br{n}")).collect();
    let branches: Vec<BranchName> = names.iter().map(BranchName::new).collect();

    let error = orderings_to_simulate(&branches).expect_err("25 branches is far past the limit");

    let message = error.to_string();
    assert!(
        message.contains("25"),
        "expected the error to name the branch count it rejected, got: {message}"
    );
    assert!(
        message.contains(&grist::simulate::MAX_BRANCHES.to_string()),
        "expected the error to name the limit, got: {message}"
    );
}

/// An empty branch list is the one input for which "the cheapest ordering" has
/// no answer, so it has to be refused by name. Ranking nothing succeeds just as
/// readily as ranking something - it yields the single empty ordering - and a
/// caller handed that empty-but-successful result reads it as "your branches are
/// already in the best order" rather than "you named no branches".
///
/// Asked of `evaluate`, against a real repository, which is what makes this a
/// test of the guard being *applied* rather than merely existing: take the
/// `orderings_to_simulate` call out of `evaluate` and nothing errors at all —
/// the empty ordering is scored cheerfully and `expect_err` below fails. The
/// repository is a real one because `Simulator::new` now insists on one, and
/// that is no loss: an empty list is still refused before a scratch worktree is
/// built, since ranking nothing is decided before there is anything to rank.
#[test]
fn refuses_an_empty_branch_list_rather_than_ranking_nothing() {
    let repo = TestRepo::init();
    repo.commit_file("base.txt", "base\n", "base");

    let simulator = Simulator::new(repo.path(), "main").expect("open the fixture repository");

    let error = simulator
        .evaluate(&[])
        .expect_err("there is nothing to put in an order");

    let message = error.to_string();
    assert!(
        message.contains("no branches to order"),
        "expected the error to say there was nothing to order, got: {message}"
    );
}

/// Somewhere outside every repository there is nothing to simulate, and that has
/// to be settled when the `Simulator` is built rather than when it first runs.
///
/// `tests/cli.rs` pins what a user sees — the refusal reaching stderr before any
/// run is announced. This pins the structural reason it can: the constructor
/// carries `gitscratch`'s pre-flight, so a `Simulator` that exists is one with
/// somewhere to run, and no caller can be holding one that was never going to
/// work. Left to the first replay, the same fact arrives as git's own complaint
/// from inside `worktree add`, naming `.git` instead of the directory that was
/// actually wrong.
#[test]
fn refuses_to_simulate_against_a_directory_that_is_not_a_repository() {
    let elsewhere = not_a_repository();

    // `.err()` rather than `expect_err`, which would want a `Debug` on
    // `Simulator` that exists for no other reason than this line.
    let error = Simulator::new(elsewhere.path(), "main")
        .err()
        .expect("a directory outside every repository has no orderings to rank");

    let message = format!("{error:#}");
    assert!(
        message.contains("is not inside a git repository"),
        "the refusal has to be the pre-flight's, not a failed simulation's, got: {message}"
    );
    assert!(
        message.contains(&elsewhere.path().display().to_string()),
        "the refusal has to name the directory it was pointed at, got: {message}"
    );
}

/// A dry run has exactly two honest answers when it cannot replay: "expensive"
/// or "I cannot answer". "Cheap" is never one of them, and neither entry point
/// may quietly turn a failed replay into one.
///
/// This repository is the input where that matters most. Two branches that touch
/// nothing in common cost zero in either order, so unsealed it ranks every
/// ordering as a tie at zero — exactly the number a replay that threw its work
/// away also produces. A failure folded into a score here would be
/// indistinguishable from the truth.
///
/// Sealing the object database is what forces the failure, and it is the replay
/// that it lands on: `main` has moved ahead of both branches, so replaying
/// either has to *write* a commit, while `git worktree add` writes no objects
/// and still succeeds. So the scratch worktree is built, the first branch is
/// checked out, and the rebase is the first thing that cannot proceed.
///
/// Both entry points propagate the replay's `Result` today, so this passes on
/// arrival. It exists so a later `unwrap_or_default`, a swallowed branch, or a
/// per-ordering `continue` cannot make grist confident about work it never did.
#[cfg(unix)]
#[test]
fn refuses_to_score_an_ordering_whose_replay_could_not_be_carried_out() {
    use gitscratch::testing::branches_behind_main_repo;

    let repo = branches_behind_main_repo();
    // Read the abbreviation from the same object database the replay will
    // abbreviate against, so `%h` here and `%h` there agree.
    let dropped_sha = repo.git(&["log", "-1", "--format=%h", "alpha"]);
    let dropped_subject = repo.git(&["log", "-1", "--format=%s", "alpha"]);

    let simulator = Simulator::new(repo.path(), "main").expect("open the fixture repository");
    let branches = order(&["alpha", "beta"]);

    // Sealed only around the calls themselves: building the repository and
    // reading it back needs a writable store, and so does tearing the temporary
    // directory down.
    let sealed = repo.seal_object_store();
    let evaluated = simulator.evaluate(&branches);
    let scored = simulator.score(&branches);
    // Released before a single assertion runs, so a failing one cannot leave a
    // read-only directory behind for the temporary directory to trip over.
    drop(sealed);

    // Both public entry points, held to the same standard. `evaluate` is what
    // the binary calls and `score` is what a library caller reaches for, and a
    // guard on one is no guard at all.
    for (entry_point, result) in [
        ("evaluate", evaluated.map(|scores| format!("{scores:?}"))),
        ("score", scored.map(|score| format!("{score:?}"))),
    ] {
        let error = match result {
            Ok(reported) => panic!(
                "`{entry_point}` reported a cost for a replay that never happened: {reported}\n\
                 every ordering here ties at zero, so a swallowed failure is indistinguishable \
                 from 'they all cost nothing, pick whichever you prefer'"
            ),
            Err(error) => format!("{error:#}"),
        };

        // Erroring is not enough on its own, and not academically so: swallow
        // the replay's result and this input still fails, just later and
        // elsewhere - the squash's own `commit-tree` cannot write against a
        // sealed store either. So `is_err()` alone stays green while the replay
        // silently reports zero. These two are what pin the failure to the
        // replay: which branch was being landed, and which commit git could not
        // write.
        assert!(
            error.contains("could not replay 'alpha'"),
            "`{entry_point}` should say whose replay failed, not just that something did: {error}"
        );
        assert!(
            error.contains(&dropped_sha) && error.contains(&dropped_subject),
            "`{entry_point}` should name the commit git could not write ({dropped_sha} \
             {dropped_subject}): {error}"
        );
    }
}

/// A branch name that starts with a dash is a branch name, and the checkout
/// that puts the replay on it has to read it as one.
///
/// `git checkout -q --detach --progress` is a complete and valid command. Git
/// reads `--progress` as its own option, finds no branch left to check out, and
/// detaches HEAD where it already stands - exit 0, no complaint. So the scratch
/// worktree stayed on the base, the rebase found nothing to replay, and the
/// ordering scored zero: a free ordering for a branch nobody replayed. Zero is
/// also what a genuinely free ordering scores, so nothing downstream can tell
/// the two apart.
///
/// `--progress` rather than a name nobody would type, because it is the shape
/// that succeeds. A dash-leading name git does not know fails either way, and
/// `-b` cannot be used with `--detach` at all; this one is the name that used
/// to be obeyed.
///
/// The control at the end scores an ordering of real branches and requires it
/// to cost something. Without it a simulator that refused every branch, or one
/// that could not replay anything on this fixture, would pass here.
#[test]
fn refuses_a_branch_whose_name_starts_with_a_dash_rather_than_scoring_a_replay_it_never_did() {
    let repo = contested_region_repo();
    let simulator = Simulator::new(repo.path(), "main").expect("open the fixture repository");

    let error = simulator
        .score(&order(&["--progress"]))
        .map(|score| format!("{score:?}"))
        .expect_err(
            "a branch name that names no branch has to stop the run. Reading it as an option \
             leaves the scratch worktree on the base, so the ordering scores zero for work that \
             was never replayed",
        );

    let message = format!("{error:#}");
    assert!(
        message.contains("--progress"),
        "the refusal has to name the branch it could not check out: {message}"
    );

    let control = simulator
        .score(&order(&["single", "iterated"]))
        .expect("score an ordering of branches the fixture really has");

    assert!(
        control.hunks() > Hunks::new(0),
        "the fixture has to cost something, or the refusal above proves only that this \
         simulator answers nothing at all, got {}",
        control.hunks()
    );
}

/// Replaying dozens of commits takes real time, so the caller needs to be told
/// what is happening rather than staring at a silent terminal.
#[test]
fn reports_progress_for_each_branch_it_lands() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let repo = contested_region_repo();
    let seen = Rc::new(RefCell::new(Vec::new()));

    let recorder = Rc::clone(&seen);
    let simulator = Simulator::new(repo.path(), "main")
        .expect("open the fixture repository")
        .with_progress(move |message| recorder.borrow_mut().push(message.to_owned()));

    simulator
        .score(&order(&["iterated", "single"]))
        .expect("simulation runs");

    let messages = seen.borrow();
    assert!(
        messages.iter().any(|message| message.contains("iterated")),
        "expected progress mentioning 'iterated', got {messages:?}"
    );
    assert!(
        messages.iter().any(|message| message.contains("single")),
        "expected progress mentioning 'single', got {messages:?}"
    );
}
