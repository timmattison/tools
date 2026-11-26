//! The walk does an interrupted call again in place, and loses nothing.
//!
//! These tests put `EINTR` between the walk and the real file system. A call
//! that gets the fault does not reach the kernel, as an interrupted system
//! call does nothing. A walk that treats `EINTR` as a fatal error thus loses
//! each subtree that an interruption reaches, and these tests fail against
//! it. `tests/sigusr1.rs` tells why a real signal cannot make this failure on
//! a local disk.

#[path = "common/faulty.rs"]
mod faulty;
#[path = "common/tree.rs"]
mod tree;

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use faulty::{errno, Call, Fault, Faulty, UnknownTypes};
use ff::fs::RealFs;
use ff::pattern::{Case, Matcher};
use ff::walk::{walk, Options, WalkRoot};
use tree::{count_entries, sorted, Found, Tree};

fn glob(pattern: &str) -> Matcher {
    Matcher::glob(pattern, Case::Insensitive).expect("a valid glob")
}

/// Interrupts the first, the third, the fifth, and each odd call of each
/// (call, path) pair. Each real call thus gets one interruption before it
/// runs, also each read in the middle of a directory.
#[derive(Default)]
struct EveryOtherCall {
    calls: Mutex<HashMap<(Call, PathBuf), u64>>,
    injected: AtomicU64,
}

impl EveryOtherCall {
    fn rule(&self, call: Call, path: &Path) -> Option<Fault> {
        let mut calls = self.calls.lock().expect("the lock of the call counts");
        let count = calls.entry((call, path.to_path_buf())).or_insert(0);
        *count += 1;
        if *count % 2 == 1 {
            self.injected.fetch_add(1, Ordering::Relaxed);
            Some(errno(libc::EINTR))
        } else {
            None
        }
    }

    fn injected(&self) -> u64 {
        self.injected.load(Ordering::Relaxed)
    }
}

/// Builds a tree with many directories, and plants a match in some of them.
fn planted_tree(label: &str) -> (Tree, Vec<PathBuf>) {
    let tree = Tree::new(label);
    for directory in 0..30 {
        for file in 0..10 {
            tree.file(&format!(
                "bulk/{}/{directory}/file-{file}.txt",
                directory % 4
            ));
        }
    }
    let planted = (0..8)
        .map(|index| tree.file(&format!("deep/a/b/c/{index}/120.mp4")))
        .chain([tree.file("120.mp4"), tree.file("bulk/1/120.mp4")])
        .collect();
    (tree, sorted(planted))
}

#[test]
fn each_interrupted_open_and_read_is_done_again_and_the_walk_loses_nothing() {
    let (tree, planted) = planted_tree("interrupt-open-read");
    let interrupter = EveryOtherCall::default();
    let faulty = Faulty::new(RealFs, |call, path: &Path| interrupter.rule(call, path));

    let found = Found::default();
    let report = walk(
        &faulty,
        &[WalkRoot::new(tree.root())],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), planted);
    assert_eq!(report.entries, count_entries(tree.root()));
    assert!(report.is_complete(), "{:?}", report.incomplete);
    assert!(interrupter.injected() > 100, "the faults reached the walk");
    assert_eq!(report.retried_interruptions, interrupter.injected());
}

#[test]
fn each_interrupted_device_read_is_done_again() {
    let (tree, planted) = planted_tree("interrupt-device");
    let interrupter = EveryOtherCall::default();
    let faulty = Faulty::new(RealFs, |call, path: &Path| interrupter.rule(call, path));
    let root = WalkRoot {
        stay_on_device: true,
        ..WalkRoot::new(tree.root())
    };

    let found = Found::default();
    let report = walk(
        &faulty,
        &[root],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), planted);
    assert!(report.is_complete(), "{:?}", report.incomplete);
    assert_eq!(report.retried_interruptions, interrupter.injected());
}

#[test]
fn each_interrupted_type_read_is_done_again() {
    let (tree, planted) = planted_tree("interrupt-type");
    let interrupter = EveryOtherCall::default();
    let faulty = UnknownTypes(Faulty::new(RealFs, |call, path: &Path| {
        interrupter.rule(call, path)
    }));

    let found = Found::default();
    let report = walk(
        &faulty,
        &[WalkRoot::new(tree.root())],
        Options::default(),
        &glob("120.mp4"),
        found.sink(),
    );

    assert_eq!(found.sorted(), planted);
    assert!(report.is_complete(), "{:?}", report.incomplete);
    assert_eq!(report.retried_interruptions, interrupter.injected());
}

#[test]
fn a_call_that_stays_interrupted_ends_the_walk_of_its_directory_as_incomplete() {
    let tree = Tree::new("interrupt-forever");
    let expected = vec![tree.file("fine/120.mp4")];
    tree.file("stuck/120.mp4");
    let stuck = tree.root().join("stuck");
    let faulty = Faulty::new(RealFs, |call, path: &Path| {
        (call == Call::Read && path == stuck).then(|| errno(libc::EINTR))
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
    assert_eq!(report.incomplete.len(), 1, "{report:?}");
    assert_eq!(report.incomplete[0].path, tree.root().join("stuck"));
    assert_eq!(
        report.incomplete[0].error.kind(),
        io::ErrorKind::Interrupted
    );
}
