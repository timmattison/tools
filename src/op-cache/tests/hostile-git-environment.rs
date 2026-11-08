//! `op-cache read` caches in the repository it stands in, not in the repository
//! that an inherited `GIT_DIR` names.
//!
//! A hook exports `GIT_DIR` and its siblings, and `op-cache` can run from a
//! hook. The cache file holds secrets, so it must never land in another
//! repository. The test aims the hostile variables at a decoy and runs the real
//! binary from a subdirectory of a fixture. A fake `op` answers with a fixed
//! fake secret, so the test never reaches the real 1Password CLI.
#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use gitscratch::testing::{DecoyRepository, TestRepo};

/// The fake secret that the fake `op` prints. It is not a real credential.
const FAKE_SECRET: &str = "fake-secret-for-tests-only";

/// The name of the cache file at the root of a repository.
const CACHE_FILENAME: &str = ".op-cache.json";

/// Writes a fake `op` into `dir`. It prints the fake secret for `op read <path>`.
fn write_fake_op(dir: &Path) {
    let script = dir.join("op");
    fs::write(
        &script,
        format!("#!/bin/sh\nprintf '%s' '{FAKE_SECRET}'\nexit 0\n"),
    )
    .expect("write the fake op");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("make the fake op run");
}

#[test]
fn read_caches_in_the_repository_it_stands_in() {
    let fixture = TestRepo::init();
    fixture.commit_file("a.txt", "a\n", "first");
    let subdirectory = fixture.path().join("nested");
    fs::create_dir(&subdirectory).expect("create the subdirectory");

    let decoy_repo = TestRepo::init();
    decoy_repo.commit_file("decoy.txt", "decoy\n", "decoy commit");
    let decoy = DecoyRepository::of(decoy_repo);

    let fake_directory = tempfile::tempdir().expect("create the fake op directory");
    write_fake_op(fake_directory.path());
    let mut search_path = vec![fake_directory.path().to_path_buf()];
    search_path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path: OsString = std::env::join_paths(search_path).expect("join PATH");

    let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_op-cache"));
    decoy.aim(&mut command);
    // `GIT_DIR` alone cannot make `git rev-parse --show-toplevel` name the
    // decoy: without a work tree, git answers with the current directory. With
    // `GIT_WORK_TREE`, a leaked environment makes the answer the decoy.
    command.env("GIT_WORK_TREE", decoy.path());
    command
        .env("PATH", path)
        .args(["read", "op://Private/Item/field"])
        .current_dir(&subdirectory);
    let output = command.output().expect("run op-cache");

    assert!(
        output.status.success(),
        "op-cache read failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), FAKE_SECRET);
    assert!(
        fixture.path().join(CACHE_FILENAME).is_file(),
        "the cache file must sit at the root of the fixture"
    );
    assert!(
        !subdirectory.join(CACHE_FILENAME).exists(),
        "the cache file must not sit in the subdirectory"
    );
    decoy.assert_untouched("op-cache");
}
