//! Where the walk stops: a pruned directory, a directory on another device,
//! and a File Provider directory.

#[path = "common/faulty.rs"]
mod faulty;
#[path = "common/tree.rs"]
mod tree;

use std::path::Path;

use faulty::{Call, Fault, Faulty};
use ff::fs::RealFs;
use ff::pattern::{Case, Matcher};
use ff::walk::{is_cloud_directory, walk, Options, WalkRoot};
use tree::{sorted, Found, Tree};

fn glob(pattern: &str) -> Matcher {
    Matcher::glob(pattern, Case::Insensitive).expect("a valid glob")
}

#[test]
fn a_pruned_directory_is_not_entered() {
    let tree = Tree::new("bounds-prune");
    let expected = vec![tree.file("kept/120.mp4")];
    tree.file("pruned/120.mp4");
    let mut root = WalkRoot::new(tree.root());
    root.prune.insert(tree.root().join("pruned"));

    let found = Found::default();
    let report = walk(
        &RealFs,
        &[root],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), expected);
    assert!(report.is_complete(), "{report:?}");
}

#[test]
fn a_walk_that_stays_on_its_device_does_not_enter_another_device() {
    let tree = Tree::new("bounds-device");
    let here = tree.file("here/120.mp4");
    let there = tree.file("mounted/120.mp4");
    let mounted = tree.root().join("mounted");
    let faulty = Faulty::new(RealFs, |call, path: &Path| {
        (call == Call::Device && path == mounted).then_some(Fault::Device(u64::MAX))
    });

    let stays = WalkRoot {
        stay_on_device: true,
        ..WalkRoot::new(tree.root())
    };
    let found = Found::default();
    let report = walk(
        &faulty,
        &[stays],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );
    assert_eq!(found.sorted(), vec![here.clone()]);
    assert!(report.is_complete(), "{report:?}");

    let crosses = WalkRoot::new(tree.root());
    let found = Found::default();
    let report = walk(
        &faulty,
        &[crosses],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );
    assert_eq!(found.sorted(), sorted(vec![here, there]));
    assert!(report.is_complete(), "{report:?}");
}

#[test]
fn the_file_provider_directories_are_skipped_by_default_and_named_in_the_report() {
    let tree = Tree::new("bounds-cloud");
    let expected = vec![tree.file("Users/me/Library/Other/120.mp4")];
    tree.file("Users/me/Library/CloudStorage/GoogleDrive-me@example.com/120.mp4");
    tree.file("Users/me/Library/CloudStorage/ProtonDrive-me@example.com-folder/120.mp4");
    tree.file("Users/me/Library/Mobile Documents/com~apple~CloudDocs/120.mp4");

    let found = Found::default();
    let report = walk(
        &RealFs,
        &[WalkRoot::new(tree.root())],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), expected);
    let library = tree.root().join("Users/me/Library");
    assert_eq!(
        report.cloud_skipped,
        vec![
            library.join("CloudStorage/GoogleDrive-me@example.com"),
            library.join("CloudStorage/ProtonDrive-me@example.com-folder"),
            library.join("Mobile Documents"),
        ]
    );
    assert!(report.is_complete(), "{report:?}");
}

#[test]
fn include_cloud_walks_the_file_provider_directories() {
    let tree = Tree::new("bounds-cloud-included");
    let expected = vec![
        tree.file("Users/me/Library/CloudStorage/GoogleDrive-me@example.com/120.mp4"),
        tree.file("Users/me/Library/Mobile Documents/com~apple~CloudDocs/120.mp4"),
    ];

    let found = Found::default();
    let report = walk(
        &RealFs,
        &[WalkRoot::new(tree.root())],
        Options {
            include_cloud: true,
        },
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), sorted(expected));
    assert!(report.cloud_skipped.is_empty(), "{report:?}");
}

#[test]
fn a_root_that_is_a_file_provider_directory_is_walked() {
    let tree = Tree::new("bounds-cloud-root");
    let drive = tree.dir("Library/CloudStorage/GoogleDrive-me@example.com");
    let expected = vec![tree.file("Library/CloudStorage/GoogleDrive-me@example.com/120.mp4")];

    let found = Found::default();
    let report = walk(
        &RealFs,
        &[WalkRoot::new(&drive)],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), expected);
    assert!(report.cloud_skipped.is_empty(), "{report:?}");
}

#[test]
fn the_cloud_rule_names_each_child_of_cloud_storage_and_mobile_documents() {
    let library = Path::new("/Users/me/Library");
    let storage = library.join("CloudStorage");
    assert!(is_cloud_directory(&storage, "GoogleDrive-me".as_ref()));
    assert!(is_cloud_directory(&storage, "Dropbox".as_ref()));
    assert!(is_cloud_directory(library, "Mobile Documents".as_ref()));
    assert!(!is_cloud_directory(library, "CloudStorage".as_ref()));
    assert!(!is_cloud_directory(library, "Caches".as_ref()));
    assert!(!is_cloud_directory(
        Path::new("/Users/me/Documents"),
        "Mobile Documents".as_ref()
    ));
}

#[test]
fn a_root_that_is_a_link_into_a_library_skips_the_file_provider_directories() {
    let tree = Tree::new("bounds-cloud-link");
    tree.file("Users/me/Library/CloudStorage/Dropbox/120.mp4");
    tree.file("Users/me/Library/Mobile Documents/com~apple~CloudDocs/120.mp4");
    tree.file("Users/me/Library/Other/120.mp4");
    let link = tree.root().join("lib");
    std::os::unix::fs::symlink(tree.root().join("Users/me/Library"), &link).expect("make a link");
    let canonical = std::fs::canonicalize(&link).expect("resolve the link");
    let root = ff::plan::named_root(&link, &canonical, false, &[]);

    let found = Found::default();
    let report = walk(
        &RealFs,
        &[root],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), vec![link.join("Other/120.mp4")]);
    assert_eq!(
        report.cloud_skipped,
        vec![
            link.join("CloudStorage/Dropbox"),
            link.join("Mobile Documents")
        ]
    );
    assert!(report.is_complete(), "{report:?}");
}
