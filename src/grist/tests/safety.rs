//! `grist` runs against the developer's real repository. These tests pin the
//! properties that make that acceptable.

mod support;

use grist::{BranchName, Simulator};
use support::conflicting_repo;

fn order(names: &[&str]) -> Vec<BranchName> {
    names.iter().map(|n| BranchName::new(*n)).collect()
}

/// `rebase.updateRefs` rewrites any branch pointing into the range being
/// replayed - which is exactly the branch under simulation. A developer who has
/// turned it on must not lose their branches to a dry run.
#[test]
fn never_moves_real_branch_refs_even_when_rebase_update_refs_is_enabled() {
    let repo = conflicting_repo();
    repo.git(&["config", "rebase.updateRefs", "true"]);

    let before: Vec<(String, String)> = ["main", "left", "right"]
        .iter()
        .map(|name| ((*name).to_string(), repo.rev_parse(name)))
        .collect();

    let simulator = Simulator::new(repo.path(), "main");
    simulator
        .score(&order(&["left", "right"]))
        .expect("simulation runs");

    for (name, sha) in before {
        assert_eq!(repo.rev_parse(&name), sha, "simulation moved branch '{name}'");
    }
}

/// The branches worth comparing are usually the ones already checked out in
/// other worktrees - which is exactly the situation where a plain `git checkout`
/// refuses to run. Simulation must detach instead.
#[test]
fn works_when_the_branches_are_checked_out_in_other_worktrees() {
    let repo = conflicting_repo();
    let _left = repo.add_worktree("left");
    let _right = repo.add_worktree("right");

    let simulator = Simulator::new(repo.path(), "main");

    let ranked = simulator
        .evaluate(&order(&["left", "right"]))
        .expect("evaluation runs even though both branches are checked out elsewhere");

    assert_eq!(ranked.len(), 2);
}
