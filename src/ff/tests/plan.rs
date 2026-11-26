//! The plan of a search of the whole machine: one walk of the data volume, and
//! no directory of it left out.

#[path = "common/tree.rs"]
mod tree;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ff::fs::RealFs;
use ff::pattern::{Case, Matcher};
use ff::plan::{plan_machine, Firmlink, Firmlinks, Mount, SkipReason, SkippedMount};
use ff::walk::{walk, Options};
use tree::{sorted, Found, Tree};

/// A part of the firmlink map of a real Mac, in the form of the file.
const FIRMLINKS: &str = "/Applications\tApplications\n\
/Library\tLibrary\n\
/System/Library/Caches\tSystem/Library/Caches\n\
/Users\tUsers\n\
/Volumes\tVolumes\n\
/private\tprivate\n\
/usr/local\tusr/local\n";

/// The mount table of a real Mac, with one external disk.
fn mounts() -> Vec<Mount> {
    vec![
        Mount::new("/", "apfs"),
        Mount::new("/dev", "devfs"),
        Mount::new("/System/Volumes/VM", "apfs"),
        Mount::new("/System/Volumes/Preboot", "apfs"),
        Mount::new("/System/Volumes/Update", "apfs"),
        Mount::new("/System/Volumes/xarts", "apfs"),
        Mount::new("/System/Volumes/iSCPreboot", "apfs"),
        Mount::new("/System/Volumes/Hardware", "apfs"),
        Mount::new("/System/Volumes/Data", "apfs"),
        Mount::new("/System/Volumes/Data/home", "autofs"),
        Mount::new("/Volumes/External", "hfs"),
        Mount::new("/Library/Developer/CoreSimulator/Volumes/iOS_23F77", "apfs"),
        Mount::new("/System/Volumes/Update/mnt1", "apfs"),
    ]
}

fn paths(items: &[&str]) -> BTreeSet<PathBuf> {
    items.iter().map(PathBuf::from).collect()
}

#[test]
fn the_firmlink_map_is_read_from_its_file_form() {
    let firmlinks = Firmlinks::parse("/Users\tUsers\n\n/usr/local\tusr/local\nno tab here\n");
    assert_eq!(
        firmlinks.entries(),
        [
            Firmlink {
                system_path: PathBuf::from("/Users"),
                data_path: PathBuf::from("Users"),
            },
            Firmlink {
                system_path: PathBuf::from("/usr/local"),
                data_path: PathBuf::from("usr/local"),
            },
        ]
    );
}

#[test]
fn the_plan_walks_the_data_volume_once_and_covers_each_directory_without_a_firmlink() {
    let plan = plan_machine(Path::new("/"), &mounts(), &Firmlinks::parse(FIRMLINKS));

    let root_paths: Vec<_> = plan.roots.iter().map(|root| root.path.clone()).collect();
    assert_eq!(
        root_paths,
        ["/", "/System/Volumes/Data", "/Volumes/External"].map(PathBuf::from)
    );
    assert!(plan.roots.iter().all(|root| root.stay_on_device));
    assert!(plan.roots.iter().all(|root| root.canonical == root.path));
    assert!(plan.roots.iter().all(|root| !root.named));

    let system = &plan.roots[0];
    assert_eq!(
        system.prune,
        paths(&[
            "/Library/Developer/CoreSimulator/Volumes/iOS_23F77",
            "/System/Volumes/Data",
            "/System/Volumes/Data/home",
            "/System/Volumes/Hardware",
            "/System/Volumes/Preboot",
            "/System/Volumes/Update",
            "/System/Volumes/Update/mnt1",
            "/System/Volumes/VM",
            "/System/Volumes/iSCPreboot",
            "/System/Volumes/xarts",
            "/Volumes/External",
            "/dev",
        ]),
        "the walk of / enters no other volume, and not the data volume"
    );

    let data = &plan.roots[1];
    assert_eq!(
        data.prune,
        paths(&["/System/Volumes/Data/home"]),
        "the data volume prunes its mount points and nothing else"
    );
    assert_eq!(
        data.hide,
        paths(&[
            "/System/Volumes/Data/Applications",
            "/System/Volumes/Data/Library",
            "/System/Volumes/Data/System/Library/Caches",
            "/System/Volumes/Data/Users",
            "/System/Volumes/Data/Volumes",
            "/System/Volumes/Data/private",
            "/System/Volumes/Data/usr/local",
        ]),
        "the data volume hides each firmlinked directory and nothing else"
    );
    for uncovered in [
        "/System/Volumes/Data/mnt",
        "/System/Volumes/Data/sw",
        "/System/Volumes/Data/MobileSoftwareUpdate",
        "/System/Volumes/Data/.PreviousSystemInformation",
    ] {
        assert!(!data.prune.contains(Path::new(uncovered)), "{uncovered}");
        assert!(!data.hide.contains(Path::new(uncovered)), "{uncovered}");
    }

    assert!(system.hide.is_empty());
    assert!(plan.roots[2].prune.is_empty());
    assert!(plan.roots[2].hide.is_empty());
}

#[test]
fn the_plan_skips_the_volumes_that_hold_no_user_files() {
    let plan = plan_machine(Path::new("/"), &mounts(), &Firmlinks::parse(FIRMLINKS));

    let skipped = |path: &str, reason: SkipReason| SkippedMount {
        mount_point: PathBuf::from(path),
        reason,
    };
    assert_eq!(
        plan.skipped,
        vec![
            skipped(
                "/Library/Developer/CoreSimulator/Volumes/iOS_23F77",
                SkipReason::SimulatorImage
            ),
            skipped("/System/Volumes/Data/home", SkipReason::Automounter),
            skipped("/System/Volumes/Hardware", SkipReason::SystemVolume),
            skipped("/System/Volumes/Preboot", SkipReason::SystemVolume),
            skipped("/System/Volumes/Update", SkipReason::SystemVolume),
            skipped("/System/Volumes/Update/mnt1", SkipReason::SystemVolume),
            skipped("/System/Volumes/VM", SkipReason::SystemVolume),
            skipped("/System/Volumes/iSCPreboot", SkipReason::SystemVolume),
            skipped("/System/Volumes/xarts", SkipReason::SystemVolume),
            skipped("/dev", SkipReason::DeviceFiles),
        ]
    );
}

#[test]
fn a_volume_inside_a_skipped_volume_is_skipped_too() {
    let table = [
        Mount::new("/", "apfs"),
        Mount::new("/System/Volumes/Data/home", "autofs"),
        Mount::new("/System/Volumes/Data/home/me", "nfs"),
    ];
    let plan = plan_machine(Path::new("/"), &table, &Firmlinks::default());

    assert!(plan.skipped.contains(&SkippedMount {
        mount_point: PathBuf::from("/System/Volumes/Data/home/me"),
        reason: SkipReason::InsideSkipped(PathBuf::from("/System/Volumes/Data/home")),
    }));
    assert_eq!(plan.roots.len(), 1);
}

#[test]
fn a_mount_point_that_the_table_lists_twice_is_one_root() {
    let table = [
        Mount::new("/", "apfs"),
        Mount::new("/Volumes/External", "hfs"),
        Mount::new("/Volumes/External", "hfs"),
    ];
    let plan = plan_machine(Path::new("/"), &table, &Firmlinks::default());

    assert_eq!(plan.roots.len(), 2);
}

#[test]
fn without_the_system_root_the_data_volume_hides_no_firmlink() {
    let table = [Mount::new("/System/Volumes/Data", "apfs")];
    let plan = plan_machine(Path::new("/"), &table, &Firmlinks::parse(FIRMLINKS));

    assert_eq!(plan.roots.len(), 1);
    assert!(
        plan.roots[0].prune.is_empty(),
        "no walk reaches a firmlinked directory through its firmlink"
    );
    assert!(
        plan.roots[0].hide.is_empty(),
        "no walk reaches a firmlinked directory through its firmlink"
    );
}

/// The walk of a planned tree finds each file one time. A firmlink cannot be
/// made in a test, thus each firmlinked directory of the data volume holds a
/// copy of what its firmlink shows, as the two paths of a real Mac show one
/// directory.
#[test]
fn a_walk_of_the_plan_finds_each_file_once_and_each_file_without_a_firmlink() {
    let tree = Tree::new("plan-walk");
    let system = tree.root();
    let expected = vec![
        tree.file("Users/me/120.mp4"),
        tree.file("System/Library/Caches/120.mp4"),
        tree.file("System/Volumes/Data/mnt/120.mp4"),
        tree.file("System/Volumes/Data/sw/120.mp4"),
        tree.file("System/Volumes/Data/MobileSoftwareUpdate/120.mp4"),
        tree.file("System/Volumes/Data/.PreviousSystemInformation/120.mp4"),
        tree.file("System/Volumes/Data/System/Library/NoFirmlink/120.mp4"),
    ];
    tree.file("System/Volumes/Data/Users/me/120.mp4");
    tree.file("System/Volumes/Data/System/Library/Caches/120.mp4");
    let table = [
        Mount::new(system, "apfs"),
        Mount::new(system.join("System/Volumes/Data"), "apfs"),
    ];
    let firmlinks =
        Firmlinks::parse("/Users\tUsers\n/System/Library/Caches\tSystem/Library/Caches\n");
    let plan = plan_machine(system, &table, &firmlinks);

    let found = Found::default();
    let report = walk(
        &RealFs,
        &plan.roots,
        Options::default(),
        &Matcher::glob("120.mp4", Case::Insensitive).expect("a valid glob"),
        found.sink(),
    );

    assert_eq!(found.sorted(), sorted(expected));
    assert!(report.is_complete(), "{report:?}");
}

/// A firmlinked directory is one directory, thus the walk of the plan
/// prints it one time, through its firmlink.
#[test]
fn a_walk_of_the_plan_prints_a_firmlinked_directory_once() {
    let tree = Tree::new("plan-firmlinked-name");
    let system = tree.root();
    tree.file("Users/me/120.mp4");
    tree.file("System/Library/Caches/120.mp4");
    tree.file("System/Volumes/Data/Users/me/120.mp4");
    tree.file("System/Volumes/Data/System/Library/Caches/120.mp4");
    let table = [
        Mount::new(system, "apfs"),
        Mount::new(system.join("System/Volumes/Data"), "apfs"),
    ];
    let firmlinks =
        Firmlinks::parse("/Users\tUsers\n/System/Library/Caches\tSystem/Library/Caches\n");
    let plan = plan_machine(system, &table, &firmlinks);

    let found = Found::default();
    let report = walk(
        &RealFs,
        &plan.roots,
        Options::default(),
        &Matcher::regex("^(Users|Caches)$", Case::Sensitive).expect("a valid regex"),
        found.sink(),
    );

    assert_eq!(
        found.sorted(),
        sorted(vec![
            system.join("Users"),
            system.join("System/Library/Caches"),
        ])
    );
    assert!(report.is_complete(), "{report:?}");
}

/// The walk of the volume above a mount point reads it as an entry, and
/// matches it there. The root of the mount point does not match it again.
#[test]
fn a_walk_of_the_plan_prints_each_mount_point_once() {
    let tree = Tree::new("plan-mount-name");
    let system = tree.root();
    tree.file("Users/me/120.mp4");
    tree.file("System/Library/Caches/120.mp4");
    tree.file("System/Volumes/Data/mnt/120.mp4");
    tree.file("System/Volumes/Data/sw/120.mp4");
    tree.file("System/Volumes/Data/MobileSoftwareUpdate/120.mp4");
    tree.file("System/Volumes/Data/.PreviousSystemInformation/120.mp4");
    tree.file("System/Volumes/Data/System/Library/NoFirmlink/120.mp4");
    tree.file("System/Volumes/Data/Users/me/120.mp4");
    tree.file("System/Volumes/Data/System/Library/Caches/120.mp4");
    let table = [
        Mount::new(system, "apfs"),
        Mount::new(system.join("System/Volumes/Data"), "apfs"),
    ];
    let firmlinks =
        Firmlinks::parse("/Users\tUsers\n/System/Library/Caches\tSystem/Library/Caches\n");
    let plan = plan_machine(system, &table, &firmlinks);

    let found = Found::default();
    let report = walk(
        &RealFs,
        &plan.roots,
        Options::default(),
        &Matcher::regex("^Data$", Case::Sensitive).expect("a valid regex"),
        found.sink(),
    );

    assert_eq!(found.sorted(), vec![system.join("System/Volumes/Data")]);
    assert!(report.is_complete(), "{report:?}");
}

#[test]
fn a_named_root_crosses_each_mount_point_without_one_file_system() {
    let table = [Mount::new("/data/sub/mnt", "apfs")];
    let root = ff::plan::named_root(Path::new("./sub"), Path::new("/data/sub"), false, &table);

    assert_eq!(
        root,
        ff::walk::WalkRoot {
            canonical: PathBuf::from("/data/sub"),
            ..ff::walk::WalkRoot::new("./sub")
        }
    );
}

#[test]
fn a_named_root_with_one_file_system_stays_on_its_device_and_prunes_each_mount_point_below_it() {
    let table = [
        Mount::new("/", "apfs"),
        Mount::new("/data/sub", "apfs"),
        Mount::new("/data/sub/mnt", "apfs"),
        Mount::new("/data/sub/deep/er", "nfs"),
        Mount::new("/data/other", "apfs"),
        Mount::new("/data/subway", "apfs"),
    ];
    let root = ff::plan::named_root(Path::new("./sub"), Path::new("/data/sub"), true, &table);

    assert_eq!(root.path, PathBuf::from("./sub"));
    assert_eq!(root.canonical, PathBuf::from("/data/sub"));
    assert!(root.stay_on_device);
    assert!(root.named);
    assert_eq!(root.prune, paths(&["./sub/deep/er", "./sub/mnt"]));
    assert!(root.hide.is_empty());
}
