//! `wn` starts `gh` with the `GIT_` variables of a person and without the ones
//! a hook exports.
//!
//! `gh` reads the remotes of a repository through git, so under an inherited
//! `GIT_DIR` it names another repository. A fake `gh` runs no git, so a decoy
//! cannot show that damage. This test reads what the fake `gh` RECEIVED
//! instead. The fake records its environment on every call. The run must reach
//! both `gh repo view` and `gh api graphql`, which are the two sites that start
//! `gh`. No recorded call may hold a variable that git exports into a hook.
//! `GIT_SSH_COMMAND` is one that a person sets on purpose, so every call must
//! still hold it.
#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "every unwrap in this file is an assertion on a fixture file or on the spawn of the freshly built binary"
)]

use std::fs;
use std::os::unix::fs::PermissionsExt;

use gitscratch::testing::{DecoyRepository, TestRepo};

/// The line that starts each recorded call in the recording file.
const SEPARATOR: &str = "=== CALL ===";

/// The value of `GIT_SSH_COMMAND` that the test sets on the child.
const SSH_MARKER: &str = "ssh -i /marker/wn-test-key";

/// The repository the fake `gh` names.
const REPO: &str = "timmattison/tools";

/// A GraphQL answer with one open issue.
const ONE_ISSUE: &str = r#"{"data":{"repository":{
"i277":{"__typename":"Issue","number":277,"title":"First thing","state":"OPEN","stateReason":null}
}}}"#;

/// The variables that git exports into a hook and that a child must not see.
const HOOK_VARIABLES: [&str; 4] = [
    "GIT_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_CONFIG_PARAMETERS",
];

#[test]
fn wn_starts_gh_without_the_git_variables_of_a_hook() {
    let decoy_repo = TestRepo::init();
    decoy_repo.commit_file("decoy.txt", "decoy\n", "decoy commit");
    let decoy = DecoyRepository::of(decoy_repo);

    let bin = tempfile::tempdir().unwrap();
    let recording = bin.path().join("recording");
    let script = format!(
        r#"#!/bin/sh
{{
    printf '%s\n' '{SEPARATOR}'
    printf 'ARG1=%s\n' "$1"
    env
}} >> '{recording}'
if [ "$1" = "repo" ]; then
    printf '%s\n' '{REPO}'
    exit 0
fi
cat <<'WN_FAKE_GH_BODY'
{ONE_ISSUE}
WN_FAKE_GH_BODY
"#,
        recording = recording.display(),
    );
    let gh = bin.path().join("gh");
    fs::write(&gh, script).unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();

    let stand_in = tempfile::tempdir().unwrap();
    let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_wn"));
    decoy.aim(&mut command);
    command
        .env("GIT_SSH_COMMAND", SSH_MARKER)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.path().display()))
        .env("COLUMNS", "80")
        .env("NO_COLOR", "1")
        .env("WN_NO_CLIPBOARD", "1")
        .env("WN_NO_CLAUDE", "1")
        .arg("#277")
        .current_dir(stand_in.path());
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "wn failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let recorded = fs::read_to_string(&recording).unwrap();
    let calls: Vec<&str> = recorded
        .split(SEPARATOR)
        .filter(|call| !call.trim().is_empty())
        .collect();
    let first_argument = |name: &str| {
        calls
            .iter()
            .any(|call| call.lines().any(|line| line == format!("ARG1={name}")))
    };
    assert!(first_argument("repo"), "wn never ran `gh repo view`");
    assert!(first_argument("api"), "wn never ran `gh api graphql`");

    for call in &calls {
        for variable in HOOK_VARIABLES {
            let prefix = format!("{variable}=");
            assert!(
                !call.lines().any(|line| line.starts_with(&prefix)),
                "gh received {variable} in a call:\n{call}"
            );
        }
        assert!(
            call.lines()
                .any(|line| line == format!("GIT_SSH_COMMAND={SSH_MARKER}")),
            "gh lost GIT_SSH_COMMAND in a call:\n{call}"
        );
    }

    decoy.assert_untouched("wn");
}
