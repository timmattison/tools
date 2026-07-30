//! End-to-end coverage of the binary itself, so the wiring between argument
//! parsing, simulation and output is exercised the way a user runs it.

mod support;

use std::process::Command;

use support::contested_region_repo;

fn grist(repo: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_grist"))
        .args(args)
        .current_dir(repo)
        .output()
        .expect("failed to run grist")
}

/// `-q` exists so the answer can be piped straight into the next command.
#[test]
fn quiet_mode_prints_only_the_winning_order() {
    let repo = contested_region_repo();

    let output = grist(repo.path(), &["--onto", "main", "-q", "single", "iterated"]);

    assert!(
        output.status.success(),
        "grist failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "iterated single"
    );
}

/// The table names both orderings and marks the winner.
#[test]
fn default_output_ranks_both_orderings_with_their_costs() {
    let repo = contested_region_repo();

    let output = grist(repo.path(), &["--onto", "main", "single", "iterated"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(
        stdout.contains("iterated \u{2192} single"),
        "got:\n{stdout}"
    );
    assert!(
        stdout.contains("single \u{2192} iterated"),
        "got:\n{stdout}"
    );
    assert!(
        stdout.contains("Land them in this order: iterated single"),
        "got:\n{stdout}"
    );
}

/// Listing the same branch twice is a mistake, not a plan.
#[test]
fn rejects_a_repeated_branch() {
    let repo = contested_region_repo();

    let output = grist(repo.path(), &["--onto", "main", "single", "single"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("only be listed once"),
        "got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
