//! One entry the scan cannot read is not a reason to abandon the tree.
//!
//! `workit` walks a whole directory tree, and a tree of any size holds things
//! it cannot read: a directory whose mode keeps it out, a `Cargo.toml` that
//! nobody can parse. Both used to end the walk, so a single unreadable entry
//! anywhere under the search path threw away every package the scan had
//! already found and reported nothing about them. A scan of a home directory
//! or of a shared tree meets such an entry routinely.
//!
//! The rule under test: each such entry is named on stderr and skipped, the
//! walk carries on, and the total is stated once at the end. The one failure
//! left is a scan that skipped something and found nothing — it never answered
//! the question it was asked, so it must not report an empty tree and succeed.
//!
//! Every test here builds its own tree in its own temporary directory, runs
//! the binary over that tree with `--dry-run`, and points `--output` inside the
//! fixture. Nothing is written and nothing outside the temporary directory is
//! read, so two copies of this file can run at the same time and neither one
//! can reach a real repository.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

/// A `Cargo.toml` no parser accepts.
const MALFORMED_MANIFEST: &str = "this is not = valid toml [[[\n";

/// A temporary directory and its canonical path.
///
/// The path is canonical because `workit` subtracts canonical paths to build a
/// member, and a temporary directory on macOS is reached through a symbolic
/// link. The `TempDir` is returned with it: it deletes the tree when it drops,
/// so the caller has to keep it alive.
fn fixture() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("a temporary directory is created");
    let root = fs::canonicalize(temp.path()).expect("the temporary directory has a canonical path");
    (temp, root)
}

/// Writes `content` as the `Cargo.toml` of the directory `relative` under
/// `root`, and returns the path of the file it wrote.
fn manifest_at(root: &Path, relative: &str, content: &str) -> PathBuf {
    let dir = root.join(relative);
    fs::create_dir_all(&dir).expect("the fixture directory is created");
    let manifest = dir.join("Cargo.toml");
    fs::write(&manifest, content).expect("the fixture manifest is written");
    manifest
}

/// Writes a well-formed package manifest at `relative` under `root`, so the
/// walk has something to find there. Every package in a workspace needs a name
/// of its own, so each call states one.
fn package_at(root: &Path, relative: &str, name: &str) {
    manifest_at(
        root,
        relative,
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    );
}

/// Runs the binary over `root` and hands back what it did, exit status
/// included: what a scan does when it meets an entry it cannot read is the
/// thing under test, so no run here may assume it succeeded.
///
/// `--dry-run` keeps it from writing a manifest, and the output path names a
/// file inside the fixture, so no run can touch a manifest of the repository it
/// was started from.
fn workit(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workit"))
        .current_dir(root)
        .arg("--path")
        .arg(root)
        .arg("--output")
        .arg(root.join("workspace-manifest.toml"))
        .arg("--dry-run")
        .output()
        .expect("the workit binary runs")
}

/// The members the run printed, in the order it printed them.
fn members_of(output: &Output) -> Vec<String> {
    let printed = std::str::from_utf8(&output.stdout).expect("workit writes UTF-8");
    printed
        .lines()
        .filter_map(|line| line.strip_prefix("  - "))
        .map(str::to_string)
        .collect()
}

/// What the run said on stderr.
fn warnings_of(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("workit writes UTF-8")
}

/// Asserts the run succeeded, and says what it printed when it did not.
fn assert_succeeded(output: &Output) {
    assert!(
        output.status.success(),
        "workit exited with {}, stderr: {}",
        output.status,
        warnings_of(output)
    );
}

#[test]
fn a_malformed_manifest_does_not_stop_the_scan() {
    let (_temp, root) = fixture();
    package_at(&root, "good", "workit-fixture-malformed-good");
    manifest_at(&root, "broken", MALFORMED_MANIFEST);

    let output = workit(&root);

    assert_succeeded(&output);
    assert_eq!(
        members_of(&output),
        vec!["good".to_string()],
        "one manifest nobody can parse is a reason to skip that file, not the tree around it"
    );
}

#[test]
fn the_warning_for_a_malformed_manifest_names_the_file() {
    let (_temp, root) = fixture();
    package_at(&root, "good", "workit-fixture-named-good");
    let broken = manifest_at(&root, "broken", MALFORMED_MANIFEST);

    let warnings = warnings_of(&workit(&root));

    assert!(
        warnings.contains(&broken.display().to_string()),
        "a parse failure that names no file leaves the reader to find it: {warnings}"
    );
}

#[test]
fn an_unreadable_manifest_does_not_stop_the_scan() {
    let (_temp, root) = fixture();
    package_at(&root, "good", "workit-fixture-unreadable-manifest-good");
    let locked = manifest_at(&root, "locked", "[package]\nname = \"locked\"\n");
    let Some(_guard) = Unreadable::claim(locked.clone()) else {
        eprintln!("skipping: this run can read a file of mode 000, so the test measures nothing");
        return;
    };

    let output = workit(&root);

    assert_succeeded(&output);
    assert_eq!(
        members_of(&output),
        vec!["good".to_string()],
        "a manifest the run cannot open is skipped like one it cannot parse"
    );
    assert!(
        warnings_of(&output).contains(&locked.display().to_string()),
        "the warning names the file that could not be read"
    );
}

#[test]
fn an_unreadable_directory_does_not_stop_the_scan() {
    let (_temp, root) = fixture();
    package_at(&root, "good", "workit-fixture-unreadable-directory-good");
    let locked = root.join("locked");
    fs::create_dir_all(&locked).expect("the fixture directory is created");
    let Some(_guard) = Unreadable::claim(locked.clone()) else {
        eprintln!(
            "skipping: this run can read a directory of mode 000, so the test measures nothing"
        );
        return;
    };

    let output = workit(&root);

    assert_succeeded(&output);
    assert_eq!(
        members_of(&output),
        vec!["good".to_string()],
        "the package the scan already found is still the answer"
    );
    assert!(
        warnings_of(&output).contains(&locked.display().to_string()),
        "the warning names the directory that could not be read"
    );
}

#[test]
fn the_scan_says_how_many_entries_it_skipped() {
    let (_temp, root) = fixture();
    package_at(&root, "good", "workit-fixture-counted-good");
    manifest_at(&root, "broken-one", MALFORMED_MANIFEST);
    manifest_at(&root, "broken-two", MALFORMED_MANIFEST);

    let output = workit(&root);

    assert_succeeded(&output);
    let warnings = warnings_of(&output);
    assert!(
        warnings.contains("Skipped 2"),
        "a scan that skipped something must not read afterwards like a scan that read everything: {warnings}"
    );
}

#[test]
fn a_scan_that_read_the_whole_tree_and_found_nothing_succeeds() {
    let (_temp, root) = fixture();
    fs::create_dir_all(root.join("empty")).expect("the fixture directory is created");

    let output = workit(&root);

    assert_succeeded(&output);
    assert!(
        !warnings_of(&output).contains("Skipped"),
        "a tree that holds no package is an answer, not a failure"
    );
}

#[test]
fn a_scan_that_skipped_everything_and_found_nothing_fails() {
    let (_temp, root) = fixture();
    manifest_at(&root, "broken", MALFORMED_MANIFEST);

    let output = workit(&root);
    let warnings = warnings_of(&output);

    assert!(
        !output.status.success(),
        "a scan that could not read the one thing it was pointed at never answered the question, \
         so it must not report an empty tree and succeed: {warnings}"
    );
    assert!(
        warnings.contains("Skipped 1"),
        "it fails for the reason it met, and says how many entries it never read: {warnings}"
    );
}

/// A path nobody can read, restored when it drops.
///
/// `TempDir` deletes its tree on drop and cannot delete what it cannot enter,
/// so the mode has to go back even when an assertion panics first.
struct Unreadable(PathBuf);

impl Unreadable {
    /// Takes the permissions of `path` away.
    ///
    /// Answers `None` when the run can still read it afterwards: root ignores
    /// the mode, and a test that measures nothing must say so rather than fail
    /// for a reason that has nothing to do with the code.
    fn claim(path: PathBuf) -> Option<Self> {
        Self::set_mode(&path, 0o000);
        let guard = Self(path);
        let readable = if guard.0.is_dir() {
            fs::read_dir(&guard.0).is_ok()
        } else {
            fs::read_to_string(&guard.0).is_ok()
        };
        if readable {
            None
        } else {
            Some(guard)
        }
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .expect("the fixture entry takes a new mode");
    }

    #[cfg(not(unix))]
    fn set_mode(_path: &Path, _mode: u32) {
        // Windows has no mode to take away; `claim` then finds the path
        // readable and the test that asked for it says it measured nothing.
    }
}

impl Drop for Unreadable {
    fn drop(&mut self) {
        Self::set_mode(&self.0, 0o755);
    }
}
