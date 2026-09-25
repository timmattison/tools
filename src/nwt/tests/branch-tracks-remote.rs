//! End-to-end coverage for `nwt -b <name>` when a remote holds `<name>`
//! (issue #528).
//!
//! When the clone has no local branch `<name>` and exactly one remote holds
//! it, the new branch starts at `<remote>/<name>` and tracks it. Without this,
//! the new worktree does not hold the work of that branch, and the user must
//! move the branch and set its upstream by hand.
//!
//! Every test runs the real binary through `support::nwt_command`. The fixture
//! is a bare repository as the remote and a clone of it. The remote holds
//! [`REMOTE_BRANCH`] at a commit that is not the `HEAD` of the clone, so "the
//! worktree starts at the remote branch" and "the worktree starts at `HEAD`"
//! give different answers. Each test owns its own temporary directory, so the
//! fixed branch names are private to one test.
//!
//! Each `nwt` run reads an empty global and system git configuration, so a
//! `checkout.defaultRemote` or a `branch.autoSetupMerge` of the host cannot
//! change the answer. The run sets the two variables on the child only.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use support::{clone_of, git_stdout, init_repo, nwt_command, run_git, write_file};
use tempfile::{NamedTempFile, TempDir};

/// The branch that the remote holds, and that the clone holds only as a
/// remote-tracking branch.
const REMOTE_BRANCH: &str = "issue-33";

/// The one remote of the clone.
const REMOTE: &str = "origin";

/// A clone whose one remote holds a branch that the clone does not hold.
struct Fixture {
    /// The temporary directory that holds the clone and its worktrees
    /// directory.
    _temp: TempDir,
    /// The clone, where each `nwt` run starts.
    clone: PathBuf,
    /// The temporary directories that hold the remotes, kept alive for the
    /// life of the clone.
    _remotes: Vec<TempDir>,
    /// An empty file that each `nwt` run reads as its global and its system
    /// git configuration.
    empty_config: NamedTempFile,
}

impl Fixture {
    /// The directory where `nwt` puts the worktrees of the clone.
    fn worktrees_dir(&self) -> PathBuf {
        let name = self
            .clone
            .file_name()
            .expect("the clone has a name")
            .to_str()
            .expect("utf-8 clone name");
        self.clone.with_file_name(format!("{name}-worktrees"))
    }
}

/// Make a bare remote that holds `branch`, and a clone of it that holds
/// `branch` only as `origin/<branch>`.
///
/// `branch` stays at the first commit, and the checked-out branch of the
/// remote gets a second commit. So `HEAD` of the clone is not the commit of
/// `origin/<branch>`.
fn clone_whose_remote_holds(branch: &str) -> Fixture {
    let (remote_temp, source) = init_repo();
    assert!(run_git(&source, &["branch", branch]), "git branch failed");
    commit_new_file(&source, "later.txt", "move HEAD past the branch");

    let bare = remote_temp.path().join("remote.git");
    assert!(
        run_git(
            remote_temp.path(),
            &[
                "clone",
                "--bare",
                "--quiet",
                source.to_str().expect("utf-8 source path"),
                bare.to_str().expect("utf-8 remote path"),
            ]
        ),
        "git clone --bare failed"
    );

    let (temp, clone) = clone_of(&bare);

    let local = format!("refs/heads/{branch}");
    assert!(
        !run_git(&clone, &["show-ref", "--verify", "--quiet", &local]),
        "the fixture clone must not hold {local}"
    );
    assert_ne!(
        rev_parse(&clone, "HEAD"),
        rev_parse(&clone, &remote_ref(branch)),
        "the fixture branch must not be at HEAD of the clone"
    );

    Fixture {
        _temp: temp,
        clone,
        _remotes: vec![remote_temp],
        empty_config: NamedTempFile::new().expect("create an empty git configuration"),
    }
}

/// Write `file` into `repo`, and commit it with `message`.
///
/// The file holds its own name and a line break, so each new file gives a tree
/// that no other commit of the fixture has.
fn commit_new_file(repo: &Path, file: &str, message: &str) {
    write_file(repo, file, &format!("{file}\n"));
    assert!(run_git(repo, &["add", "--", file]), "git add failed");
    assert!(
        run_git(
            repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", message]
        ),
        "git commit failed"
    );
}

/// The full remote-tracking ref of `branch` on [`REMOTE`].
fn remote_ref(branch: &str) -> String {
    remote_ref_on(REMOTE, branch)
}

/// The full remote-tracking ref of `branch` on `remote`.
fn remote_ref_on(remote: &str, branch: &str) -> String {
    format!("refs/remotes/{remote}/{branch}")
}

/// The commit that `rev` names in `repo`.
fn rev_parse(repo: &Path, rev: &str) -> String {
    git_stdout(repo, &["rev-parse", "--verify", rev])
        .trim_end()
        .to_owned()
}

/// The upstream of `branch` in `repo`, in its short form (`origin/issue-33`).
fn upstream_of(repo: &Path, branch: &str) -> String {
    git_stdout(
        repo,
        &[
            "rev-parse",
            "--abbrev-ref",
            &format!("{branch}@{{upstream}}"),
        ],
    )
    .trim_end()
    .to_owned()
}

/// Run `nwt` in the clone with `args`, without the `.env` copy and the hook
/// bootstrap, and hand back what it wrote.
fn run_nwt(fixture: &Fixture, args: &[&str]) -> Output {
    nwt_command(&fixture.clone)
        .args(["--no-copy-env", "--no-bootstrap-hooks"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", fixture.empty_config.path())
        .env("GIT_CONFIG_SYSTEM", fixture.empty_config.path())
        .output()
        .expect("run the nwt binary")
}

/// Demand that `output` is a run that worked, and hand back the worktree path
/// it printed.
fn created_worktree(output: &Output) -> PathBuf {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "nwt failed ({:?}):\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code()
    );

    let printed = PathBuf::from(stdout.trim());
    assert!(
        printed.is_dir(),
        "nwt printed {}, which is no directory.\nstderr:\n{stderr}",
        printed.display()
    );
    printed
}

/// Demand that `worktree` is the directory `<clone>-worktrees/<name>`.
///
/// Git can name the clone through a path that holds a symbolic link, such as
/// `/var` for `/private/var` on macOS. So the two paths are compared after
/// each is resolved.
fn assert_worktree_is_named(fixture: &Fixture, worktree: &Path, name: &str) {
    let expected = fixture.worktrees_dir().join(name);
    assert_eq!(
        fs::canonicalize(worktree).expect("resolve the worktree path"),
        fs::canonicalize(&expected).expect("resolve the expected worktree path"),
        "the worktree must be {}",
        expected.display()
    );
}

/// With `-b <name>`, and one remote that holds `<name>`, the new branch starts
/// at the commit of `<remote>/<name>` and tracks that branch. The directory
/// still takes the branch name.
#[test]
fn a_branch_that_one_remote_holds_starts_at_that_branch_and_tracks_it() {
    let fixture = clone_whose_remote_holds(REMOTE_BRANCH);

    let output = run_nwt(&fixture, &["-b", REMOTE_BRANCH]);
    let worktree = created_worktree(&output);

    assert_worktree_is_named(&fixture, &worktree, REMOTE_BRANCH);
    assert_eq!(
        rev_parse(&worktree, "HEAD"),
        rev_parse(&fixture.clone, &remote_ref(REMOTE_BRANCH)),
        "the worktree must start at {REMOTE}/{REMOTE_BRANCH}, and not at HEAD of the clone.\n\
         stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        upstream_of(&fixture.clone, REMOTE_BRANCH),
        format!("{REMOTE}/{REMOTE_BRANCH}"),
        "the new branch must track {REMOTE}/{REMOTE_BRANCH}"
    );
}

/// Each line that `output` wrote to stderr.
fn stderr_lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// After a run that tracks a remote branch, stderr holds one line that names
/// that branch. The line tells the user which start point `nwt` used. It goes
/// to stderr, because the shell wrapper reads the worktree path from stdout.
#[test]
fn a_run_that_tracks_a_remote_branch_names_it_on_stderr() {
    let fixture = clone_whose_remote_holds(REMOTE_BRANCH);

    let output = run_nwt(&fixture, &["-b", REMOTE_BRANCH]);
    created_worktree(&output);

    let expected = format!("Tracking {REMOTE}/{REMOTE_BRANCH}");
    let lines = stderr_lines(&output);
    assert!(
        lines.contains(&expected),
        "stderr must hold the line {expected:?}, but it holds:\n{}",
        lines.join("\n")
    );
}

/// The first word of the line that names the branch that a run tracks.
const TRACKING_WORD: &str = "Tracking";

/// `--quiet` suppresses the line that names the tracked branch. The run still
/// starts at the remote branch and tracks it.
#[test]
fn quiet_prints_no_tracking_line_and_still_tracks() {
    let fixture = clone_whose_remote_holds(REMOTE_BRANCH);

    let output = run_nwt(&fixture, &["-b", REMOTE_BRANCH, "--quiet"]);
    let worktree = created_worktree(&output);

    let lines = stderr_lines(&output);
    assert!(
        !lines.iter().any(|line| line.starts_with(TRACKING_WORD)),
        "--quiet must print no {TRACKING_WORD} line, but stderr holds:\n{}",
        lines.join("\n")
    );
    assert_eq!(
        rev_parse(&worktree, "HEAD"),
        rev_parse(&fixture.clone, &remote_ref(REMOTE_BRANCH)),
        "the worktree must start at {REMOTE}/{REMOTE_BRANCH} under --quiet too"
    );
    assert_eq!(
        upstream_of(&fixture.clone, REMOTE_BRANCH),
        format!("{REMOTE}/{REMOTE_BRANCH}"),
        "the new branch must track {REMOTE}/{REMOTE_BRANCH} under --quiet too"
    );
}

/// The bare number that the shorthand turns into [`REMOTE_BRANCH`].
const REMOTE_BRANCH_NUMBER: &str = "33";

/// `-b 33` becomes `-b issue-33`, and the lookup reads the branch name after
/// that step. So the shorthand gives the same result as the full name: the
/// directory `issue-33`, a start at `origin/issue-33`, and that upstream.
#[test]
fn the_bare_number_shorthand_tracks_the_remote_branch_too() {
    let fixture = clone_whose_remote_holds(REMOTE_BRANCH);

    let output = run_nwt(&fixture, &["-b", REMOTE_BRANCH_NUMBER]);
    let worktree = created_worktree(&output);

    assert_worktree_is_named(&fixture, &worktree, REMOTE_BRANCH);
    assert_eq!(
        rev_parse(&worktree, "HEAD"),
        rev_parse(&fixture.clone, &remote_ref(REMOTE_BRANCH)),
        "-b {REMOTE_BRANCH_NUMBER} must start at {REMOTE}/{REMOTE_BRANCH}"
    );
    assert_eq!(
        upstream_of(&fixture.clone, REMOTE_BRANCH),
        format!("{REMOTE}/{REMOTE_BRANCH}"),
        "-b {REMOTE_BRANCH_NUMBER} must track {REMOTE}/{REMOTE_BRANCH}"
    );
}

/// A branch that no remote holds.
const LOCAL_ONLY_BRANCH: &str = "issue-44";

/// When no remote holds `<name>`, the new branch starts at `HEAD` of the
/// clone and has no upstream, as before the lookup existed. No line names a
/// tracked branch.
#[test]
fn a_branch_that_no_remote_holds_starts_at_head_without_an_upstream() {
    let fixture = clone_whose_remote_holds(REMOTE_BRANCH);
    let remote_ref_of_the_name = remote_ref(LOCAL_ONLY_BRANCH);
    assert!(
        !run_git(
            &fixture.clone,
            &["show-ref", "--verify", "--quiet", &remote_ref_of_the_name]
        ),
        "the fixture clone must not hold {remote_ref_of_the_name}"
    );

    let output = run_nwt(&fixture, &["-b", LOCAL_ONLY_BRANCH]);
    let worktree = created_worktree(&output);

    assert_worktree_is_named(&fixture, &worktree, LOCAL_ONLY_BRANCH);
    assert_eq!(
        rev_parse(&worktree, "HEAD"),
        rev_parse(&fixture.clone, "HEAD"),
        "the worktree must start at HEAD of the clone"
    );

    // The branch must exist, so that the failed upstream question below means
    // "no upstream" and not "no branch".
    let local = format!("refs/heads/{LOCAL_ONLY_BRANCH}");
    assert!(
        run_git(&fixture.clone, &["show-ref", "--verify", "--quiet", &local]),
        "the run must make {local}"
    );
    assert!(
        !run_git(
            &fixture.clone,
            &[
                "rev-parse",
                "--abbrev-ref",
                &format!("{LOCAL_ONLY_BRANCH}@{{upstream}}"),
            ]
        ),
        "{LOCAL_ONLY_BRANCH} must have no upstream"
    );

    let lines = stderr_lines(&output);
    assert!(
        !lines.iter().any(|line| line.starts_with(TRACKING_WORD)),
        "a run that tracks nothing must print no {TRACKING_WORD} line, but stderr holds:\n{}",
        lines.join("\n")
    );
}

/// The second remote of a clone that two remotes give [`REMOTE_BRANCH`].
const SECOND_REMOTE: &str = "upstream";

/// The exit code of a run that two remotes give the branch, when
/// `checkout.defaultRemote` picks neither. It is `AMBIGUOUS_REMOTE_BRANCH` in
/// the `exit_codes` of `nwt`.
const AMBIGUOUS_REMOTE_BRANCH: i32 = 16;

/// Make a clone whose [`REMOTE`] and [`SECOND_REMOTE`] both hold `branch`, each
/// at its own commit.
///
/// The second remote is a repository of its own. Its `branch` holds a file
/// that no other commit of the fixture holds, so its commit is neither the
/// commit of `origin/<branch>` nor `HEAD` of the clone. The clone gets the
/// second remote through `git remote add` and `git fetch`, as a user adds a
/// fork. The clone configures no `checkout.defaultRemote`.
fn clone_whose_two_remotes_hold(branch: &str) -> Fixture {
    let mut fixture = clone_whose_remote_holds(branch);

    let (second_temp, second) = init_repo();
    commit_new_file(&second, "upstream.txt", "the work of the second remote");
    assert!(run_git(&second, &["branch", branch]), "git branch failed");

    let second = second.to_str().expect("utf-8 remote path");
    assert!(
        run_git(&fixture.clone, &["remote", "add", SECOND_REMOTE, second]),
        "git remote add failed"
    );
    assert!(
        run_git(&fixture.clone, &["fetch", "--quiet", SECOND_REMOTE]),
        "git fetch failed"
    );
    fixture._remotes.push(second_temp);

    let second_commit = rev_parse(&fixture.clone, &remote_ref_on(SECOND_REMOTE, branch));
    assert_ne!(
        second_commit,
        rev_parse(&fixture.clone, "HEAD"),
        "{SECOND_REMOTE}/{branch} must not be at HEAD of the clone"
    );
    assert_ne!(
        second_commit,
        rev_parse(&fixture.clone, &remote_ref(branch)),
        "{SECOND_REMOTE}/{branch} must not be at the commit of {REMOTE}/{branch}"
    );

    fixture
}

/// Demand that a refused run made nothing in the clone of `fixture`: no
/// worktrees directory beside the clone, no worktree that git knows about
/// other than the main worktree, no local branch `branch`, and no
/// `branch.<branch>.*` configuration.
///
/// The temporary directory of the fixture holds only the clone until `nwt`
/// makes something.
fn assert_made_nothing(fixture: &Fixture, branch: &str) {
    let parent = fixture.clone.parent().expect("the clone has a parent");
    let beside: Vec<String> = fs::read_dir(parent)
        .expect("read the directory that holds the clone")
        .map(|entry| {
            entry
                .expect("read one directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let clone_name = fixture
        .clone
        .file_name()
        .expect("the clone has a name")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        beside,
        vec![clone_name],
        "a refused run must make no worktrees directory"
    );

    let listed: Vec<PathBuf> = git_stdout(&fixture.clone, &["worktree", "list", "--porcelain"])
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(|path| fs::canonicalize(path).expect("resolve a listed worktree"))
        .collect();
    assert_eq!(
        listed,
        vec![fs::canonicalize(&fixture.clone).expect("resolve the clone")],
        "a refused run must leave the main worktree as the only worktree"
    );

    let local = format!("refs/heads/{branch}");
    assert!(
        !run_git(&fixture.clone, &["show-ref", "--verify", "--quiet", &local]),
        "a refused run must make no {local}"
    );

    let section = format!("branch.{branch}.");
    let configured = git_stdout(&fixture.clone, &["config", "--local", "--list"]);
    let written: Vec<&str> = configured
        .lines()
        .filter(|line| line.starts_with(&section))
        .collect();
    assert!(
        written.is_empty(),
        "a refused run must write no {section}* configuration, but the clone holds:\n{}",
        written.join("\n")
    );
}

/// The lines of the refusal when [`REMOTE`] and [`SECOND_REMOTE`] both hold
/// `branch`, and `checkout.defaultRemote` picks neither.
fn ambiguous_remote_lines(branch: &str) -> Vec<String> {
    vec![
        format!(
            "Error: more than one remote holds the branch '{branch}', and \
             checkout.defaultRemote picks none of them:"
        ),
        format!("  {REMOTE}/{branch}"),
        format!("  {SECOND_REMOTE}/{branch}"),
        "Name the remote to track, and run nwt again:".to_owned(),
        "  git config checkout.defaultRemote <remote>".to_owned(),
    ]
}

/// When two remotes hold `<name>` and `checkout.defaultRemote` picks neither,
/// `nwt` cannot know which branch the user wants. It refuses with exit 16
/// before it makes anything. The message names each candidate, and it gives
/// the command that picks one. Before this refusal, the branch started at
/// `HEAD` in silence.
#[test]
fn two_remotes_that_hold_the_branch_and_no_default_remote_refuse_and_make_nothing() {
    let fixture = clone_whose_two_remotes_hold(REMOTE_BRANCH);

    let output = run_nwt(&fixture, &["-b", REMOTE_BRANCH]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stderr_lines(&output);
    assert_eq!(
        output.status.code(),
        Some(AMBIGUOUS_REMOTE_BRANCH),
        "the run must exit {AMBIGUOUS_REMOTE_BRANCH}.\nstdout:\n{stdout}\nstderr:\n{}",
        lines.join("\n")
    );
    assert!(
        stdout.is_empty(),
        "a refused run prints no path. stdout: {stdout:?}"
    );

    let expected = ambiguous_remote_lines(REMOTE_BRANCH);
    assert!(
        lines
            .windows(expected.len())
            .any(|window| window == expected.as_slice()),
        "stderr must hold these lines:\n{}\nbut it holds:\n{}",
        expected.join("\n"),
        lines.join("\n")
    );

    assert_made_nothing(&fixture, REMOTE_BRANCH);
}
