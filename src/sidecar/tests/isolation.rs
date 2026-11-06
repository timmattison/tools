#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
//! Safety and isolation of the `sidecar` tests: a decoy repository, forced color,
//! and the network.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use support::Fixture;

/// The name of the sidecar in these tests.
const NAME: &str = "testdata";

/// Every file under `dir` as a sorted map from relative path to bytes. The walk
/// follows no symbolic link. A symbolic link is recorded by its target text.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    collect(dir, dir, &mut files);
    files
}

/// Adds the files under `current` to `files`, with paths relative to `base`.
fn collect(base: &Path, current: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(current).expect("read the directory") {
        let entry = entry.expect("read the entry");
        let path = entry.path();
        let kind = entry.file_type().expect("read the file type");
        let relative = path
            .strip_prefix(base)
            .expect("under the base")
            .to_path_buf();
        if kind.is_symlink() {
            let target = fs::read_link(&path).expect("read the link");
            files.insert(relative, target.to_string_lossy().into_owned().into_bytes());
        } else if kind.is_dir() {
            files.insert(relative, Vec::new());
            collect(base, &path, files);
        } else {
            files.insert(relative, fs::read(&path).expect("read the file"));
        }
    }
}

/// The paths that differ between two snapshots: added, removed, or changed.
fn changed_paths(
    before: &BTreeMap<PathBuf, Vec<u8>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
) -> Vec<PathBuf> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Makes the decoy repository `<root>/decoy` with one commit and returns its path.
fn make_decoy(fixture: &Fixture) -> PathBuf {
    let decoy = fixture.root().join("decoy");
    fs::create_dir_all(&decoy).unwrap();
    fixture.git(&decoy, &["init", "--quiet", "-b", "main"]);
    // A commit starts a background maintenance run that makes a lock file later. The
    // run is the fixture own git, not the tool, thus the decoy turns it off.
    fixture.git(&decoy, &["config", "maintenance.auto", "false"]);
    fixture.git(&decoy, &["config", "gc.auto", "0"]);
    fixture.write(&decoy.join("decoy.txt"), "decoy\n");
    fixture.git(&decoy, &["add", "decoy.txt"]);
    fixture.git(&decoy, &["commit", "--quiet", "-m", "decoy"]);
    decoy
}

#[test]
fn each_command_passes_under_a_decoy_git_dir_and_the_decoy_stays_byte_identical() {
    let fixture = Fixture::new(&[NAME]);
    let decoy = make_decoy(&fixture);
    let before = snapshot(&decoy);
    let git_dir = decoy.join(".git");
    let index = git_dir.join("index");
    let env = [
        ("GIT_DIR", &*git_dir.to_string_lossy().into_owned()),
        ("GIT_INDEX_FILE", &*index.to_string_lossy().into_owned()),
    ];
    let host = fixture.add_host_worktree("host-branch");
    let commands: [&[&str]; 7] = [
        &["setup"],
        &["setup", "--hook"],
        &["status"],
        &["path", NAME],
        &["worktree", NAME, "decoy-branch"],
        &["worktree", NAME, "main"],
        &["add", "pics", "--yes"],
    ];
    for args in commands {
        let run = fixture.run_with_env(&host, args, &env);
        assert_eq!(run.code, 0, "sidecar {args:?}: {run:?}");
    }
    assert_eq!(
        changed_paths(&before, &snapshot(&decoy)),
        Vec::<PathBuf>::new()
    );
    assert_eq!(
        fs::canonicalize(fixture.link(&host, NAME)).unwrap(),
        fs::canonicalize(fixture.main_clone(NAME)).unwrap()
    );
    assert!(fixture.worktrees_dir(NAME).join("decoy-branch").is_dir());
    assert!(fixture.main_clone("pics").join(".git").exists());
    assert_eq!(
        fs::canonicalize(fixture.link(&host, "pics")).unwrap(),
        fs::canonicalize(fixture.main_clone("pics")).unwrap()
    );
    let registry = fs::read_to_string(host.join("sidecar.toml")).unwrap();
    assert!(registry.contains("\"pics\""), "{registry}");
}
