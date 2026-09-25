//! End-to-end coverage for a new branch `<name>` when a remote holds `<name>`
//! (issue #528). The name comes from `-b <name>`, from the bare-number
//! shorthand `-b 33`, or from `branch = "<name>"` in `~/.nwt.toml`.
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
//! `support::nwt_command` also gives each run a private home directory, so a
//! `~/.nwt.toml` of the host cannot remove the `Tracking` line or a refusal
//! that these tests read, and cannot send a run through `-c`. The test of the
//! `branch` key gives its run a home of its own, in the temporary directory of
//! its fixture, and writes the `~/.nwt.toml` of that home.

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
    commit_new_files(&source, &["later.txt"], "move HEAD past the branch");

    clone_through_a_bare_remote(remote_temp, &source, branch)
}

/// Make a bare copy of `source` in `remote_temp`, and a clone of that copy.
///
/// `source` must hold `branch` as a local branch at a commit that is not its
/// checked-out commit. The clone then holds `branch` only as
/// `origin/<branch>`, and `HEAD` of the clone is not the commit of
/// `origin/<branch>`.
fn clone_through_a_bare_remote(remote_temp: TempDir, source: &Path, branch: &str) -> Fixture {
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

/// Write each of `files` into `repo`, and commit them and each staged change
/// with `message`.
///
/// Each file holds its own path and a line break, so each new file gives a
/// tree that no other commit of the fixture has.
fn commit_new_files(repo: &Path, files: &[&str], message: &str) {
    for file in files {
        write_file(repo, file, &format!("{file}\n"));
        assert!(run_git(repo, &["add", "--", file]), "git add failed");
    }
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
    run_nwt_under_home(fixture, None, args)
}

/// Run `nwt` as [`run_nwt`] does, and hand back what it wrote.
///
/// `home`, when it is there, becomes the home directory of the child, so the
/// child reads the `~/.nwt.toml` of that directory. `home` wins over the
/// private home of `support::nwt_command`, because this call sets `HOME` after
/// `nwt_command` sets it.
fn run_nwt_under_home(fixture: &Fixture, home: Option<&Path>, args: &[&str]) -> Output {
    let mut command = nwt_command(&fixture.clone);
    command
        .args(["--no-copy-env", "--no-bootstrap-hooks"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", fixture.empty_config.path())
        .env("GIT_CONFIG_SYSTEM", fixture.empty_config.path());
    if let Some(home) = home {
        command.env("HOME", home);
    }
    command.output().expect("run the nwt binary")
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

/// The file name of the `nwt` configuration in a home directory.
const CONFIG_FILE: &str = ".nwt.toml";

/// Make a home directory in the temporary directory of `fixture`, write a
/// [`CONFIG_FILE`] into it that sets `branch` to `branch`, and hand back the
/// home directory.
///
/// The home is beside the clone, so it is private to one test, and it goes
/// away with the fixture.
fn home_whose_config_names_the_branch(fixture: &Fixture, branch: &str) -> PathBuf {
    let home = fixture
        .clone
        .parent()
        .expect("the clone has a parent")
        .join("home");
    fs::create_dir(&home).unwrap_or_else(|e| panic!("create {}: {e}", home.display()));
    write_file(&home, CONFIG_FILE, &format!("branch = \"{branch}\"\n"));
    home
}

/// `branch = "<name>"` in `~/.nwt.toml` is a source of the name, as `-b <name>`
/// is. The run gives no `-b`. The lookup reads the name from the
/// configuration, so the new branch starts at `<remote>/<name>` and tracks it,
/// the directory takes the branch name, and stderr names the tracked branch.
#[test]
fn a_branch_from_the_config_file_starts_at_the_remote_branch_and_tracks_it() {
    let fixture = clone_whose_remote_holds(REMOTE_BRANCH);
    let home = home_whose_config_names_the_branch(&fixture, REMOTE_BRANCH);

    let output = run_nwt_under_home(&fixture, Some(&home), &[]);
    let worktree = created_worktree(&output);

    assert_worktree_is_named(&fixture, &worktree, REMOTE_BRANCH);
    assert_eq!(
        rev_parse(&worktree, "HEAD"),
        rev_parse(&fixture.clone, &remote_ref(REMOTE_BRANCH)),
        "branch in {CONFIG_FILE} must start at {REMOTE}/{REMOTE_BRANCH}, and not at HEAD of \
         the clone.\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        upstream_of(&fixture.clone, REMOTE_BRANCH),
        format!("{REMOTE}/{REMOTE_BRANCH}"),
        "branch in {CONFIG_FILE} must track {REMOTE}/{REMOTE_BRANCH}"
    );

    let expected = format!("{TRACKING_WORD} {REMOTE}/{REMOTE_BRANCH}");
    let lines = stderr_lines(&output);
    assert!(
        lines.contains(&expected),
        "stderr must hold the line {expected:?}, but it holds:\n{}",
        lines.join("\n")
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
    commit_new_files(&second, &["upstream.txt"], "the work of the second remote");
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

/// The exit code of a run whose add git refuses. It is `WORKTREE_FAILED` in
/// the `exit_codes` of `nwt`.
const WORKTREE_FAILED: i32 = 7;

/// Make a local branch `branch` at `HEAD` of the clone of `fixture`, and hand
/// back its commit.
///
/// `HEAD` of the clone is not the commit of any remote branch of the fixture,
/// so a run that moves the branch to a remote branch changes the commit.
fn make_local_branch(fixture: &Fixture, branch: &str) -> String {
    assert!(
        run_git(&fixture.clone, &["branch", "--no-track", branch]),
        "git branch failed"
    );
    rev_parse(&fixture.clone, &format!("refs/heads/{branch}"))
}

/// Demand that `output` is the refusal of a branch that already exists, as
/// `tests/branch-already-exists.rs` pins it: exit 7, no path on stdout, and
/// the message that names the branch on stderr.
fn assert_branch_exists_refusal(output: &Output, branch: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(WORKTREE_FAILED),
        "a local branch {branch} must give the branch-exists error, exit {WORKTREE_FAILED}.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.is_empty(),
        "a refused run prints no path. stdout: {stdout:?}"
    );
    let expected = format!("Error: Branch '{branch}' already exists.");
    assert!(
        stderr.contains(&expected),
        "stderr must hold {expected:?}:\n{stderr}"
    );
}

/// Demand that the local branch `branch` is still at `commit`, and that it
/// has no upstream.
fn assert_local_branch_untouched(fixture: &Fixture, branch: &str, commit: &str) {
    assert_eq!(
        rev_parse(&fixture.clone, &format!("refs/heads/{branch}")),
        commit,
        "the local branch {branch} must keep its commit"
    );
    assert!(
        !run_git(
            &fixture.clone,
            &[
                "rev-parse",
                "--abbrev-ref",
                &format!("{branch}@{{upstream}}"),
            ]
        ),
        "the local branch {branch} must get no upstream"
    );
}

/// The lookup runs only when no local branch `<name>` exists. When one
/// exists, the run gives the branch-exists error that it gave before the
/// lookup existed, also when two remotes hold `<name>` and no default remote
/// picks one. The error names the problem that the user has: the branch is
/// already there, and `--checkout` gets it.
#[test]
fn a_local_branch_gives_the_branch_exists_error_even_when_two_remotes_hold_it() {
    let fixture = clone_whose_two_remotes_hold(REMOTE_BRANCH);
    let commit = make_local_branch(&fixture, REMOTE_BRANCH);

    let output = run_nwt(&fixture, &["-b", REMOTE_BRANCH]);

    assert_branch_exists_refusal(&output, REMOTE_BRANCH);
    assert_local_branch_untouched(&fixture, REMOTE_BRANCH, &commit);
}

/// When two remotes hold `<name>`, `checkout.defaultRemote` names the remote
/// whose branch the new branch starts at and tracks, as it does for the
/// checkout DWIM of git. The clone states the key in its own configuration,
/// because each run reads an empty global and system configuration.
#[test]
fn checkout_default_remote_picks_the_branch_that_two_remotes_hold() {
    let fixture = clone_whose_two_remotes_hold(REMOTE_BRANCH);
    assert!(
        run_git(
            &fixture.clone,
            &["config", "checkout.defaultRemote", SECOND_REMOTE]
        ),
        "git config failed"
    );

    let output = run_nwt(&fixture, &["-b", REMOTE_BRANCH]);
    let worktree = created_worktree(&output);

    assert_worktree_is_named(&fixture, &worktree, REMOTE_BRANCH);
    assert_eq!(
        rev_parse(&worktree, "HEAD"),
        rev_parse(&fixture.clone, &remote_ref_on(SECOND_REMOTE, REMOTE_BRANCH)),
        "the worktree must start at {SECOND_REMOTE}/{REMOTE_BRANCH}"
    );
    assert_eq!(
        upstream_of(&fixture.clone, REMOTE_BRANCH),
        format!("{SECOND_REMOTE}/{REMOTE_BRANCH}"),
        "the new branch must track {SECOND_REMOTE}/{REMOTE_BRANCH}"
    );

    let expected = format!("{TRACKING_WORD} {SECOND_REMOTE}/{REMOTE_BRANCH}");
    let lines = stderr_lines(&output);
    assert!(
        lines.contains(&expected),
        "stderr must hold the line {expected:?}, but it holds:\n{}",
        lines.join("\n")
    );
}

/// When one remote holds `<name>` and a local branch `<name>` exists, the
/// run gives the branch-exists error that it gave before the lookup existed.
/// The local branch keeps its commit, and it gets no upstream.
#[test]
fn a_local_branch_gives_the_branch_exists_error_when_one_remote_holds_it() {
    let fixture = clone_whose_remote_holds(REMOTE_BRANCH);
    let commit = make_local_branch(&fixture, REMOTE_BRANCH);
    assert_ne!(
        commit,
        rev_parse(&fixture.clone, &remote_ref(REMOTE_BRANCH)),
        "the local branch must not be at the commit of {REMOTE}/{REMOTE_BRANCH}"
    );

    let output = run_nwt(&fixture, &["-b", REMOTE_BRANCH]);

    assert_branch_exists_refusal(&output, REMOTE_BRANCH);
    assert_local_branch_untouched(&fixture, REMOTE_BRANCH, &commit);
}

/// The directory that the remote branch tracks, and that `HEAD` of the clone
/// does not track.
const HEAVY_DIR: &str = "heavy";

/// The tracked files under [`HEAVY_DIR`] on the remote branch.
const HEAVY_FILES: &[&str] = &["heavy/big.txt", "heavy/sub/deep.txt"];

/// The file of the first commit that `init_repo` makes.
const BASELINE_FILE: &str = "README.md";

/// The tracked files of the remote branch outside [`HEAVY_DIR`].
const KEPT_FILES: &[&str] = &[BASELINE_FILE, "light.txt"];

/// The directory that `HEAD` of the clone tracks, and that the remote branch
/// does not track.
const HEAD_ONLY_DIR: &str = "head-only";

/// The tracked file under [`HEAD_ONLY_DIR`] at `HEAD` of the clone.
const HEAD_ONLY_FILE: &str = "head-only/big.txt";

/// The exit code of a run that refuses a `--sparse-exclude` value. It is
/// `INVALID_SPARSE_EXCLUDE` in the `exit_codes` of `nwt`.
const INVALID_SPARSE_EXCLUDE: i32 = 15;

/// True when git tracks `dir` as a directory at `rev` in `repo`.
///
/// `git ls-tree -d --name-only <rev> -- <dir>` prints `dir` for a directory,
/// and nothing for a file or a missing path.
fn tracks_directory_at(repo: &Path, rev: &str, dir: &str) -> bool {
    git_stdout(repo, &["ls-tree", "-d", "--name-only", rev, "--", dir])
        .lines()
        .any(|entry| entry == dir)
}

/// Make a clone whose one remote holds `branch` with [`HEAVY_DIR`], and whose
/// `HEAD` holds [`HEAD_ONLY_DIR`] in its place.
///
/// The remote branch tracks [`KEPT_FILES`] and [`HEAVY_FILES`]. The checked-out
/// branch of the remote then removes [`HEAVY_DIR`] and adds
/// [`HEAD_ONLY_FILE`]. So a `--sparse-exclude` check that reads `HEAD` of the
/// clone gives the opposite answer to a check that reads `origin/<branch>`, for
/// each of the two directories.
fn clone_whose_remote_branch_holds_the_heavy_dir(branch: &str) -> Fixture {
    let (remote_temp, source) = init_repo();
    let tree: Vec<&str> = KEPT_FILES
        .iter()
        .chain(HEAVY_FILES)
        .copied()
        .filter(|file| *file != BASELINE_FILE)
        .collect();
    commit_new_files(&source, &tree, "add the heavy dir");
    assert!(run_git(&source, &["branch", branch]), "git branch failed");

    assert!(
        run_git(&source, &["rm", "-r", "--quiet", "--", HEAVY_DIR]),
        "git rm failed"
    );
    commit_new_files(
        &source,
        &[HEAD_ONLY_FILE],
        "replace the heavy dir with the head-only dir",
    );

    let fixture = clone_through_a_bare_remote(remote_temp, &source, branch);

    let remote_branch = remote_ref(branch);
    assert!(
        tracks_directory_at(&fixture.clone, &remote_branch, HEAVY_DIR)
            && !tracks_directory_at(&fixture.clone, "HEAD", HEAVY_DIR),
        "the fixture must track {HEAVY_DIR}/ at {remote_branch} and not at HEAD"
    );
    assert!(
        tracks_directory_at(&fixture.clone, "HEAD", HEAD_ONLY_DIR)
            && !tracks_directory_at(&fixture.clone, &remote_branch, HEAD_ONLY_DIR),
        "the fixture must track {HEAD_ONLY_DIR}/ at HEAD and not at {remote_branch}"
    );

    fixture
}

/// With `-b <name>` that tracks `<remote>/<name>`, the `--sparse-exclude` check
/// reads that remote branch, because the files of the new worktree come from
/// it. So a directory that only the remote branch tracks passes the check.
///
/// The worktree does not hold the directory, holds each other tracked file of
/// the remote branch, and has no change for `git status` to report. It starts
/// at the remote branch and tracks it. A check that reads `HEAD` of the clone
/// refuses the run with exit 15, because `HEAD` does not track the directory.
#[test]
fn sparse_exclude_with_a_tracked_branch_accepts_a_directory_that_only_the_remote_branch_holds() {
    let fixture = clone_whose_remote_branch_holds_the_heavy_dir(REMOTE_BRANCH);

    let output = run_nwt(
        &fixture,
        &["-b", REMOTE_BRANCH, "--sparse-exclude", HEAVY_DIR],
    );
    let worktree = created_worktree(&output);

    assert!(
        !worktree.join(HEAVY_DIR).exists(),
        "{HEAVY_DIR}/ must be out of the worktree at {}",
        worktree.display()
    );
    for file in KEPT_FILES {
        assert!(
            worktree.join(file).is_file(),
            "{file} of {REMOTE}/{REMOTE_BRANCH} must be in the worktree at {}",
            worktree.display()
        );
    }
    let status = git_stdout(&worktree, &["status", "--short"]);
    assert!(
        status.is_empty(),
        "a sparse worktree has no change to report, but git status says:\n{status}"
    );

    assert_eq!(
        rev_parse(&worktree, "HEAD"),
        rev_parse(&fixture.clone, &remote_ref(REMOTE_BRANCH)),
        "the sparse worktree must start at {REMOTE}/{REMOTE_BRANCH}"
    );
    assert_eq!(
        upstream_of(&fixture.clone, REMOTE_BRANCH),
        format!("{REMOTE}/{REMOTE_BRANCH}"),
        "the new branch of a sparse run must track {REMOTE}/{REMOTE_BRANCH}"
    );
}

/// The stderr line of a run that refuses `dir`, because git does not track it
/// as a directory at `at_ref`.
fn not_tracked_message(dir: &str, at_ref: &str) -> String {
    format!(
        "Error: --sparse-exclude '{dir}' is not a directory that git tracks at '{at_ref}'. \
         Give a directory that git tracks at that ref."
    )
}

/// With `-b <name>` that tracks `<remote>/<name>`, a directory that `HEAD` of
/// the clone tracks and the remote branch does not is refused. The new
/// worktree has no such directory to exclude.
///
/// The run exits 15, prints no path, and names the directory and the short form
/// of the remote branch, as the user reads it. It refuses before it makes
/// anything, so no worktree, no branch, and no upstream configuration stay. A
/// check that reads `HEAD` of the clone accepts the directory, and the run
/// works.
#[test]
fn sparse_exclude_with_a_tracked_branch_refuses_a_directory_that_only_head_holds() {
    let fixture = clone_whose_remote_branch_holds_the_heavy_dir(REMOTE_BRANCH);

    let output = run_nwt(
        &fixture,
        &["-b", REMOTE_BRANCH, "--sparse-exclude", HEAD_ONLY_DIR],
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stderr_lines(&output);
    assert_eq!(
        output.status.code(),
        Some(INVALID_SPARSE_EXCLUDE),
        "the run must exit {INVALID_SPARSE_EXCLUDE}.\nstdout:\n{stdout}\nstderr:\n{}",
        lines.join("\n")
    );
    assert!(
        stdout.is_empty(),
        "a refused run prints no path. stdout: {stdout:?}"
    );

    let expected = not_tracked_message(HEAD_ONLY_DIR, &format!("{REMOTE}/{REMOTE_BRANCH}"));
    assert!(
        lines.contains(&expected),
        "stderr must hold the line {expected:?}, but it holds:\n{}",
        lines.join("\n")
    );

    assert_made_nothing(&fixture, REMOTE_BRANCH);
}

/// The exit status of the `post-checkout` hook that fails.
///
/// The value is not 1, so a run that reports the status of the hook is
/// distinguishable from a run that reports a generic failure.
#[cfg(unix)]
const HOOK_EXIT_STATUS: i32 = 3;

/// The start of the error line of a sparse run whose `post-checkout` hook
/// fails.
#[cfg(unix)]
const HOOK_FAILED_ERROR: &str = "Error: the post-checkout hook failed";

/// Make the clone of [`clone_whose_remote_branch_holds_the_heavy_dir`], give
/// it a `post-checkout` hook that exits [`HOOK_EXIT_STATUS`], and run
/// `nwt -b issue-33 --sparse-exclude heavy` with `extra` arguments in it.
///
/// The hook lives in a directory beside the clone, so it goes away with the
/// fixture. A sparse run adds the worktree with `--no-checkout`, which runs no
/// hook, so the one hook run is the `git hook run` of `nwt`.
#[cfg(unix)]
fn run_tracked_sparse_nwt_whose_hook_fails(extra: &[&str]) -> (Fixture, Output) {
    let fixture = clone_whose_remote_branch_holds_the_heavy_dir(REMOTE_BRANCH);
    let hooks = fixture
        .clone
        .parent()
        .expect("the clone has a parent")
        .join("hooks");
    fs::create_dir(&hooks).unwrap_or_else(|e| panic!("create {}: {e}", hooks.display()));
    support::install_post_checkout_hook(
        &fixture.clone,
        &hooks,
        &format!("exit {HOOK_EXIT_STATUS}\n"),
    );

    let mut arguments = vec!["-b", REMOTE_BRANCH, "--sparse-exclude", HEAVY_DIR];
    arguments.extend_from_slice(extra);
    let output = run_nwt(&fixture, &arguments);
    (fixture, output)
}

/// Demand that `output` is a tracked sparse run whose `post-checkout` hook
/// failed, and that the run kept what git made: exit 7, no path on stdout,
/// the sparse worktree `issue-33`, and the branch `issue-33` with
/// `origin/issue-33` as its upstream.
///
/// The upstream proves that the run took the remote branch as its start
/// point, so a missing `Tracking` line is a missing report and not a run that
/// tracked nothing.
#[cfg(unix)]
fn assert_the_failed_hook_kept_the_tracked_worktree(fixture: &Fixture, output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stderr_lines(output);
    assert_eq!(
        output.status.code(),
        Some(WORKTREE_FAILED),
        "a failed post-checkout hook is a worktree failure.\nstdout:\n{stdout}\nstderr:\n{}",
        lines.join("\n")
    );
    assert!(
        stdout.is_empty(),
        "a failed run prints no path, so the shell wrapper stays put. stdout: {stdout:?}"
    );

    let worktree = fixture.worktrees_dir().join(REMOTE_BRANCH);
    assert!(
        worktree.is_dir(),
        "the run keeps the worktree that the hook ran in: {}",
        worktree.display()
    );
    assert!(
        !worktree.join(HEAVY_DIR).exists(),
        "the kept worktree stays sparse: {}",
        worktree.display()
    );
    assert_eq!(
        upstream_of(&fixture.clone, REMOTE_BRANCH),
        format!("{REMOTE}/{REMOTE_BRANCH}"),
        "the kept branch must track {REMOTE}/{REMOTE_BRANCH}"
    );
}

/// A tracked sparse run whose `post-checkout` hook fails keeps the worktree
/// and its tracking branch. So stderr names the tracked branch, after the
/// error line that says where the worktree stays.
///
/// Git reports the upstream it set on its stdout, and `nwt` sends that stream
/// to null. The `Tracking` line is thus the only report of the start point,
/// on this path as on a run that works.
#[cfg(unix)]
#[test]
fn a_tracked_sparse_run_whose_hook_fails_names_the_tracked_branch_after_the_error() {
    let (fixture, output) = run_tracked_sparse_nwt_whose_hook_fails(&[]);
    assert_the_failed_hook_kept_the_tracked_worktree(&fixture, &output);

    let lines = stderr_lines(&output);
    let tracking = format!("{TRACKING_WORD} {REMOTE}/{REMOTE_BRANCH}");
    let error_at = lines
        .iter()
        .position(|line| line.starts_with(HOOK_FAILED_ERROR))
        .unwrap_or_else(|| {
            panic!(
                "stderr must hold a line that starts with {HOOK_FAILED_ERROR:?}, but it \
                 holds:\n{}",
                lines.join("\n")
            )
        });
    let tracking_at = lines
        .iter()
        .position(|line| *line == tracking)
        .unwrap_or_else(|| {
            panic!(
                "stderr must hold the line {tracking:?}, but it holds:\n{}",
                lines.join("\n")
            )
        });
    assert!(
        error_at < tracking_at,
        "the line {tracking:?} must come after the error line, but stderr holds:\n{}",
        lines.join("\n")
    );
}

/// `--quiet` suppresses the line that names the tracked branch when the
/// `post-checkout` hook of a tracked sparse run fails, as it does after a run
/// that works. The run still keeps the worktree and its tracking branch.
#[cfg(unix)]
#[test]
fn quiet_prints_no_tracking_line_when_the_hook_of_a_tracked_sparse_run_fails() {
    let (fixture, output) = run_tracked_sparse_nwt_whose_hook_fails(&["--quiet"]);
    assert_the_failed_hook_kept_the_tracked_worktree(&fixture, &output);

    let lines = stderr_lines(&output);
    assert!(
        !lines.iter().any(|line| line.starts_with(TRACKING_WORD)),
        "--quiet must print no {TRACKING_WORD} line, but stderr holds:\n{}",
        lines.join("\n")
    );
}
