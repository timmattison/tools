use gitscratch::testing::{contested_region_repo, numbered_lines, stacked_branches_repo, TestRepo};
use grist::{BranchName, Files, Hunks, Simulator, Stops};

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

    let simulator = Simulator::new(repo.path(), "main");
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
    let simulator = Simulator::new(repo.path(), "main");

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
    let simulator = Simulator::new(repo.path(), "main");

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
    let simulator = Simulator::new(repo.path(), "main");

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
    let simulator = Simulator::new(repo.path(), "main");

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
#[test]
fn refuses_more_branches_than_the_limit_instead_of_overflowing_the_ordering_count() {
    let names: Vec<String> = (1..=25).map(|n| format!("br{n}")).collect();
    let branches: Vec<BranchName> = names.iter().map(BranchName::new).collect();

    // No repository needed: the branch list is validated before any git work,
    // which is also why an over-limit run costs nothing to reject.
    let simulator = Simulator::new("/grist-does-not-exist", "main");

    let error = simulator
        .evaluate(&branches)
        .expect_err("25 branches is far past the limit");

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
#[test]
fn refuses_an_empty_branch_list_rather_than_ranking_nothing() {
    // No repository needed: the branch list is validated before any git work,
    // the same reason an over-limit list costs nothing to reject.
    let simulator = Simulator::new("/grist-does-not-exist", "main");

    let error = simulator
        .evaluate(&[])
        .expect_err("there is nothing to put in an order");

    let message = error.to_string();
    assert!(
        message.contains("no branches to order"),
        "expected the error to say there was nothing to order, got: {message}"
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
