//! What the walk does when a call fails: each failure goes into one class of
//! the report, and no failure stops the walk of the other subtrees.

#[path = "common/faulty.rs"]
mod faulty;
#[path = "common/tree.rs"]
mod tree;

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use faulty::{errno, Call, Faulty, UnknownTypes};
use ff::fs::RealFs;
use ff::pattern::{Case, Matcher};
use ff::walk::{walk, Options, WalkRoot};
use tree::{sorted, Found, Tree};

fn glob(pattern: &str) -> Matcher {
    Matcher::glob(pattern, Case::Insensitive).expect("a valid glob")
}

/// Takes all access away from a directory, and gives it back when it drops,
/// thus the tree can be removed also when a test fails.
struct Locked(PathBuf);

impl Locked {
    fn new(path: PathBuf) -> Self {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
            .expect("lock the directory");
        Self(path)
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn a_directory_that_the_system_refuses_is_denied_and_the_walk_stays_complete() {
    // SAFETY: `geteuid` reads the effective user id, and it cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("root opens a directory with mode 000, thus this test proves nothing as root");
        return;
    }
    let tree = Tree::new("errors-denied");
    let visible = tree.file("open/120.mp4");
    tree.file("locked/120.mp4");
    let _locked = Locked::new(tree.root().join("locked"));

    let found = Found::default();
    let report = walk(
        &RealFs,
        &[WalkRoot::new(tree.root())],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), vec![visible]);
    assert_eq!(report.denied, vec![tree.root().join("locked")]);
    assert!(report.is_complete(), "{report:?}");
}

#[test]
fn a_root_that_does_not_exist_is_incomplete() {
    let tree = Tree::new("errors-missing-root");
    let missing = tree.root().join("missing");

    let report = walk(
        &RealFs,
        &[WalkRoot::new(&missing)],
        Options::default(),
        &glob("120.mp4"),
        Found::default().sink(),
    );

    assert_eq!(report.incomplete.len(), 1, "{report:?}");
    assert_eq!(report.incomplete[0].path, missing);
    assert_eq!(report.incomplete[0].error.kind(), io::ErrorKind::NotFound);
}

#[test]
fn a_root_that_is_a_file_matches_by_its_name() {
    let tree = Tree::new("errors-file-root");
    let file = tree.file("120.mp4");
    let other = tree.file("other.txt");

    let found = Found::default();
    let report = walk(
        &RealFs,
        &[WalkRoot::new(&file), WalkRoot::new(&other)],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), vec![file]);
    assert_eq!(report.matches, 1);
    assert!(report.is_complete(), "{report:?}");
}

#[test]
fn a_read_error_marks_its_directory_incomplete_and_the_other_subtrees_stay_whole() {
    let tree = Tree::new("errors-read");
    let expected = vec![
        tree.file("good/120.mp4"),
        tree.file("also-good/deep/120.mp4"),
    ];
    tree.file("bad/120.mp4");
    let bad = tree.root().join("bad");
    let faulty = Faulty::new(RealFs, |call, path: &Path| {
        (call == Call::Read && path == bad).then(|| errno(libc::EIO))
    });

    let found = Found::default();
    let report = walk(
        &faulty,
        &[WalkRoot::new(tree.root())],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), sorted(expected));
    assert_eq!(report.incomplete.len(), 1, "{report:?}");
    assert_eq!(report.incomplete[0].path, tree.root().join("bad"));
    assert_eq!(report.incomplete[0].error.raw_os_error(), Some(libc::EIO));
}

#[test]
fn a_directory_that_vanishes_during_the_walk_is_not_a_failure() {
    let tree = Tree::new("errors-vanished");
    let expected = vec![tree.file("stays/120.mp4")];
    tree.file("goes/120.mp4");
    tree.file("turns-into-a-file/120.mp4");
    let goes = tree.root().join("goes");
    let turns = tree.root().join("turns-into-a-file");
    let faulty = Faulty::new(RealFs, |call, path: &Path| {
        if call != Call::Open {
            return None;
        }
        if path == goes {
            Some(errno(libc::ENOENT))
        } else if path == turns {
            Some(errno(libc::ENOTDIR))
        } else {
            None
        }
    });

    let found = Found::default();
    let report = walk(
        &faulty,
        &[WalkRoot::new(tree.root())],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), expected);
    assert!(report.is_complete(), "{report:?}");
    assert!(report.denied.is_empty(), "{report:?}");
}

#[test]
fn an_entry_whose_type_cannot_be_read_is_incomplete() {
    let tree = Tree::new("errors-type");
    tree.file("dir/120.mp4");
    let dir = tree.root().join("dir");
    let faulty = UnknownTypes(Faulty::new(RealFs, |call, path: &Path| {
        (call == Call::IsDirectory && path == dir).then(|| errno(libc::EIO))
    }));

    let report = walk(
        &faulty,
        &[WalkRoot::new(tree.root())],
        Options::default(),
        &glob("120.mp4"),
        Found::default().sink(),
    );

    assert_eq!(report.incomplete.len(), 1, "{report:?}");
    assert_eq!(report.incomplete[0].path, tree.root().join("dir"));
}

#[test]
fn a_walk_with_unknown_entry_types_finds_the_same_matches() {
    let tree = Tree::new("errors-unknown-types");
    let expected = vec![tree.file("a/b/120.mp4"), tree.file("120.mp4")];

    let found = Found::default();
    let report = walk(
        &UnknownTypes(RealFs),
        &[WalkRoot::new(tree.root())],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), sorted(expected));
    assert!(report.is_complete(), "{report:?}");
}
