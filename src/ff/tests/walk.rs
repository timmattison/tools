//! The walk, on real directory trees.

#[path = "common/tree.rs"]
mod tree;

use ff::fs::RealFs;
use ff::pattern::{Case, Matcher};
use ff::walk::{walk, Options, WalkRoot};
use tree::{count_entries, sorted, Found, Tree};

fn glob(pattern: &str) -> Matcher {
    Matcher::glob(pattern, Case::Insensitive).expect("a valid glob")
}

#[test]
fn the_walk_finds_each_matching_entry_once_and_counts_each_entry() {
    let tree = Tree::new("walk-finds");
    let expected = vec![
        tree.file("120.mp4"),
        tree.file("a/b/c/d/e/f/120.MP4"),
        tree.dir("x/120.mp4"),
        tree.file("x/120.mp4/inner/120.mp4"),
        tree.file("日本語/🎉/120.mp4"),
    ];
    for index in 0..200 {
        tree.file(&format!(
            "noise/{}/{}/file-{index}.txt",
            index % 7,
            index % 3
        ));
    }

    let found = Found::default();
    let report = walk(
        &RealFs,
        &[WalkRoot::new(tree.root())],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), sorted(expected));
    assert_eq!(report.matches, 5);
    assert_eq!(report.entries, count_entries(tree.root()));
    assert!(report.is_complete(), "{report:?}");
}

#[test]
fn the_walk_does_not_follow_a_symbolic_link_below_a_root() {
    let tree = Tree::new("walk-no-follow");
    let real = tree.file("real/120.mp4");
    std::os::unix::fs::symlink(tree.root().join("real"), tree.root().join("link"))
        .expect("make the link");

    let found = Found::default();
    let report = walk(
        &RealFs,
        &[WalkRoot::new(tree.root())],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), vec![real]);
    assert_eq!(report.entries, 3, "real, real/120.mp4, and link");
    assert!(report.is_complete(), "{report:?}");
}

#[test]
fn the_walk_follows_a_root_that_is_a_symbolic_link() {
    let tree = Tree::new("walk-root-link");
    tree.file("real/120.mp4");
    let link = tree.root().join("link");
    std::os::unix::fs::symlink(tree.root().join("real"), &link).expect("make the link");

    let found = Found::default();
    let report = walk(
        &RealFs,
        &[WalkRoot::new(&link)],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), vec![link.join("120.mp4")]);
    assert!(report.is_complete(), "{report:?}");
}

#[test]
fn the_walk_covers_each_root() {
    let first = Tree::new("walk-root-one");
    let second = Tree::new("walk-root-two");
    let expected = vec![first.file("one/120.mp4"), second.file("two/120.mp4")];

    let found = Found::default();
    let report = walk(
        &RealFs,
        &[WalkRoot::new(first.root()), WalkRoot::new(second.root())],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), sorted(expected));
    assert!(report.is_complete(), "{report:?}");
}
