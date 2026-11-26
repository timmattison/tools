//! The mount table of the machine that runs the test. macOS only.

#![cfg(target_os = "macos")]

use std::path::Path;

use ff::machine::{mount_table, plan_this_machine};

#[test]
fn the_mount_table_holds_the_root_volume_and_the_device_files() {
    let mounts = mount_table().expect("read the mount table");

    assert!(
        mounts
            .iter()
            .any(|mount| mount.mount_point == Path::new("/")),
        "{mounts:?}"
    );
    assert!(
        mounts
            .iter()
            .any(|mount| mount.mount_point == Path::new("/dev") && mount.fs_type == "devfs"),
        "{mounts:?}"
    );
}

#[test]
fn the_plan_of_this_machine_walks_the_root_volume_and_skips_the_device_files() {
    let plan = plan_this_machine().expect("plan this machine");

    assert!(
        plan.roots.iter().any(|root| root.path == Path::new("/")),
        "{plan:?}"
    );
    assert!(
        plan.skipped
            .iter()
            .any(|skipped| skipped.mount_point == Path::new("/dev")),
        "{plan:?}"
    );
}
