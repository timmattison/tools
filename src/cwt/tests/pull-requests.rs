//! The list that `cwt` prints shows the numbers of the open pull requests of
//! each branch after the branch. These tests run the real binary against a stub
//! `gh` that they put first on `PATH`, so no test ever talks to GitHub.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

use support::{add_worktree, git, make_repo, scrub_git_env, stdout};

/// An owner that cannot exist on GitHub, so a mistake in a test can never reach
/// a real repository.
const OWNER: &str = "cwt-test-owner";

/// The `--repo` value that `cwt` gives `gh` for the repository `one`.
const ONE: &str = "github.com/cwt-test-owner/one";

/// The `--repo` value for the repository `fam` of the family tests.
const FAM: &str = "github.com/cwt-test-owner/fam";

/// The `--repo` value for the repository `kid` of the family tests.
const KID: &str = "github.com/cwt-test-owner/kid";

/// The time limit that a run gets unless the test states its own, in seconds.
/// The machine is often heavily loaded, and a stub that answers must never lose
/// to the time limit.
const GENEROUS_LIMIT: &str = "120";

/// The text of the stub `gh`. `@DIR@` becomes the directory of the stub.
///
/// The behavior for each repository comes from files in that directory. The
/// name of a file starts with the `--repo` value, with each `/` replaced by `_`.
/// Every wait is bounded, and the hang is an `exec` so that the kill of `cwt`
/// reaches the `sleep` and not a shell that stands in front of it. The stub cuts
/// the answer to `--limit` lines like the real `gh`, whose default is 30.
const STUB_SCRIPT: &str = r#"#!/bin/sh
dir='@DIR@'
: > "$dir/ran"
printf '%s\n' "$@" > "$dir/args"
env > "$dir/env"
repo=''; limit=30
while [ $# -gt 0 ]; do
  case "$1" in
    --repo) repo=$2; shift ;;
    --limit) limit=$2; shift ;;
  esac
  shift
done
key=$(printf '%s' "$repo" | tr '/' '_')
: > "$dir/$key.started"
if [ -f "$dir/$key.wait-for" ]; then
  other=$(cat "$dir/$key.wait-for")
  i=0
  while [ ! -f "$dir/$other.started" ] && [ "$i" -lt 600 ]; do sleep 0.1; i=$((i + 1)); done
  if [ ! -f "$dir/$other.started" ]; then echo "the lookup of $other never started" >&2; exit 1; fi
fi
if [ -f "$dir/$key.hang" ]; then exec sleep 120; fi
if [ -f "$dir/$key.fail" ]; then cat "$dir/$key.fail" >&2; exit 1; fi
if [ -f "$dir/$key.json" ]; then head -n "$limit" "$dir/$key.json"; echo ']'; exit 0; fi
echo "the stub has no answer for $repo" >&2; exit 1
"#;

/// A pull request that the stub reports: the branch, the owner of the
/// repository that holds the branch, and the number.
struct Pull {
    branch: String,
    owner: String,
    number: u64,
}

impl Pull {
    /// A pull request from a branch of the repository itself.
    fn own(branch: &str, number: u64) -> Self {
        Self {
            branch: branch.to_string(),
            owner: OWNER.to_string(),
            number,
        }
    }

    /// A pull request from a fork that `owner` holds.
    fn fork(branch: &str, owner: &str, number: u64) -> Self {
        Self {
            branch: branch.to_string(),
            owner: owner.to_string(),
            number,
        }
    }

    /// The entry as `gh pr list --json` prints it. Only `"` and `\` need an
    /// escape in JSON text, and multibyte text needs none.
    fn json(&self) -> String {
        format!(
            r#"{{"headRefName":"{}","headRepositoryOwner":{{"id":"X","login":"{}"}},"number":{}}}"#,
            escape_json(&self.branch),
            escape_json(&self.owner),
            self.number
        )
    }
}

/// Escapes the two characters that JSON text cannot hold as they are.
fn escape_json(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A stub `gh` in a directory of its own.
///
/// Every test makes its own, in a fresh temporary directory, so two copies of
/// the suite can run at the same time without sharing a file.
struct GhStub {
    dir: TempDir,
}

impl GhStub {
    /// Writes the script `gh` into a new directory.
    fn new() -> Self {
        let dir = TempDir::new().expect("failed to create the stub directory");
        let path = dir.path().canonicalize().expect("failed to canonicalize");
        let text = path.display().to_string();
        assert!(!text.contains('\''), "the stub path must not hold a quote");
        let script = STUB_SCRIPT.replace("@DIR@", &text);
        let file = path.join("gh");
        std::fs::write(&file, script).expect("failed to write the stub");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))
            .expect("failed to make the stub executable");
        Self { dir }
    }

    /// The directory of the stub.
    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The name that the files of `repo` start with.
    fn key(repo: &str) -> String {
        repo.replace('/', "_")
    }

    /// Writes a file that the stub reads for `repo`.
    fn put(&self, repo: &str, extension: &str, text: &str) {
        let name = format!("{}.{extension}", Self::key(repo));
        std::fs::write(self.path().join(name), text).expect("failed to write a stub file");
    }

    /// Makes the stub answer for `repo` with these pull requests, in this order.
    ///
    /// The file holds one entry on each line, so `head -n LIMIT` cuts it the
    /// way `gh` cuts a list: the first line opens the array and each other line
    /// starts with a comma.
    fn answer(&self, repo: &str, pulls: &[Pull]) {
        let mut text = String::new();
        if pulls.is_empty() {
            text.push_str("[\n");
        }
        for (index, pull) in pulls.iter().enumerate() {
            text.push_str(if index == 0 { "[" } else { "," });
            text.push_str(&pull.json());
            text.push('\n');
        }
        self.put(repo, "json", &text);
    }

    /// True when the stub ran at least once.
    fn ran(&self) -> bool {
        self.path().join("ran").exists()
    }

    /// The arguments of the last run, one in each line.
    fn args(&self) -> Vec<String> {
        std::fs::read_to_string(self.path().join("args"))
            .expect("the stub did not record its arguments")
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The environment of the last run, as `NAME=value` lines.
    fn env(&self) -> String {
        std::fs::read_to_string(self.path().join("env"))
            .expect("the stub did not record its environment")
    }

    /// Runs `cwt` in `dir` with the stub first on `PATH`.
    fn run(&self, dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        let path = prepend_to_path(self.path());
        run_cwt(dir, args, env, &path)
    }
}

/// The `PATH` of this test process with `first` in front of it.
fn prepend_to_path(first: &Path) -> OsString {
    let mut entries = vec![first.to_path_buf()];
    if let Some(path) = std::env::var_os("PATH") {
        entries.extend(std::env::split_paths(&path));
    }
    std::env::join_paths(entries).expect("failed to join PATH")
}

/// Runs the real `cwt` in `dir`.
///
/// The environment of the child is built here. The test process never calls
/// `set_var`, because the tests of one binary run on many threads.
fn run_cwt(dir: &Path, args: &[&str], env: &[(&str, &str)], path: &OsString) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cwt"));
    command
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env("PATH", path)
        .env("CWT_PR_TIMEOUT", GENEROUS_LIMIT);
    for (key, value) in env {
        command.env(key, value);
    }
    scrub_git_env(&mut command)
        .output()
        .expect("failed to run cwt")
}

/// Standard error of a `cwt` run, as visible glyphs.
fn stderr(output: &Output) -> String {
    testcolor::strip_ansi(&String::from_utf8_lossy(&output.stderr))
}

/// Gives the repository at `path` a GitHub `origin` of the owner [`OWNER`].
fn set_origin(path: &Path, name: &str) {
    let url = format!("git@github.com:{OWNER}/{name}.git");
    git(path, &["remote", "add", "origin", &url]);
}

/// A canonical path inside a new temporary directory, with the directory.
fn scratch() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("failed to create temp dir");
    let root = tmp.path().canonicalize().expect("failed to canonicalize");
    (tmp, root)
}

#[test]
fn a_branch_with_an_open_pull_request_shows_its_number() {
    let (_tmp, root) = scratch();
    let repo = root.join("one");
    make_repo(&repo, "main");
    add_worktree(&repo, "../one-worktrees/feature", "feature");
    set_origin(&repo, "one");

    let stub = GhStub::new();
    stub.answer(ONE, &[Pull::own("main", 547)]);
    let output = stub.run(&repo, &[], &[]);

    assert!(output.status.success(), "cwt failed: {}", stderr(&output));
    let listing = stdout(&output);
    let lines: Vec<&str> = listing.lines().collect();
    assert_eq!(
        lines,
        vec![
            format!("> {} [main] #547", repo.display()),
            format!("  {} [feature]", root.join("one-worktrees/feature").display()),
        ]
    );
}

#[test]
fn gh_gets_the_documented_arguments_and_the_gh_variables_of_the_user() {
    let (_tmp, root) = scratch();
    let repo = root.join("one");
    make_repo(&repo, "main");
    set_origin(&repo, "one");

    let stub = GhStub::new();
    stub.answer(ONE, &[]);
    let output = stub.run(&repo, &[], &[("GH_TOKEN", "cwt-test-token")]);

    assert!(output.status.success(), "cwt failed: {}", stderr(&output));
    assert_eq!(
        stub.args(),
        [
            "pr",
            "list",
            "--repo",
            ONE,
            "--state",
            "open",
            "--limit",
            "1000",
            "--json",
            "number,headRefName,headRepositoryOwner"
        ]
    );
    assert!(
        stub.env().lines().any(|line| line == "GH_TOKEN=cwt-test-token"),
        "GH_TOKEN must reach gh, but gh saw:\n{}",
        stub.env()
    );
}
