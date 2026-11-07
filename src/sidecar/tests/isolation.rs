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

/// The escape byte that starts each ANSI color code.
const ESCAPE: char = '\u{1b}';

/// The variables that ask a program to paint its output.
const FORCE_COLOR_VARIABLES: [(&str, &str); 3] = [
    ("CARGO_TERM_COLOR", "always"),
    ("FORCE_COLOR", "1"),
    ("CLICOLOR_FORCE", "1"),
];

/// Writes `<root>/home/color.gitconfig`. It holds the fixture configuration and
/// forces color for every git command that reads it. Returns its path.
fn write_color_gitconfig(fixture: &Fixture) -> PathBuf {
    let path = fixture.root().join("home").join("color.gitconfig");
    let plain = fs::read_to_string(fixture.gitconfig()).unwrap();
    fixture.write(
        &path,
        &format!(
            "{plain}[color]\n\tui = always\n\tbranch = always\n\tstatus = always\n\tdiff = always\n"
        ),
    );
    path
}

#[test]
fn parsed_git_output_is_the_same_under_forced_color() {
    let fixture = Fixture::new(&[NAME]);
    let color_config = write_color_gitconfig(&fixture);
    let color_config = color_config.to_string_lossy().into_owned();
    let mut colored_env = FORCE_COLOR_VARIABLES.to_vec();
    colored_env.push(("GIT_CONFIG_GLOBAL", &color_config));
    let host = fixture.main_checkout();
    let setup = fixture.run(&host, &["setup"]);
    assert_eq!(setup.code, 0, "{setup:?}");
    for args in [&["status"][..], &["path", NAME][..]] {
        let plain = fixture.run(&host, args);
        let colored = fixture.run_with_env(&host, args, &colored_env);
        assert_eq!(plain.code, 0, "{plain:?}");
        assert_eq!(colored.code, 0, "{colored:?}");
        assert_eq!(plain.stdout, colored.stdout, "sidecar {args:?}");
        assert!(!colored.stdout.contains(ESCAPE), "{colored:?}");
    }
    let worktree = fixture.run_with_env(&host, &["worktree", NAME, "feature"], &colored_env);
    assert_eq!(worktree.code, 0, "{worktree:?}");
    assert!(!worktree.stdout.contains(ESCAPE), "{worktree:?}");
    assert!(worktree.stdout.contains("(branch feature)"), "{worktree:?}");
    let target = fixture.worktrees_dir(NAME).join("feature");
    assert_eq!(
        fs::canonicalize(fixture.link(&host, NAME)).unwrap(),
        fs::canonicalize(target).unwrap()
    );
    let status = fixture.run_with_env(&host, &["status"], &colored_env);
    assert_eq!(status.code, 0, "{status:?}");
    assert!(!status.stdout.contains(ESCAPE), "{status:?}");
    assert!(
        status.stdout.contains("at a worktree on branch feature"),
        "{status:?}"
    );
}

#[test]
fn no_test_touches_the_network() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    let command = fixture.sidecar(&host, &["status"]);
    let path = command
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value)
        .expect("the fixture sets PATH");
    let first = std::env::split_paths(path)
        .next()
        .expect("PATH has an entry");
    assert_eq!(first, fixture.root().join("bin"));
    assert!(first.join("gh").is_file(), "the fake gh is missing");
    let setup = fixture.run(&host, &["setup"]);
    assert_eq!(setup.code, 0, "{setup:?}");
    for repository in [host, fixture.main_clone(NAME)] {
        let url = fixture.git(&repository, &["remote", "get-url", "origin"]);
        assert!(Path::new(&url).is_absolute(), "{url} is not a local path");
        assert!(Path::new(&url).exists(), "{url} does not exist");
        assert!(url.starts_with(&*fixture.root().to_string_lossy()), "{url}");
    }
}
