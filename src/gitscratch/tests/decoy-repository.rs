//! What `gitscratch::testing::DecoyRepository` reports.
//!
//! An unchanged decoy passes. A new file, a changed file, and a removed file
//! each make `assert_untouched` panic, and the panic names the tool and the
//! path.

use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};

use gitscratch::testing::{DecoyRepository, TestRepo};

/// A decoy with one committed file, `tracked.txt`.
fn decoy() -> DecoyRepository {
    let repo = TestRepo::init();
    repo.commit_file("tracked.txt", "baseline\n", "baseline");
    DecoyRepository::of(repo)
}

/// The text of the panic that `assert_untouched` raises, or `None` for a pass.
fn panic_text(decoy: &DecoyRepository) -> Option<String> {
    let caught = catch_unwind(AssertUnwindSafe(|| decoy.assert_untouched("probe-tool")));

    caught.err().map(|payload| {
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_string())
            })
            .unwrap_or_default()
    })
}

#[test]
fn an_unchanged_decoy_passes() {
    decoy().assert_untouched("probe-tool");
}

#[test]
fn a_new_file_is_reported() {
    let decoy = decoy();
    fs::write(decoy.path().join("intruder.txt"), "new\n").expect("write the new file");

    let text = panic_text(&decoy).expect("a new file must fail the check");
    assert!(text.contains("probe-tool"), "{text}");
    assert!(text.contains("appeared: intruder.txt"), "{text}");
}

#[test]
fn a_changed_file_is_reported() {
    let decoy = decoy();
    fs::write(decoy.path().join("tracked.txt"), "changed\n").expect("change the file");

    let text = panic_text(&decoy).expect("a changed file must fail the check");
    assert!(text.contains("probe-tool"), "{text}");
    assert!(text.contains("changed:  tracked.txt"), "{text}");
}

#[test]
fn a_removed_file_is_reported() {
    let decoy = decoy();
    fs::remove_file(decoy.path().join("tracked.txt")).expect("remove the file");

    let text = panic_text(&decoy).expect("a removed file must fail the check");
    assert!(text.contains("probe-tool"), "{text}");
    assert!(text.contains("vanished: tracked.txt"), "{text}");
}

#[test]
fn the_hostile_environment_aims_at_the_decoy() {
    let decoy = decoy();
    let environment = decoy.hostile_environment();
    let value = |name: &str| {
        environment
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string_lossy().into_owned())
            .unwrap_or_default()
    };

    assert!(value("GIT_DIR").ends_with("/.git"));
    assert!(value("GIT_INDEX_FILE").ends_with("/.git/index"));
    assert!(value("GIT_OBJECT_DIRECTORY").ends_with("/.git/objects"));
    assert_eq!(
        value("GIT_CONFIG_PARAMETERS"),
        "'gitscratch.envleakprobe=leaked'"
    );

    let mut command = childenv::fixture_command("/usr/bin/env");
    decoy.aim(&mut command);
    // A sweep records each `GIT_` variable of this process as a removal, so
    // only a value that the command sets counts here.
    let aimed = command
        .get_envs()
        .filter(|(_, value)| value.is_some())
        .count();
    assert_eq!(aimed, 4);
}
