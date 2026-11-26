//! A temporary tree of directories and files, and a sink that keeps what a
//! walk found.
//!
//! The name of each tree holds the process id and a nanosecond timestamp, thus
//! two runs of one test at the same time do not share a tree. The tree is
//! removed when it drops, also when a test fails.

#![allow(
    dead_code,
    reason = "each test binary includes this module and uses a different part of it"
)]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// A directory tree that is removed when it drops.
pub struct Tree {
    dir: tempfile::TempDir,
}

impl Tree {
    /// Makes an empty tree whose name holds `label`.
    pub fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let prefix = format!("ff-{label}-{}-{nanos}-", std::process::id());
        let dir = tempfile::Builder::new()
            .prefix(&prefix)
            .tempdir()
            .expect("make the temporary tree");
        Self { dir }
    }

    /// The top directory of the tree.
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Makes an empty file at `relative`, and each directory above it.
    pub fn file(&self, relative: &str) -> PathBuf {
        let path = self.root().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("make the parent directories");
        }
        std::fs::write(&path, b"").expect("write the file");
        path
    }

    /// Makes the directory at `relative`, and each directory above it.
    pub fn dir(&self, relative: &str) -> PathBuf {
        let path = self.root().join(relative);
        std::fs::create_dir_all(&path).expect("make the directory");
        path
    }
}

/// Counts each entry below `root`, directories included, with the standard
/// library. It does not follow a symbolic link. The walk must agree with it.
pub fn count_entries(root: &Path) -> u64 {
    let mut count = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("read a directory of the tree") {
            let entry = entry.expect("read an entry of the tree");
            count += 1;
            if entry.file_type().expect("read the entry type").is_dir() {
                pending.push(entry.path());
            }
        }
    }
    count
}

/// Keeps each path that a walk gives to its sink.
#[derive(Default)]
pub struct Found(Mutex<Vec<PathBuf>>);

impl Found {
    /// A sink for [`ff::walk::walk`] that keeps each path.
    pub fn sink(&self) -> impl Fn(&Path) -> io::Result<()> + Sync + '_ {
        |path: &Path| {
            self.0
                .lock()
                .expect("the lock of the found paths")
                .push(path.to_path_buf());
            Ok(())
        }
    }

    /// Each kept path, sorted. A path that the walk gave twice stays twice.
    pub fn sorted(&self) -> Vec<PathBuf> {
        let mut paths = self.0.lock().expect("the lock of the found paths").clone();
        paths.sort();
        paths
    }
}

/// Sorts `paths`, to compare them with [`Found::sorted`].
pub fn sorted(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort();
    paths
}
