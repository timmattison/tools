mod support;

use grist::{BranchName, Files, Hunks, Simulator, Stops};
use support::{numbered_lines, TestRepo};

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
