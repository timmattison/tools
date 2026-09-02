//! Pins that `nwt`'s shared test fixtures stay sandboxed from the ambient git
//! environment.
//!
//! When git invokes a hook it exports a whole family of `GIT_*` variables into
//! the hook's environment — the location vars (`GIT_DIR`, `GIT_WORK_TREE`,
//! `GIT_INDEX_FILE`, `GIT_PREFIX`), the identity vars (`GIT_AUTHOR_NAME` and
//! friends) and `GIT_CONFIG_PARAMETERS` — and anything that hook launches,
//! `cargo test` included, inherits them. `GIT_DIR` overrides cwd-based repo
//! discovery, so a fixture helper that only sets `current_dir(tempdir)` still
//! has its `git` child operate on the *real* repo.
//!
//! That is not hypothetical. It is how `user.email = t@example.com` /
//! `user.name = Test` (this suite's sibling fixture values over in `gsw`) came
//! to be written into this repo's own `.git/config`, which then silently
//! authored every subsequent commit as `Test <t@example.com>` until it was
//! noticed. A config write is sticky: one leak outlives the run that caused it.
//!
//! `.husky/pre-commit` already scrubs a few of these vars before `cargo test`,
//! but that is a single caller and a short list. This pins the guarantee at the
//! fixture itself — matching what `gsw`'s suite already does — so the sandbox
//! holds for any other runner (`bacon`, an IDE, a future hook or CI job) that
//! forgets the scrub.
//!
//! The guarantee under test is the whole `GIT_*` prefix, not a list of names, so
//! the probes below cover four distinct ways a leaked variable redirects git: a
//! config write, an index write, an object write, and injected config. A scrub
//! that names variables one at a time goes stale the day git adds another, and
//! a stale list reports exactly as clean as a working one.
//!
//! This file deliberately holds a SINGLE `#[test]`: it mutates the process-wide
//! environment, and cargo runs the tests within one test binary on parallel
//! threads. One test per binary means there is no sibling thread to race with,
//! while every other test file is a separate process with its own environment.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

use support::run_git;

/// The email probe A asks `run_git` to write.
///
/// Deliberately unique to this test and under the reserved `.invalid` TLD
/// (RFC 2606), so finding it in any config file is unambiguous evidence of a
/// leak rather than a pre-existing value someone legitimately configured.
const LEAK_MARKER: &str = "nwt-git-env-isolation-probe@example.invalid";

/// The config key probe D injects through `GIT_CONFIG_PARAMETERS`.
///
/// Namespaced under `nwt.` and unique to this test, so a `git config --get` that
/// finds it can only have read it out of the injected environment: no host
/// `~/.gitconfig`, `/etc/gitconfig` or fixture config defines it.
const INJECTED_KEY: &str = "nwt.envleakprobe";

/// The value probe D injects for [`INJECTED_KEY`].
const INJECTED_VALUE: &str = "leaked";

/// The file probe B stages into the fixture.
const PROBE_FILE: &str = "staged.txt";

/// The `GIT_*` variables the probes set, all aimed at the sentinel repo.
///
/// Removed as a set before any assertion runs so that a failing probe cannot
/// leave the process environment pointed at a scratch directory.
const PROBE_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_CONFIG_PARAMETERS",
];

/// Removes every inherited `GIT_*` variable from `cmd`'s child environment.
///
/// This is deliberately a private copy of the rule rather than a call to
/// `support::run_git`'s scrub or to
/// `gitscratch::shed_inherited_git_environment` itself: those are the code under
/// test, and a probe that borrows its sandbox from the code it is probing cannot
/// fail. Before the fix, that scrub is exactly what leaks.
///
/// The rule is the `GIT_` prefix, never a list of names — see this module's
/// header. Enumerating `std::env::vars_os()` means a variable git adds tomorrow
/// is swept without anyone editing this file.
fn scrub_inherited_git_env(cmd: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(&key);
        }
    }
}

/// Runs `git init` in `dir` with the whole inherited `GIT_*` family scrubbed.
///
/// # Panics
///
/// Panics if `git` cannot be spawned or `git init` reports failure.
fn init_sandboxed_repo(dir: &Path) {
    let mut cmd = Command::new("git");
    cmd.arg("init")
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    scrub_inherited_git_env(&mut cmd);

    let status = cmd.status().expect("invoke git init");
    assert!(status.success(), "git init failed in {}", dir.display());
}

/// Runs `git <args>` in `dir` with the whole inherited `GIT_*` family scrubbed,
/// returning its trimmed stdout.
///
/// Reading a repo's index needs `git`, and reading it through the helper under
/// test would defeat the point, so this carries the same private scrub as
/// [`init_sandboxed_repo`].
///
/// # Panics
///
/// Panics if `git` cannot be spawned, exits non-zero, or writes non-UTF-8.
fn sandboxed_git_stdout(dir: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    scrub_inherited_git_env(&mut cmd);

    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("invoke git {args:?} in {}: {e}", dir.display()));
    assert!(
        output.status.success(),
        "git {args:?} failed in {}",
        dir.display()
    );
    String::from_utf8(output.stdout)
        .expect("git stdout is valid UTF-8")
        .trim()
        .to_string()
}

/// Creates `<temp>/<name>` as a freshly initialised git repo, asserts git agrees
/// the repo landed there, and returns its path.
///
/// The landing assertion is HERMETIC-TESTS.md's "assert the tool landed" rule:
/// if this file's own scrub ever breaks, the failure surfaces here, loudly,
/// instead of letting the probes below run against the developer's real
/// repository and report something that reads like a different bug.
///
/// # Panics
///
/// Panics if the directory cannot be created, `git init` fails, or git reports a
/// top level other than the created directory.
fn sandboxed_repo(temp: &TempDir, name: &str) -> PathBuf {
    let repo = temp.path().join(name);
    fs::create_dir(&repo).expect("create repo dir");
    init_sandboxed_repo(&repo);

    let reported = sandboxed_git_stdout(&repo, &["rev-parse", "--show-toplevel"]);
    // Canonicalise both sides: on macOS a temp dir under `/var/...` is a symlink
    // to `/private/var/...`, and git reports the resolved path.
    let landed = fs::canonicalize(&reported).expect("canonicalize git's reported top level");
    let expected = fs::canonicalize(&repo).expect("canonicalize the fixture repo path");
    assert_eq!(
        landed,
        expected,
        "git init did not land in {}: git reports {reported} as the top level, so this \
         test's own setup is not sandboxed from the ambient git environment",
        repo.display()
    );

    repo
}

/// Reads a repo's local git config file as a string.
fn read_local_config(repo: &Path) -> String {
    fs::read_to_string(repo.join(".git").join("config")).expect("read local git config")
}

/// Counts the files (not directories) under a repo's `.git/objects`.
///
/// A fresh `git init` creates `objects/info` and `objects/pack` as *empty*
/// directories, so an untouched repo has a count of zero and establishing that
/// needs no `git` invocation at all — which is what makes this probe immune to
/// the leak it is measuring.
///
/// # Panics
///
/// Panics if the object store cannot be read.
fn object_file_count(repo: &Path) -> usize {
    fn count_files(dir: &Path) -> usize {
        let mut total = 0;
        for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
            let path = entry
                .unwrap_or_else(|e| panic!("read an entry of {}: {e}", dir.display()))
                .path();
            if path.is_dir() {
                total += count_files(&path);
            } else {
                total += 1;
            }
        }
        total
    }

    count_files(&repo.join(".git").join("objects"))
}

#[test]
fn run_git_ignores_the_ambient_git_environment() {
    let temp = TempDir::new().expect("create temp dir");
    // Both repos are built *before* any variable is set, so the fixtures
    // themselves cannot be redirected by the leak this test goes on to
    // reproduce. The sentinel stands in for the developer's real repo — the one
    // the inherited environment points at when the suite runs under a git hook.
    let sentinel = sandboxed_repo(&temp, "sentinel");
    // The repo `run_git` is explicitly pointed at, and the only one it may touch.
    let fixture = sandboxed_repo(&temp, "fixture");
    fs::write(fixture.join(PROBE_FILE), "probe\n").expect("write the probe file");

    // Reproduce the hook environment: every way git can be told "which
    // repository" from the environment, all aimed at the sentinel, while the
    // helper is handed an entirely different directory.
    std::env::set_var("GIT_DIR", sentinel.join(".git"));
    std::env::set_var("GIT_WORK_TREE", &sentinel);
    std::env::set_var("GIT_INDEX_FILE", sentinel.join(".git").join("index"));
    std::env::set_var(
        "GIT_OBJECT_DIRECTORY",
        sentinel.join(".git").join("objects"),
    );
    std::env::set_var(
        "GIT_CONFIG_PARAMETERS",
        format!("'{INJECTED_KEY}={INJECTED_VALUE}'"),
    );

    let wrote_config = run_git(&fixture, &["config", "user.email", LEAK_MARKER]);
    let staged = run_git(&fixture, &["add", PROBE_FILE]);
    // `git config --get` exits 0 only when the key resolves, so a `true` here
    // means the injected config reached the child.
    let injection_visible = run_git(&fixture, &["config", "--get", INJECTED_KEY]);

    // Put the environment back before reading anything or asserting: an
    // assertion that fires panics, and leaving these set would point everything
    // that ran afterwards at a temp dir that is about to be deleted.
    for key in PROBE_VARS {
        std::env::remove_var(key);
    }

    let sentinel_config = read_local_config(&sentinel);
    let fixture_config = read_local_config(&fixture);
    let sentinel_index = sandboxed_git_stdout(&sentinel, &["ls-files"]);
    let fixture_index = sandboxed_git_stdout(&fixture, &["ls-files"]);
    let sentinel_objects = object_file_count(&sentinel);
    let fixture_objects = object_file_count(&fixture);

    assert!(
        wrote_config,
        "the probe's `git config` invocation should succeed"
    );
    assert!(staged, "the probe's `git add` invocation should succeed");

    // Probe A — a config write. This is the leak that corrupted this repo.
    assert!(
        !sentinel_config.contains(LEAK_MARKER),
        "run_git wrote to the repo named by the ambient GIT_DIR instead of the \
         directory it was given, so a fixture running under a git hook would \
         corrupt the real repo's config.\nsentinel config:\n{sentinel_config}"
    );
    assert!(
        fixture_config.contains(LEAK_MARKER),
        "run_git must still write to the directory it was given — otherwise \
         this test could pass merely because the write went nowhere.\nfixture \
         config:\n{fixture_config}"
    );

    // Probe B — an index write. `GIT_INDEX_FILE` alone is enough to make a
    // fixture's `git add` stage that file into the real repo's index, and git
    // exports `GIT_INDEX_FILE` to every hook. `init_repo` calls
    // `run_git(&repo, &["add", "README.md"])`, so this is the exact shape.
    // `GIT_WORK_TREE` fails this probe too, by resolving the pathspec elsewhere.
    assert!(
        sentinel_index.is_empty(),
        "run_git staged into the index named by the ambient environment instead \
         of the fixture's, so a fixture running under a git hook would stage its \
         scratch files into the real repo.\nsentinel index:\n{sentinel_index}"
    );
    assert!(
        fixture_index.contains(PROBE_FILE),
        "run_git must still stage into the repo it was given — otherwise this \
         probe could pass merely because the staging went nowhere.\nfixture \
         index:\n{fixture_index}"
    );

    // Probe C — an object write. `GIT_OBJECT_DIRECTORY` passes straight through
    // a scrub of the three location vars and redirects every blob and tree a
    // fixture writes into a foreign repository's object store.
    assert_eq!(
        sentinel_objects, 0,
        "run_git wrote {sentinel_objects} object(s) into the object store named by \
         the ambient GIT_OBJECT_DIRECTORY — that is a write into another \
         repository, the exact class of damage this sandbox exists to stop"
    );
    assert!(
        fixture_objects > 0,
        "run_git must write its objects into the repo it was given — otherwise \
         this probe could pass merely because nothing was written anywhere"
    );

    // Probe D — injected config. Git exports `GIT_CONFIG_PARAMETERS` to every
    // pre-commit hook, and it can set *any* config key: `user.email`,
    // `core.bare`, `core.hooksPath`. It carries no path, so no location scrub
    // will ever catch it; only sweeping the whole `GIT_*` prefix does.
    assert!(
        !injection_visible,
        "run_git let the ambient GIT_CONFIG_PARAMETERS inject {INJECTED_KEY}=\
         {INJECTED_VALUE} into the fixture's git, so anything the launching \
         environment configures — user.email, core.bare, core.hooksPath — silently \
         applies to every fixture in this suite"
    );
}
