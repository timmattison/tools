//! The list that `cwt` prints shows the numbers of the open pull requests of
//! each branch after the branch. These tests run the real binary against a stub
//! `gh` that they put first on `PATH`, so no test ever talks to GitHub.

// Mirrors the crate-root attributes in src/main.rs; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use std::ffi::{OsStr, OsString};
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
fn run_cwt(dir: &Path, args: &[&str], env: &[(&str, &str)], path: &OsStr) -> Output {
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
            format!(
                "  {} [feature]",
                root.join("one-worktrees/feature").display()
            ),
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
        stub.env()
            .lines()
            .any(|line| line == "GH_TOKEN=cwt-test-token"),
        "GH_TOKEN must reach gh, but gh saw:\n{}",
        stub.env()
    );
}

/// The lines of a listing, as visible glyphs.
fn lines_of(output: &Output) -> Vec<String> {
    stdout(output).lines().map(str::to_string).collect()
}

/// The one line of the listing that names `path`.
fn row_of(output: &Output, path: &Path) -> String {
    let shown = path.display().to_string();
    let rows: Vec<String> = lines_of(output)
        .into_iter()
        .filter(|line| line.contains(&format!("{shown} [")))
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "expected one row for {shown}, listing was:\n{}",
        stdout(output)
    );
    rows.into_iter().next().expect("one row")
}

/// The lines of standard error.
fn stderr_lines(output: &Output) -> Vec<String> {
    stderr(output).lines().map(str::to_string).collect()
}

/// A repository `one` on `main` with a GitHub origin, and a worktree on each of
/// `branches`. The worktree of the branch at position `N` of `branches` sits in
/// `one-worktrees/wN`.
fn one_repo_with(root: &Path, branches: &[&str]) -> PathBuf {
    let repo = root.join("one");
    make_repo(&repo, "main");
    for (index, branch) in branches.iter().enumerate() {
        add_worktree(&repo, &format!("../one-worktrees/w{index}"), branch);
    }
    set_origin(&repo, "one");
    repo
}

/// The family of the family tests: the repository `fam`, and the repository
/// `kid` one level below it. Both have a worktree on a branch `shared`.
///
/// Returns the directory of `fam`, the worktree of `fam`, and the worktree of
/// `kid`.
fn family_with_shared_branch(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let fam = root.join("fam");
    make_repo(&fam, "main");
    set_origin(&fam, "fam");
    add_worktree(&fam, "../fam-worktrees/shared", "shared");

    let kid = fam.join("kid");
    make_repo(&kid, "main");
    set_origin(&kid, "kid");
    add_worktree(&kid, "../kid-worktrees/shared", "shared");

    let fam_shared = root.join("fam-worktrees/shared");
    let kid_shared = fam.join("kid-worktrees/shared");
    (fam, fam_shared, kid_shared)
}

/// The path of the program `git` that this test process would find.
fn real_git() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH is not set");
    std::env::split_paths(&path)
        .map(|entry| entry.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("no git on PATH")
}

#[test]
fn two_pull_requests_of_one_branch_show_in_ascending_order() {
    let (_tmp, root) = scratch();
    let repo = one_repo_with(&root, &[]);

    let stub = GhStub::new();
    stub.answer(ONE, &[Pull::own("main", 519), Pull::own("main", 512)]);
    let output = stub.run(&repo, &[], &[]);

    assert_eq!(
        lines_of(&output),
        vec![format!("> {} [main] #512 #519", repo.display())]
    );
}

#[test]
fn a_detached_worktree_shows_no_number() {
    let (_tmp, root) = scratch();
    let repo = one_repo_with(&root, &[]);
    let detached = root.join("one-worktrees/detached");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            &detached.display().to_string(),
        ],
    );

    let stub = GhStub::new();
    stub.answer(ONE, &[Pull::own("main", 1)]);
    let output = stub.run(&repo, &[], &[]);

    assert!(row_of(&output, &repo).ends_with("[main] #1"));
    assert!(
        !row_of(&output, &detached).contains('#'),
        "a worktree with no branch has no number:\n{}",
        stdout(&output)
    );
}

#[test]
fn a_pull_request_from_a_fork_shows_no_number() {
    let (_tmp, root) = scratch();
    let repo = one_repo_with(&root, &[]);

    let stub = GhStub::new();
    stub.answer(ONE, &[Pull::fork("main", "someone-else", 9)]);
    let output = stub.run(&repo, &[], &[]);

    assert_eq!(
        lines_of(&output),
        vec![format!("> {} [main]", repo.display())]
    );
}

#[test]
fn more_than_thirty_pull_requests_do_not_hide_the_last_one() {
    let (_tmp, root) = scratch();
    let repo = one_repo_with(&root, &["feature"]);

    // The stub cuts its answer to the `--limit` it gets, and the default of the
    // real `gh` is 30. The branch with a worktree comes last, so a lookup that
    // forgets `--limit` loses exactly the number that this test reads.
    let mut pulls: Vec<Pull> = (1..=39)
        .map(|number| Pull::own(&format!("topic-{number}"), number))
        .collect();
    pulls.push(Pull::own("feature", 40));
    let stub = GhStub::new();
    stub.answer(ONE, &pulls);
    let output = stub.run(&repo, &[], &[]);

    assert!(
        row_of(&output, &root.join("one-worktrees/w0")).ends_with("[feature] #40"),
        "the list was:\n{}",
        stdout(&output)
    );
}

#[test]
fn a_machine_with_no_gh_lists_without_numbers_and_without_a_word() {
    let (_tmp, root) = scratch();
    let repo = one_repo_with(&root, &[]);
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).expect("failed to create the bin directory");
    std::os::unix::fs::symlink(real_git(), bin.join("git")).expect("failed to link git");

    let output = run_cwt(&repo, &[], &[], bin.as_os_str());

    assert!(output.status.success());
    assert_eq!(
        lines_of(&output),
        vec![format!("> {} [main]", repo.display())]
    );
    assert_eq!(stderr(&output), "");
}

#[test]
fn a_failed_gh_gives_one_warning_and_no_numbers() {
    let (_tmp, root) = scratch();
    let repo = one_repo_with(&root, &[]);
    let stub = GhStub::new();
    stub.put(ONE, "fail", "gh: HTTP 401 bad credentials\n");

    let output = stub.run(&repo, &[], &[]);
    assert!(output.status.success());
    assert_eq!(
        lines_of(&output),
        vec![format!("> {} [main]", repo.display())]
    );
    let warnings = stderr_lines(&output);
    assert_eq!(warnings.len(), 1, "stderr was: {warnings:?}");
    assert!(warnings[0].contains(ONE), "{}", warnings[0]);
    assert!(warnings[0].contains("bad credentials"), "{}", warnings[0]);

    // `--quiet` removes the warning and leaves the list.
    let quiet = stub.run(&repo, &["-q"], &[]);
    assert_eq!(stderr(&quiet), "");
    assert_eq!(lines_of(&quiet), lines_of(&output));
}

#[test]
fn a_gh_that_hangs_is_cut_off_with_one_warning() {
    let (_tmp, root) = scratch();
    let repo = one_repo_with(&root, &[]);
    let stub = GhStub::new();
    stub.put(ONE, "hang", "");

    // The bound is loose on purpose. The test proves that the run ends well
    // before the 120 seconds of the stub, and not how fast a loaded machine is.
    let start = std::time::Instant::now();
    let output = stub.run(&repo, &[], &[("CWT_PR_TIMEOUT", "1")]);
    let elapsed = start.elapsed();

    assert!(elapsed.as_secs() < 60, "the run took {elapsed:?}");
    assert_eq!(
        lines_of(&output),
        vec![format!("> {} [main]", repo.display())]
    );
    let warnings = stderr_lines(&output);
    assert_eq!(warnings.len(), 1, "stderr was: {warnings:?}");
    assert!(warnings[0].contains("did not answer"), "{}", warnings[0]);
}

#[test]
fn each_repository_of_a_family_shows_the_numbers_of_its_own_pull_requests() {
    let (_tmp, root) = scratch();
    let (fam, fam_shared, kid_shared) = family_with_shared_branch(&root);

    let stub = GhStub::new();
    stub.answer(FAM, &[Pull::own("shared", 1)]);
    stub.answer(KID, &[Pull::own("shared", 2)]);
    let output = stub.run(&fam, &[], &[]);

    assert!(output.status.success(), "cwt failed: {}", stderr(&output));
    assert!(row_of(&output, &fam_shared).ends_with("[shared] #1"));
    assert!(row_of(&output, &kid_shared).ends_with("[shared] #2"));
    assert_eq!(stderr(&output), "");
}

#[test]
fn the_lookups_of_a_family_run_at_the_same_time() {
    let (_tmp, root) = scratch();
    let (fam, fam_shared, kid_shared) = family_with_shared_branch(&root);

    // Each stub waits, with a bound, until the other one has started. A run
    // that asked one repository after the other would make the first stub give
    // up and fail, and the listing would lose its numbers.
    let stub = GhStub::new();
    stub.answer(FAM, &[Pull::own("shared", 1)]);
    stub.answer(KID, &[Pull::own("shared", 2)]);
    stub.put(FAM, "wait-for", &GhStub::key(KID));
    stub.put(KID, "wait-for", &GhStub::key(FAM));
    let output = stub.run(&fam, &[], &[]);

    assert!(row_of(&output, &fam_shared).ends_with("[shared] #1"));
    assert!(row_of(&output, &kid_shared).ends_with("[shared] #2"));
    assert_eq!(stderr(&output), "");
}

#[test]
fn moving_between_worktrees_never_runs_gh() {
    let (_tmp, root) = scratch();
    let repo = one_repo_with(&root, &["feature-a", "feature-b"]);
    let here = root.join("one-worktrees/w0");

    // Each case has its own stub, so a marker from one run cannot hide in the
    // next. The stub answers, so a call of `gh` would succeed quietly and only
    // the marker would tell.
    let cases: [(&[&str], i32); 6] = [
        (&["w1"], 0),
        (&["-f"], 0),
        (&["-p"], 0),
        (&["-m"], 0),
        (&["no-such-worktree"], 3),
        (&["feature"], 6),
    ];
    for (args, exit_code) in cases {
        let stub = GhStub::new();
        stub.answer(ONE, &[Pull::own("main", 1)]);
        let output = stub.run(&here, args, &[]);
        assert_eq!(
            support::code(&output),
            exit_code,
            "cwt {args:?}: {}",
            support::combined(&output)
        );
        assert!(!stub.ran(), "cwt {args:?} ran gh");
    }
    assert!(repo.is_dir());
}

#[test]
fn no_family_still_shows_the_numbers_of_the_one_repository() {
    let (_tmp, root) = scratch();
    let (fam, fam_shared, _kid_shared) = family_with_shared_branch(&root);

    // The stub has no answer for `kid`. A run that asked about `kid` would
    // print a warning.
    let stub = GhStub::new();
    stub.answer(FAM, &[Pull::own("shared", 1)]);
    let output = stub.run(&fam, &["--no-family"], &[]);

    assert!(row_of(&output, &fam_shared).ends_with("[shared] #1"));
    assert_eq!(stderr(&output), "");
}

#[test]
fn branch_names_with_multibyte_characters_show_their_numbers() {
    let (_tmp, root) = scratch();
    let branches = ["日本語", "🎉", "café"];
    let repo = one_repo_with(&root, &branches);

    let pulls: Vec<Pull> = branches
        .iter()
        .zip(1..)
        .map(|(branch, number)| Pull::own(branch, number))
        .collect();
    let stub = GhStub::new();
    stub.answer(ONE, &pulls);
    let output = stub.run(&repo, &[], &[]);

    for (index, (branch, number)) in branches.iter().zip(1..).enumerate() {
        let row = row_of(&output, &root.join(format!("one-worktrees/w{index}")));
        assert!(
            row.ends_with(&format!("[{branch}] #{number}")),
            "row was {row:?}"
        );
    }
}

#[test]
fn a_bad_time_limit_gives_one_warning_and_the_list_still_prints() {
    // The repository has no `origin`, so `gh` never runs. A bad value puts the
    // 3 s default back, and a stub that had to answer inside it on a loaded
    // machine would add a second warning that this test does not ask about.
    // The unit tests of `pr_time_limit_from` show that the default comes back.
    let (_tmp, root) = scratch();
    let repo = root.join("one");
    make_repo(&repo, "main");

    let stub = GhStub::new();
    let output = stub.run(&repo, &[], &[("CWT_PR_TIMEOUT", "abc")]);

    assert!(output.status.success(), "cwt failed: {}", stderr(&output));
    assert_eq!(
        lines_of(&output),
        vec![format!("> {} [main]", repo.display())]
    );
    let warnings = stderr_lines(&output);
    assert_eq!(warnings.len(), 1, "stderr was: {warnings:?}");
    assert!(warnings[0].contains("CWT_PR_TIMEOUT"), "{}", warnings[0]);
    assert!(warnings[0].contains("\"abc\""), "{}", warnings[0]);
}

#[test]
fn the_help_describes_the_pull_request_numbers_and_their_time_limit() {
    let (_tmp, root) = scratch();
    let output = run_cwt(&root, &["--help"], &[], &prepend_to_path(&root));
    assert!(output.status.success());

    let help = stdout(&output);
    assert!(help.contains("CWT_PR_TIMEOUT"), "the help was:\n{help}");
    assert!(help.contains("`gh`"), "the help was:\n{help}");
}
