//! `grist` runs against the developer's real repository. These tests pin the
//! properties that make that acceptable.

mod support;

use grist::{BranchName, Simulator};
use support::{numbered_lines, replace_line, TestRepo};

fn order(names: &[&str]) -> Vec<BranchName> {
    names.iter().map(|n| BranchName::new(*n)).collect()
}

/// Two branches that both rewrite the same line, so the simulation is
/// guaranteed to actually conflict and resolve rather than no-op.
fn conflicting_repo() -> TestRepo {
    const CONTESTED_LINE: usize = 15;

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    repo.commit_file("shared.txt", &base, "base");

    repo.branch("left");
    repo.commit_file(
        "shared.txt",
        &replace_line(&base, CONTESTED_LINE, "left-edit"),
        "left work",
    );

    repo.checkout("main");
    repo.branch("right");
    repo.commit_file(
        "shared.txt",
        &replace_line(&base, CONTESTED_LINE, "right-edit"),
        "right work",
    );

    repo.checkout("main");
    repo
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
