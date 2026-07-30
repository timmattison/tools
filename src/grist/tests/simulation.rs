mod support;

use grist::{BranchName, Files, Hunks, Simulator, Stops};
use support::{numbered_lines, replace_line, TestRepo};

/// A branch that rewrites the same region three times, and one that touches it
/// once. Landing the iterated branch second makes each of its three commits
/// collide with the already-squashed change; landing it first means only the
/// single-commit branch has to be replayed.
fn contested_region_repo() -> TestRepo {
    const CONTESTED_LINE: usize = 15;

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    repo.commit_file("shared.txt", &base, "base");

    repo.branch("iterated");
    for revision in 1..=3 {
        let contents = replace_line(&base, CONTESTED_LINE, &format!("iterated-v{revision}"));
        repo.commit_file("shared.txt", &contents, &format!("iterate {revision}"));
    }

    repo.checkout("main");
    repo.branch("single");
    let contents = replace_line(&base, CONTESTED_LINE, "single-edit");
    repo.commit_file("shared.txt", &contents, "single edit");

    repo.checkout("main");
    repo
}

/// `built-on-top` was branched from `groundwork`, not from main - the stacked
/// shape that makes squash merging different from a real merge, because
/// squashing `built-on-top` destroys the commit identity of the `groundwork`
/// commits buried inside it.
fn stacked_branches_repo() -> TestRepo {
    const CONTESTED_LINE: usize = 15;

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    repo.commit_file("shared.txt", &base, "base");

    repo.branch("groundwork");
    let groundwork = replace_line(&base, CONTESTED_LINE, "groundwork-edit");
    repo.commit_file("shared.txt", &groundwork, "groundwork");

    repo.branch("built-on-top");
    let stacked = replace_line(&groundwork, CONTESTED_LINE, "built-on-top-edit");
    repo.commit_file("shared.txt", &stacked, "built on top");

    repo.checkout("main");
    repo
}

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
