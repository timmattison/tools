//! `swt create` under the git environment that a hook exports.
//!
//! A hook exports `GIT_DIR`, `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY` and
//! `GIT_CONFIG_PARAMETERS`. `swt` can run from a hook, so it must act on the
//! repository it stands in. This test aims those variables at a decoy
//! repository and proves two things. The worktree appears beside the fixture,
//! and the decoy stays byte-identical. It also proves that the green check does
//! not inherit the variables, because a test suite that the check starts would
//! otherwise act on the decoy.
//!
//! Unix only: the check is an `sh` script.
#![cfg(unix)]

mod support;

use std::fs;

use gitscratch::testing::{DecoyRepository, TestRepo as DecoyFixture};
use support::{swt_command, unique, write_swt_check, TestRepo};

#[test]
fn create_acts_on_its_own_repository_and_the_check_sees_no_aiming_variable() {
    let repo = TestRepo::new();
    let recording = tempfile::tempdir().expect("create the recording directory");
    let recorded = recording.path().join("check-environment");
    write_swt_check(
        repo.path(),
        &format!(
            "#!/bin/sh\nenv > {}\nexit 0\n",
            shellquote::shell_quote(&recorded.to_string_lossy())
        ),
    );

    let decoy_repo = DecoyFixture::init();
    decoy_repo.commit_file("decoy.txt", "decoy\n", "decoy commit");
    let decoy = DecoyRepository::of(decoy_repo);

    let name = unique("hostile");
    let mut command = swt_command(repo.path());
    decoy.aim(&mut command);
    command.args(["create", &name]);
    let output = command.output().expect("run swt");

    assert!(
        output.status.success(),
        "swt create failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        repo.created_worktrees(&name).len(),
        1,
        "the fixture must hold exactly one new worktree"
    );
    assert_eq!(
        repo.created_branches(&name).len(),
        1,
        "the fixture must hold the new branch"
    );
    decoy.assert_untouched("swt");

    let environment = fs::read_to_string(&recorded).expect("the check records its environment");
    for line in environment.lines() {
        for (name, _) in decoy.hostile_environment() {
            let prefix = format!("{name}=");
            assert!(!line.starts_with(&prefix), "the check inherited {line:?}");
        }
    }
}
