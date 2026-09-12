//! Pins that the `kitchen-sync` **binary** clones into the temporary directory
//! it made, whatever git the environment carries.
//!
//! `shallow_clone` is the one git child this tool spawns, and it is the whole
//! tool's entrance: every later step reads the clone. So a sweep that is lost
//! from that one call site aims the clone's index and its object writes at
//! whatever repository the environment names. On a real machine that repository
//! is the developer's own, because git exports its own variables into every hook
//! it runs — `GIT_INDEX_FILE` and `GIT_CONFIG_PARAMETERS` among them — and a
//! `kitchen-sync` started from a pre-commit hook inherits them.
//!
//! The gap this closes is narrow and worth stating. A spawn that sheds nothing
//! is not written in a shape a rule can name, so `repo_guards::git_env_sweep`
//! cannot report one and says so in its own documentation. It proves no call
//! site is written as a hand-written list of variable names. It does not prove
//! a given spawn sheds anything at all. The only thing that proves *that* is a
//! run of the binary in a hostile environment, with the damage measured on the
//! file system afterwards. The unit tests of `kitchen-sync` call `shallow_clone`
//! directly and read the files it produced, so they say nothing about where the
//! objects and the index of that clone were written. This file does.
//!
//! Every variable below is set on the **child command**, and nothing here
//! touches the environment of this process. Cargo runs the tests of one binary
//! on parallel threads, so a process-wide variable would aim the git children of
//! a sibling thread at this file's decoy.

// The binary root of this crate raises both of these, and a target root that
// says nothing about them is the silent exemption `repo_guards::target_lints`
// exists to refuse. This file holds the same position: it ends through the test
// harness, and it builds each report line with `format!` into a list, so it
// calls neither.
#![deny(clippy::exit, clippy::format_push_string)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gitscratch::shed_inherited_git_environment;
use tempfile::TempDir;

/// The message `kitchen-sync` prints when a workspace holds no binary package.
///
/// The tool reaches this step only after it clones the repository and reads the
/// manifests of the clone, so the message is the proof that the clone ran.
const NO_BINARY_PACKAGES: &str = "No binary packages found in repository";

/// The message `kitchen-sync` prints when the clone itself fails.
const CLONE_FAILED: &str = "git clone failed";

/// The files of the source repository: a Cargo workspace of one library-only
/// member.
///
/// `kitchen-sync` clones this repository, reads the root manifest, finds
/// `[workspace]`, finds no binary package in it, and stops. That is the cheap
/// path through the tool. The clone — the one thing under test — still runs, and
/// no `cargo install` ever starts.
const SOURCE_FILES: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nresolver = \"2\"\nmembers = [\"library\"]\n",
    ),
    (
        "library/Cargo.toml",
        "[package]\nname = \"library-only\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    ),
    ("library/src/lib.rs", "pub fn nothing() {}\n"),
];

/// The files of the decoy repository, which nothing may write to.
const DECOY_FILES: &[(&str, &str)] = &[("README.md", "baseline\n")];

/// What one path of a snapshot holds.
///
/// A directory carries no content of its own, a file carries its bytes, and a
/// link carries the path it names. Reading a link rather than following it keeps
/// the snapshot a statement about this directory alone.
#[derive(Debug, Eq, PartialEq)]
enum Held {
    Directory,
    File(Vec<u8>),
    Link(PathBuf),
}

/// Every path under one directory, with what each path holds.
///
/// A `BTreeMap` keys on the relative path and orders by it, so two snapshots of
/// one directory compare the same way on every run and a difference reads in
/// path order.
type Snapshot = BTreeMap<String, Held>;

/// Run a git command in `dir` and hand back whether it succeeded.
///
/// The whole inherited `GIT_` family is shed through
/// [`gitscratch::shed_inherited_git_environment`], which is what makes `dir` the
/// repository git acts on. `current_dir(dir)` alone is not enough: git obeys
/// `GIT_DIR` before it obeys the directory it was pointed at, so a run of this
/// suite from inside a pre-commit hook would put the fixture's commits into the
/// repository being committed to.
///
/// A fixture takes the blanket entrance rather than the production one beside
/// it, because a fixture has no user whose intent to honor. It builds a
/// throwaway repository from a local path, so a `GIT_SSH_COMMAND` it inherited
/// names a program it has no reason to run.
fn run_git(dir: &Path, args: &[&str]) -> bool {
    let mut command = Command::new("git");
    shed_inherited_git_environment(&mut command);

    command
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Build a throwaway git repository that holds `files`, and commit them once.
///
/// The repository is a subdirectory of the `TempDir`, and the caller keeps that
/// handle alive for as long as it reads the repository.
///
/// `maintenance.auto` is `false`, and that is load-bearing rather than
/// tidiness. `git commit` ends by starting `git maintenance run --auto`, which
/// detaches and takes `.git/objects/maintenance.lock` while it decides it has
/// nothing to do. The lock is there when `commit` returns on most runs, and it
/// is gone a few milliseconds later. So a test that reads the whole tree of this
/// repository sees a path that appears or vanishes on its own, and this file
/// reads exactly that to say the decoy came back byte-identical. Left on, a
/// detached git of the fixture's own making answers that question about one run
/// in five.
///
/// # Panics
///
/// Panics if the temporary directory cannot be made, if a file cannot be
/// written, or if any git command fails.
fn init_repo(files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("make a temp dir");
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).expect("make the repo subdir");

    assert!(run_git(&repo, &["init"]), "git init failed");
    assert!(
        run_git(&repo, &["config", "user.email", "test@example.com"]),
        "git config user.email failed"
    );
    assert!(
        run_git(&repo, &["config", "user.name", "Test User"]),
        "git config user.name failed"
    );
    assert!(
        run_git(&repo, &["config", "maintenance.auto", "false"]),
        "git config maintenance.auto failed"
    );

    for (name, contents) in files {
        let path = repo.join(name);
        let parent = path.parent().expect("every fixture file has a parent");
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("make {}: {e}", parent.display()));
        fs::write(&path, contents).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        assert!(run_git(&repo, &["add", name]), "git add {name} failed");
    }

    assert!(
        run_git(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "baseline"]
        ),
        "git commit failed"
    );

    (temp, repo)
}

/// The path of `path` under `root`, spelled with forward slashes.
///
/// # Panics
///
/// Panics if `path` does not lie under `root`.
fn relative_name(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or_else(|e| panic!("{} lies under {}: {e}", path.display(), root.display()))
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Record every path under `dir` into `into`, and descend into each directory.
///
/// # Panics
///
/// Panics if a directory cannot be read, or a file cannot be read back.
fn record(root: &Path, dir: &Path, into: &mut Snapshot) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry
            .unwrap_or_else(|e| panic!("read an entry of {}: {e}", dir.display()))
            .path();
        let name = relative_name(root, &path);

        // `symlink_metadata` reads the entry itself. `metadata` follows a link,
        // so a link that points outside the decoy would put another directory's
        // bytes into a snapshot of this one.
        let kind = fs::symlink_metadata(&path)
            .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
            .file_type();

        if kind.is_dir() {
            into.insert(name, Held::Directory);
            record(root, &path, into);
        } else if kind.is_symlink() {
            let target =
                fs::read_link(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            into.insert(name, Held::Link(target));
        } else {
            let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            into.insert(name, Held::File(bytes));
        }
    }
}

/// Every path under `root`, with the bytes of every file.
///
/// A count of refs or of objects is not enough to say a repository is untouched:
/// a write that replaces one object with another keeps the count. The whole
/// tree, content included, is what makes the word "byte-identical" true.
fn snapshot(root: &Path) -> Snapshot {
    let mut held = Snapshot::new();
    record(root, root, &mut held);
    held
}

/// How `after` differs from `before`, one line per path, in path order.
///
/// An empty list means the two snapshots are equal. Naming each path, and
/// whether it appeared, vanished or changed, is what turns a failure into
/// evidence of which command wrote where.
fn difference(before: &Snapshot, after: &Snapshot) -> Vec<String> {
    let mut lines = Vec::new();

    for (path, held) in after {
        match before.get(path) {
            None => lines.push(format!("appeared: {path}")),
            Some(was) if was != held => lines.push(format!("changed:  {path}")),
            Some(_) => {}
        }
    }

    for path in before.keys() {
        if !after.contains_key(path) {
            lines.push(format!("vanished: {path}"));
        }
    }

    lines.sort();
    lines
}

/// The binary must clone into the temporary directory it made, and leave the
/// repository the environment names exactly as it found it.
///
/// Git obeys the environment before it obeys the directory a command was pointed
/// at, and it exports its own variables into every hook it runs. So a
/// `kitchen-sync` run started from a pre-commit hook, from `git bisect run`, or
/// from `rebase --exec` carries `GIT_DIR`, `GIT_INDEX_FILE` and
/// `GIT_CONFIG_PARAMETERS` that name the repository being committed to. A clone
/// that keeps them acts on that repository instead.
///
/// The four variables below are aimed at a decoy repository, and cover three
/// distinct ways a leaked variable redirects git: the repository it finds
/// (`GIT_DIR`), the index it stages into (`GIT_INDEX_FILE`), the store it writes
/// objects to (`GIT_OBJECT_DIRECTORY`), and the configuration it reads
/// (`GIT_CONFIG_PARAMETERS`). The last one carries no path at all, so no list of
/// location names ever catches it; only a sweep of the whole prefix does.
///
/// `GIT_WORK_TREE` is deliberately not among them, and the reason is measured.
/// Aimed at the decoy's own working tree it makes `git clone` stop with `fatal:
/// working tree '<decoy>' already exists` **before** git writes anything. A
/// variable that stops the clone hides the damage the other three do, so the
/// assertion that reads the decoy would then pass under a missing sweep and the
/// test would prove only that a leaked environment breaks the tool. Git also
/// exports `GIT_INDEX_FILE` to every pre-commit hook and `GIT_WORK_TREE` to
/// none, so the set below is the set a real hook hands out.
///
/// The source repository is named by a `file://` URL, which is what the user
/// types and what the tool expects. It also matters here: git treats a plain
/// path as a local clone and hard-links the objects, so a leaked
/// `GIT_OBJECT_DIRECTORY` then makes the clone fail on a missing object instead
/// of writing into the store it names. Over `file://` git transfers a real pack,
/// and the leak lands that pack in the decoy.
///
/// Two assertions, and each one has a job. The decoy must be byte-identical,
/// which is the damage. The run must reach the step past the clone, which stops
/// the first assertion passing merely because the clone failed and the tool made
/// nothing anywhere.
#[test]
fn the_binary_leaves_the_repository_the_environment_names_untouched() {
    // Two repositories under two temporary directories. The source is the one
    // `kitchen-sync` clones and the only one it may read. The decoy stands in
    // for the repository a launching hook exported, which on a real machine is
    // the developer's own.
    let (source_temp, source) = init_repo(SOURCE_FILES);
    let (_decoy_temp, decoy) = init_repo(DECOY_FILES);

    let decoy_git_dir = decoy.join(".git");
    let before = snapshot(&decoy);

    let source_url = format!(
        "file://{}",
        source.to_str().expect("utf-8 source repository path")
    );

    // Every variable goes on the child, never on this process: a sibling test
    // thread spawns git of its own, and a process-wide variable would aim it
    // here. The working directory is the temporary directory above the source
    // repository, so no ambient repository can stand in for a shed variable.
    let output = Command::new(env!("CARGO_BIN_EXE_kitchen-sync"))
        .arg(&source_url)
        .current_dir(source_temp.path())
        .stdin(Stdio::null())
        .env("GIT_DIR", &decoy_git_dir)
        .env("GIT_INDEX_FILE", decoy_git_dir.join("index"))
        .env("GIT_OBJECT_DIRECTORY", decoy_git_dir.join("objects"))
        .env("GIT_CONFIG_PARAMETERS", "'kitchensync.envleakprobe=leaked'")
        .output()
        .expect("run the kitchen-sync binary");

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let after = snapshot(&decoy);

    // The damage. `git clone` writes a whole repository, and each leaked
    // variable sends one part of that write somewhere else: the pack of the
    // transfer into another object store, and the paths of the checkout into
    // another index.
    let changed = difference(&before, &after);
    assert!(
        changed.is_empty(),
        "kitchen-sync cloned into the repository the environment named instead of \
         the temporary directory it made. A run started from a git hook therefore \
         writes into the repository being committed to. {} path(s) of the decoy at \
         {} changed:\n{}\nkitchen-sync stdout:\n{stdout}\nkitchen-sync \
         stderr:\n{stderr}",
        changed.len(),
        decoy.display(),
        changed.join("\n"),
    );

    // The control against a vacuous pass. A clone that failed leaves the decoy
    // byte-identical too. The source repository holds one library-only member,
    // so the tool reads the manifest of the clone and stops with this message.
    // Nothing prints it unless the clone landed the files.
    assert!(
        stderr.contains(NO_BINARY_PACKAGES),
        "kitchen-sync must get past the clone and report \"{NO_BINARY_PACKAGES}\", \
         so that the assertion above covers a clone that really ran. It cloned \
         {source_url} and said this instead.\nkitchen-sync stdout:\n{stdout}\n\
         kitchen-sync stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(CLONE_FAILED),
        "the clone itself must succeed, and it reported a \
         failure:\nkitchen-sync stdout:\n{stdout}\nkitchen-sync stderr:\n{stderr}"
    );
}
