//! `grist` runs against the developer's real repository. This suite pins the
//! one property that makes that acceptable and that only `grist` can pin: a
//! full simulation, composed the way `grist` composes it, leaves every real
//! branch ref exactly where it found it.

use gitscratch::testing::conflicting_repo;
use grist::{BranchName, Simulator};

fn order(names: &[&str]) -> Vec<BranchName> {
    names.iter().map(|n| BranchName::new(*n)).collect()
}

/// `gitscratch` already pins that [`gitscratch::Scratch`] on its own never moves
/// a branch ref, so why assert it again here?
///
/// Because that test drives `Scratch` through a `checkout --detach` the test
/// itself spells out. It cannot see how `grist` composes on top: `Simulator`'s
/// own `land()` does `checkout --detach` → `replay_rebase` → `squash_into`, once
/// per branch in the ordering, and the detach in that sequence belongs to
/// `grist`, not to `gitscratch`. Drop `--detach` from `land()` and every
/// `gitscratch` test still passes while every real developer branch gets moved
/// by a dry run. This test is what fails.
#[test]
fn a_full_simulation_never_moves_real_branch_refs() {
    let repo = conflicting_repo();

    let before: Vec<(String, String)> = ["main", "left", "right"]
        .iter()
        .map(|name| ((*name).to_string(), repo.rev_parse(name)))
        .collect();

    // Scoped so the `Simulator` — and with it the scratch worktree it owns — is
    // dropped before the refs are re-read: teardown runs on drop, and teardown
    // is part of what must not move a branch. Do not flatten this block away.
    {
        let simulator = Simulator::new(repo.path(), "main").expect("open the fixture repository");
        simulator
            .score(&order(&["left", "right"]))
            .expect("simulation runs");
    }

    for (name, sha) in before {
        assert_eq!(
            repo.rev_parse(&name),
            sha,
            "simulation moved branch '{name}'"
        );
    }
}
