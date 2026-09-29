//! End-to-end coverage for the child repository links of issue #537.
//!
//! A container repository tracks the map of a workspace. The real repositories
//! sit one level below it, and its `.gitignore` keeps them out of its history.
//! `git worktree add` writes only tracked files, so a new worktree of the
//! container holds no child. `nwt` links each child of the main worktree into
//! the new worktree, as a symlink to the one real checkout of that child.
//!
//! Each test builds a container in a temporary directory, runs the real `nwt`
//! binary through `support::nwt_command`, and reads the new worktree and the
//! output of the run.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use support::{git_stdout, init_repo, nanos, nwt_command, run_git, write_file};
use tempfile::TempDir;

/// The file that tells git which paths to ignore.
const IGNORE_FILE: &str = ".gitignore";

/// The file that each child repository of a fixture holds.
const CHILD_FILE: &str = "README.md";

/// The first child of the container, as the issue names it.
const VIAL_QMK: &str = "vial-qmk";

/// The second child of the container, as the issue names it.
const ZMK_CONFIG: &str = "zmk-config-corne";

/// The `.gitignore` pattern that ignores [`VIAL_QMK`] at the root, and also
/// ignores a symlink of that name.
const IGNORE_VIAL_QMK: &str = "/vial-qmk";

/// The `.gitignore` pattern that ignores [`ZMK_CONFIG`] at the root, and also
/// ignores a symlink of that name.
const IGNORE_ZMK_CONFIG: &str = "/zmk-config-corne";

/// A child that holds an untracked `.env`, as the issue names it.
const VIAL: &str = "vial";

/// The `.gitignore` pattern that ignores [`VIAL`] at the root.
const IGNORE_VIAL: &str = "/vial";

/// The file that the `.env` copy takes.
const ENV_FILE: &str = ".env";

/// The content of the `.env` of a child.
const CHILD_ENV: &str = "CHILD=1\n";

/// The start of the line that the `.env` copy prints for a destination that is
/// already there.
const KEPT_EXISTING: &str = "Kept existing:";

/// A child whose name starts with the pathspec magic character of git.
#[cfg(unix)]
const COLON_CHILD: &str = ":vial";

/// The `.gitignore` pattern that ignores [`COLON_CHILD`] at the root. It does
/// not ignore `vial`.
#[cfg(unix)]
const IGNORE_COLON_CHILD: &str = "/:vial";

/// The flag that stops a package manager install. No test here has a
/// `package.json`, and the flag keeps each run short.
const NO_BOOTSTRAP_HOOKS: &str = "--no-bootstrap-hooks";

/// The flag that makes a new branch for the new worktree.
const NEW_BRANCH_FLAG: &str = "-b";

/// The flag that checks out a branch that exists in the new worktree.
const CHECKOUT_FLAG: &str = "-c";

/// A file that a side branch tracks below the directory [`VIAL`].
const TRACKED_FILE: &str = "vial/tracked.txt";

/// The name of [`TRACKED_FILE`] in its directory.
const TRACKED_FILE_NAME: &str = "tracked.txt";

/// The content of [`TRACKED_FILE`].
const TRACKED_CONTENT: &str = "tracked by the side branch\n";

/// The content of the file [`VIAL`] that a `post-checkout` hook writes into
/// the new worktree.
#[cfg(unix)]
const HOOK_FILE_CONTENT: &str = "made by the post-checkout hook\n";

/// A name in the temporary directory of a fixture that nothing makes. A broken
/// symlink points at it.
#[cfg(unix)]
const MISSING_TARGET: &str = "missing-target";

/// The start of the line for a child whose path the new worktree already
/// holds. The line is not a warning, because nothing is wrong.
const ALREADY_THERE_PREFIX: &str = "Not linked:";

/// Why a child whose path the new worktree already holds has no link.
const ALREADY_THERE_REASON: &str = "the new worktree already holds this path";

/// A `.gitignore` pattern that ignores the directory [`VIAL`], and not a
/// symlink of that name. Git sees a symlink as a file.
const DIRECTORY_ONLY_VIAL: &str = "vial/";

/// The warning for [`VIAL`] when line 1 of `.gitignore` is
/// [`DIRECTORY_ONLY_VIAL`], as issue #537 gives it.
const DIRECTORY_ONLY_WARNING: &str = "Warning: not linked: vial (.gitignore:1 has 'vial/', \
     which matches only a directory, and git sees a symlink as a file. Write '/vial' to link it)";

/// The warning for [`VIAL`] when no pattern matches it, as issue #537 gives it.
const NO_PATTERN_WARNING: &str =
    "Warning: not linked: vial (git does not ignore this path. Add '/vial' to .gitignore to link it)";

/// The words between the child and the error of git, in the warning for a
/// child that git gives no answer about.
#[cfg(unix)]
const GIT_FAILED_REASON: &str = "git check-ignore failed:";

/// The flag that stops the `.env` copy.
#[cfg(unix)]
const NO_COPY_ENV: &str = "--no-copy-env";

/// A directory of the main worktree that holds no `.git` entry.
const PLAIN_DIR: &str = "plain";

/// A regular file of the main worktree.
const PLAIN_FILE: &str = "file.txt";

/// A directory of the main worktree that holds no `.git` entry, and that holds
/// the child repository [`INNER`].
const OUTER_DIR: &str = "outer";

/// A child repository of [`OUTER_DIR`], and thus a child of a child.
const INNER: &str = "inner";

/// The `.gitignore` patterns that ignore [`PLAIN_DIR`], [`PLAIN_FILE`],
/// [`VIAL`], and [`OUTER_DIR`] at the root.
const IGNORE_NOT_CHILDREN: [&str; 4] = ["/plain", "/file.txt", IGNORE_VIAL, "/outer"];

/// A name in the temporary directory of a fixture that nothing makes. A broken
/// `.git` file names it as the git directory.
#[cfg(unix)]
const MISSING_GIT_DIR: &str = "missing-git-dir";

/// The flag that turns the links off for one run.
const NO_LINK_CHILDREN: &str = "--no-link-children";

/// The name of the config file of `nwt` in the home directory.
const CONFIG_FILE: &str = ".nwt.toml";

/// A config file that turns the links off by default.
const LINKS_OFF_CONFIG: &str = "link_children = false\n";

/// The flag that removes the lines that report on the run.
const QUIET: &str = "-q";

/// The prefix of the line that names a worktree in `git worktree list
/// --porcelain`.
const WORKTREE_LINE_PREFIX: &str = "worktree ";

/// The first word of each line that reports a link.
const LINKED_WORD: &str = "Linked";

/// The word that each line about a link holds, in lower case. The line for a
/// link starts with `Linked`, and a warning holds `not linked`.
const LINKED_WORD_LOWER: &str = "linked";

/// The summary after a run that links two children.
const SUMMARY_OF_TWO: &str = "Linked 2 child repositories from the main worktree";

/// The summary after a run that links one child.
const SUMMARY_OF_ONE: &str = "Linked 1 child repository from the main worktree";

/// The start of the warning for a child that `nwt` did not link.
const NOT_LINKED_PREFIX: &str = "Warning: not linked:";

/// The body of a `post-checkout` hook that removes the write permission of the
/// new worktree. The hook runs with the new worktree as its working directory.
#[cfg(unix)]
const READ_ONLY_HOOK: &str = "chmod a-w .\n";

/// The mode that gives the owner of a directory the write permission again.
#[cfg(unix)]
const WRITABLE_MODE: u32 = 0o755;

/// The directory that holds the worktrees of the repository that
/// `support::init_repo` makes. That repository is `<temp>/repo`.
const WORKTREES_DIR_NAME: &str = "repo-worktrees";

/// Resolve a path before an assertion compares it.
///
/// Every fixture lives under a temporary directory that macOS reaches through a
/// symbolic link: `/var` resolves to `/private/var`.
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
}

/// A branch name that no concurrent copy of this suite can also hold.
fn unique_branch(label: &str) -> String {
    format!("{label}-{}-{}", std::process::id(), nanos())
}

/// Make the child repository `name` in `repo`, with one file in it.
///
/// # Panics
///
/// Panics when the directory cannot be made, or `git init` fails.
fn make_child(repo: &Path, name: &str) {
    let child = repo.join(name);
    fs::create_dir(&child).unwrap_or_else(|e| panic!("create {}: {e}", child.display()));
    assert!(
        run_git(&child, &["init"]),
        "git init failed in {}",
        child.display()
    );
    write_file(&child, CHILD_FILE, &format!("{name}\n"));
}

/// A container repository whose committed `.gitignore` holds `ignore_lines`,
/// with one child repository for each name of `children`.
///
/// Hand back the temporary directory (keep it alive) and the container.
///
/// # Panics
///
/// Panics when a write or a git command fails.
fn container(ignore_lines: &[&str], children: &[&str]) -> (TempDir, PathBuf) {
    let (temp, repo) = init_repo();

    let mut ignore = ignore_lines.join("\n");
    ignore.push('\n');
    write_file(&repo, IGNORE_FILE, &ignore);
    assert!(
        run_git(&repo, &["add", "--", IGNORE_FILE]),
        "git add {IGNORE_FILE} failed"
    );
    commit(&repo, "ignore the children");

    for child in children {
        make_child(&repo, child);
    }

    (temp, repo)
}

/// Commit what `repo` has staged, with `message`.
///
/// # Panics
///
/// Panics when `git commit` fails.
fn commit(repo: &Path, message: &str) {
    assert!(
        run_git(
            repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", message]
        ),
        "git commit -m {message:?} failed in {}",
        repo.display()
    );
}

/// A container whose `.gitignore` ignores `/vial`, whose main worktree holds
/// the child repository [`VIAL`], and whose branch `side` tracks
/// [`TRACKED_FILE`].
///
/// The fixture commits the file on `side` before the child exists. `git add
/// -f` takes the file past the ignore rule. Back on the first branch, git
/// removes the file and its directory, and `git init` then makes the child in
/// the same place. Thus the main worktree holds a child, and a worktree of
/// `side` holds a real directory at the same path.
///
/// # Panics
///
/// Panics when a write or a git command fails.
fn container_with_tracked_directory(side: &str) -> (TempDir, PathBuf) {
    let (temp, repo) = container(&[IGNORE_VIAL], &[]);

    assert!(
        run_git(&repo, &["checkout", "-q", "-b", side]),
        "git checkout -b {side} failed"
    );
    write_file(&repo, TRACKED_FILE, TRACKED_CONTENT);
    assert!(
        run_git(&repo, &["add", "-f", "--", TRACKED_FILE]),
        "git add -f {TRACKED_FILE} failed"
    );
    commit(&repo, "track a file where the main worktree holds a child");
    assert!(
        run_git(&repo, &["checkout", "-q", "-"]),
        "git checkout of the first branch failed"
    );

    make_child(&repo, VIAL);
    (temp, repo)
}

/// The main worktree of `repo`, as git names it.
///
/// `nwt` builds each link target from this path, so a test builds the expected
/// target from it too.
fn main_worktree(repo: &Path) -> PathBuf {
    let listing = git_stdout(repo, &["worktree", "list", "--porcelain"]);
    let main = listing
        .lines()
        .find_map(|line| line.strip_prefix(WORKTREE_LINE_PREFIX))
        .unwrap_or_else(|| panic!("git names no worktree of {}:\n{listing}", repo.display()));
    PathBuf::from(main)
}

/// The branch of the new worktree, and how `nwt` gets it.
#[derive(Clone, Copy)]
enum Start<'a> {
    /// `-b <branch>` makes the branch, and names the directory after it.
    NewBranch(&'a str),
    /// `-c <branch>` checks out a branch that exists, in a directory with a
    /// random name.
    Checkout(&'a str),
}

impl<'a> Start<'a> {
    /// The flag that gives the branch to `nwt`.
    fn flag(self) -> &'static str {
        match self {
            Start::NewBranch(_) => NEW_BRANCH_FLAG,
            Start::Checkout(_) => CHECKOUT_FLAG,
        }
    }

    /// The name of the branch.
    fn branch(self) -> &'a str {
        match self {
            Start::NewBranch(branch) | Start::Checkout(branch) => branch,
        }
    }
}

/// Run `nwt <start> --no-bootstrap-hooks <extra>` in `repo`.
///
/// `home`, when it is there, becomes the home directory of the child. It wins
/// over the private home of `support::nwt_command`, because this call sets
/// `HOME` after that function sets it.
fn run_nwt(repo: &Path, start: Start<'_>, extra: &[&str], home: Option<&Path>) -> Output {
    let mut command = nwt_command(repo);
    command
        .args([start.flag(), start.branch(), NO_BOOTSTRAP_HOOKS])
        .args(extra);
    if let Some(home) = home {
        command.env("HOME", home);
    }
    command.output().expect("run the nwt binary")
}

/// What a successful run of `nwt` gave.
struct Run {
    /// The new worktree, as `nwt` printed it.
    worktree: PathBuf,
    /// Everything `nwt` wrote to stderr.
    stderr: String,
}

/// Run `nwt -b <branch>` as [`run_nwt`] does, and prove that it succeeded and
/// that its stdout holds only the path of the new worktree.
fn successful_run(
    temp: &TempDir,
    repo: &Path,
    branch: &str,
    extra: &[&str],
    home: Option<&Path>,
) -> Run {
    successful_start(temp, repo, Start::NewBranch(branch), extra, home)
}

/// Run `nwt` as [`run_nwt`] does, and prove that it succeeded and that its
/// stdout holds only the path of the new worktree.
///
/// The shell wrapper does `dir=$(command nwt "$@")`, so any other line on
/// stdout breaks the `cd` into the worktree. A run with `-b` names the
/// directory after the branch. A run with `-c` gives the directory a random
/// name, so the proof then reads only the directory that holds it.
fn successful_start(
    temp: &TempDir,
    repo: &Path,
    start: Start<'_>,
    extra: &[&str],
    home: Option<&Path>,
) -> Run {
    let output = run_nwt(repo, start, extra, home);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let (flag, branch) = (start.flag(), start.branch());

    assert!(
        output.status.success(),
        "nwt {flag} {branch} {extra:?} failed in {}:\n{stdout}\n{stderr}",
        repo.display()
    );

    let printed = stdout.lines().next().unwrap_or_default().to_string();
    assert_eq!(
        stdout,
        format!("{printed}\n"),
        "stdout must hold only the worktree path, and stderr reads:\n{stderr}"
    );
    let worktree = PathBuf::from(printed);
    let worktrees_dir = canonical(temp.path()).join(WORKTREES_DIR_NAME);
    match start {
        Start::NewBranch(_) => assert_eq!(
            canonical(&worktree),
            worktrees_dir.join(branch),
            "nwt printed a path that is not the new worktree"
        ),
        Start::Checkout(_) => assert_eq!(
            canonical(&worktree).parent(),
            Some(worktrees_dir.as_path()),
            "nwt printed a path that is not in the directory of the worktrees"
        ),
    }

    Run { worktree, stderr }
}

/// The lines of `stderr` that start with [`LINKED_WORD`], in order.
fn linked_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| line.starts_with(LINKED_WORD))
        .collect()
}

/// The line that reports the link of the child `name` of `main`.
fn linked_line(main: &Path, name: &str) -> String {
    format!("Linked {name} -> {}", main.join(name).display())
}

/// Prove that `<worktree>/<name>` is a symlink whose target is the absolute
/// path `<main>/<name>`, and that the target is the real child in `repo`.
fn assert_linked(worktree: &Path, main: &Path, repo: &Path, name: &str) {
    let link = worktree.join(name);
    let kind = fs::symlink_metadata(&link)
        .unwrap_or_else(|e| panic!("{} is not there: {e}", link.display()))
        .file_type();
    assert!(kind.is_symlink(), "{} is not a symlink", link.display());

    let target =
        fs::read_link(&link).unwrap_or_else(|e| panic!("read link {}: {e}", link.display()));
    assert!(
        target.is_absolute(),
        "the target of {} is {}, which is not absolute",
        link.display(),
        target.display()
    );
    assert_eq!(
        target,
        main.join(name),
        "the target of {} must be the main worktree that git names, joined with the child",
        link.display()
    );
    assert_eq!(
        canonical(&target),
        canonical(&repo.join(name)),
        "the link must reach the real child of the main worktree"
    );
}

/// A container with two ignored children gets a link to each of them, one line
/// for each link, and a summary in the plural.
#[test]
fn each_ignored_child_is_linked_into_the_new_worktree() {
    let (temp, repo) = container(
        &[IGNORE_VIAL_QMK, IGNORE_ZMK_CONFIG],
        &[VIAL_QMK, ZMK_CONFIG],
    );
    let main = main_worktree(&repo);
    let branch = unique_branch("two-children");

    let run = successful_run(&temp, &repo, &branch, &[], None);

    assert_linked(&run.worktree, &main, &repo, VIAL_QMK);
    assert_linked(&run.worktree, &main, &repo, ZMK_CONFIG);
    assert_eq!(
        linked_lines(&run.stderr),
        vec![
            linked_line(&main, VIAL_QMK).as_str(),
            linked_line(&main, ZMK_CONFIG).as_str(),
            SUMMARY_OF_TWO,
        ],
        "stderr must name each link and then the count, but it reads:\n{}",
        run.stderr
    );
}

/// A container with one ignored child gets a summary in the singular.
#[test]
fn one_linked_child_gets_the_singular_summary() {
    let (temp, repo) = container(&[IGNORE_VIAL_QMK], &[VIAL_QMK]);
    let main = main_worktree(&repo);
    let branch = unique_branch("one-child");

    let run = successful_run(&temp, &repo, &branch, &[], None);

    assert_linked(&run.worktree, &main, &repo, VIAL_QMK);
    assert_eq!(
        linked_lines(&run.stderr),
        vec![linked_line(&main, VIAL_QMK).as_str(), SUMMARY_OF_ONE],
        "stderr must name the link and then the count, but it reads:\n{}",
        run.stderr
    );
}

/// `-q` removes the line for each link and the summary, and the links still
/// exist.
#[test]
fn quiet_keeps_the_links_and_removes_their_lines() {
    let (temp, repo) = container(
        &[IGNORE_VIAL_QMK, IGNORE_ZMK_CONFIG],
        &[VIAL_QMK, ZMK_CONFIG],
    );
    let main = main_worktree(&repo);
    let branch = unique_branch("quiet");

    let run = successful_run(&temp, &repo, &branch, &[QUIET], None);

    assert_linked(&run.worktree, &main, &repo, VIAL_QMK);
    assert_linked(&run.worktree, &main, &repo, ZMK_CONFIG);
    assert!(
        linked_lines(&run.stderr).is_empty(),
        "-q must remove each {LINKED_WORD} line, but stderr reads:\n{}",
        run.stderr
    );
}

/// A repository without children gets no link and no new line. Every other
/// test of the output of `nwt` depends on this rule.
#[test]
fn a_repository_without_children_prints_nothing_new() {
    let (temp, repo) = init_repo();
    let branch = unique_branch("no-children");

    let run = successful_run(&temp, &repo, &branch, &[], None);

    let mentions: Vec<&str> = run
        .stderr
        .lines()
        .filter(|line| line.to_lowercase().contains(LINKED_WORD_LOWER))
        .collect();
    assert!(
        mentions.is_empty(),
        "a repository without children must print no line about links, but stderr reads:\n{}",
        run.stderr
    );
}

/// Give the owner the write permission on a directory again when the value
/// goes out of scope.
///
/// The temporary directory cannot delete the contents of a directory without
/// that permission. The value restores it also after a failed assertion.
#[cfg(unix)]
struct WritableAgain(PathBuf);

#[cfg(unix)]
impl Drop for WritableAgain {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;

        // The directory is not there when the run failed before git made it,
        // and then there is nothing to restore.
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(WRITABLE_MODE));
    }
}

/// A symlink that cannot be made gives no link, and a warning that names the
/// child and the error. `-q` keeps the warning, because a missing link is a
/// defect that the user must see.
///
/// A `post-checkout` hook removes the write permission of the new worktree, so
/// the symlink fails with the permission error of the operating system. The
/// test takes the expected error from the same failed operation.
#[cfg(unix)]
#[test]
fn a_link_that_fails_gives_a_warning_that_quiet_keeps() {
    let (temp, repo) = container(&[IGNORE_VIAL_QMK], &[VIAL_QMK]);
    let hooks = TempDir::new().expect("create the hooks directory");
    support::install_post_checkout_hook(&repo, hooks.path(), READ_ONLY_HOOK);
    let main = main_worktree(&repo);
    let branch = unique_branch("read-only");
    let _writable = WritableAgain(temp.path().join(WORKTREES_DIR_NAME).join(&branch));

    let run = successful_run(&temp, &repo, &branch, &[QUIET], None);

    let link = run.worktree.join(VIAL_QMK);
    assert!(
        fs::symlink_metadata(&link).is_err(),
        "{} must not exist after the link failed",
        link.display()
    );
    let error = std::os::unix::fs::symlink(main.join(VIAL_QMK), &link)
        .expect_err("the hook must leave the worktree read-only");
    let warning = format!("{NOT_LINKED_PREFIX} {VIAL_QMK} ({error})");
    assert!(
        run.stderr.lines().any(|line| line == warning),
        "stderr must hold the line {warning:?}, but it reads:\n{}",
        run.stderr
    );
}

/// Git gets the real name of a child whose name starts with `:`.
///
/// Git reads a leading `:` in a pathspec as magic, so a bare `:vial` asks
/// about `vial`. The `.gitignore` ignores `/:vial` and nothing else, so only a
/// question about the real name gets the answer that git ignores the path.
/// Windows does not permit `:` in a file name.
#[cfg(unix)]
#[test]
fn a_name_that_starts_with_a_colon_goes_to_git_as_a_path() {
    let (temp, repo) = container(&[IGNORE_COLON_CHILD], &[COLON_CHILD]);
    let main = main_worktree(&repo);
    let branch = unique_branch("colon");

    let run = successful_run(&temp, &repo, &branch, &[], None);

    assert_linked(&run.worktree, &main, &repo, COLON_CHILD);
    assert_eq!(
        linked_lines(&run.stderr),
        vec![linked_line(&main, COLON_CHILD).as_str(), SUMMARY_OF_ONE],
        "stderr must name the link and then the count, but it reads:\n{}",
        run.stderr
    );
}

/// Prove that nothing exists at `<worktree>/<name>`: no link, no directory, and
/// no file.
fn assert_not_there(worktree: &Path, name: &str) {
    let path = worktree.join(name);
    assert!(
        fs::symlink_metadata(&path).is_err(),
        "{} must not exist",
        path.display()
    );
}

/// `--no-link-children` makes no link and prints no line about links.
#[test]
fn the_flag_turns_the_links_off() {
    let (temp, repo) = container(&[IGNORE_VIAL_QMK], &[VIAL_QMK]);
    let branch = unique_branch("flag-off");

    let run = successful_run(&temp, &repo, &branch, &[NO_LINK_CHILDREN], None);

    assert_not_there(&run.worktree, VIAL_QMK);
    assert!(
        linked_lines(&run.stderr).is_empty(),
        "{NO_LINK_CHILDREN} must print no {LINKED_WORD} line, but stderr reads:\n{}",
        run.stderr
    );
}

/// `link_children = false` in `~/.nwt.toml` makes no link and prints no line
/// about links.
///
/// The config file goes into a home directory that this test owns. The shared
/// private home of `support` stays empty, so no other test reads this file.
#[test]
fn the_config_key_turns_the_links_off() {
    let (temp, repo) = container(&[IGNORE_VIAL_QMK], &[VIAL_QMK]);
    let home = TempDir::new().expect("create the home directory of the run");
    write_file(home.path(), CONFIG_FILE, LINKS_OFF_CONFIG);
    let branch = unique_branch("config-off");

    let run = successful_run(&temp, &repo, &branch, &[], Some(home.path()));

    assert_not_there(&run.worktree, VIAL_QMK);
    assert!(
        linked_lines(&run.stderr).is_empty(),
        "{LINKS_OFF_CONFIG:?} must stop each {LINKED_WORD} line, but stderr reads:\n{}",
        run.stderr
    );
}

/// A container with the ignored child [`VIAL`], and an untracked `.env` in the
/// child.
fn container_with_child_env() -> (TempDir, PathBuf) {
    let (temp, repo) = container(&[IGNORE_VIAL], &[VIAL]);
    write_file(&repo.join(VIAL), ENV_FILE, CHILD_ENV);
    (temp, repo)
}

/// The `.env` of a linked child reaches the worktree through the link. The
/// `.env` copy does not go into the child, so it prints no `Kept existing:`
/// line for a file that the link already gives, and the file of the child
/// stays as it was.
#[test]
fn the_env_file_of_a_linked_child_stays_in_the_child() {
    let (temp, repo) = container_with_child_env();
    let main = main_worktree(&repo);
    let branch = unique_branch("env-linked");

    let run = successful_run(&temp, &repo, &branch, &[], None);

    assert_linked(&run.worktree, &main, &repo, VIAL);
    assert!(
        !run.stderr.contains(KEPT_EXISTING),
        "the .env copy must not go through the link into the child, but stderr reads:\n{}",
        run.stderr
    );
    let env_path = repo.join(VIAL).join(ENV_FILE);
    assert_eq!(
        fs::read_to_string(&env_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", env_path.display())),
        CHILD_ENV,
        "the .env of the child must stay as it was"
    );
}

/// Without links, the `.env` of a child makes no directory in the worktree. A
/// directory that holds only `.env` files is not a usable child.
#[test]
fn the_env_file_of_a_child_makes_no_directory_without_links() {
    let (temp, repo) = container_with_child_env();
    let branch = unique_branch("env-unlinked");

    let run = successful_run(&temp, &repo, &branch, &[NO_LINK_CHILDREN], None);

    assert_not_there(&run.worktree, VIAL);
}

/// The lines of `stderr` that hold the child `name` as a word.
///
/// A word is a run of characters between spaces. The line for a link names the
/// target path too, and that path is a different word.
fn lines_naming<'a>(stderr: &'a str, name: &str) -> Vec<&'a str> {
    stderr
        .lines()
        .filter(|line| line.split_whitespace().any(|word| word == name))
        .collect()
}

/// The line for the child `name`, whose path the new worktree already holds.
fn already_there_line(name: &str) -> String {
    format!("{ALREADY_THERE_PREFIX} {name} ({ALREADY_THERE_REASON})")
}

/// Prove that `stderr` holds `expected`, and no other line that names the child
/// `name`.
fn assert_only_line_naming(stderr: &str, name: &str, expected: &str) {
    assert_eq!(
        lines_naming(stderr, name),
        vec![expected],
        "stderr must hold one line about {name}, but it reads:\n{stderr}"
    );
}

/// A directory that the branch of the new worktree tracks stays as git wrote
/// it, and one line says that the child has no link.
///
/// `nwt -c <side>` checks out the branch that
/// [`container_with_tracked_directory`] makes, so git writes [`TRACKED_FILE`]
/// into the new worktree. Git does not ignore a directory that holds a tracked
/// file. Thus the check for a path that is already there comes before the
/// question to git. Without it, this child gets a warning about `.gitignore`,
/// and `.gitignore` is correct.
#[test]
fn a_tracked_directory_stays_and_one_line_says_so() {
    let side = unique_branch("side");
    let (temp, repo) = container_with_tracked_directory(&side);

    let run = successful_start(&temp, &repo, Start::Checkout(&side), &[], None);

    let dir = run.worktree.join(VIAL);
    let kind = fs::symlink_metadata(&dir)
        .unwrap_or_else(|e| panic!("{} is not there: {e}", dir.display()))
        .file_type();
    assert!(kind.is_dir(), "{} must stay a directory", dir.display());
    let names: Vec<String> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("read an entry of {}: {e}", dir.display()))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        vec![TRACKED_FILE_NAME],
        "{} must hold only the tracked file",
        dir.display()
    );
    let tracked = dir.join(TRACKED_FILE_NAME);
    assert_eq!(
        fs::read_to_string(&tracked).unwrap_or_else(|e| panic!("read {}: {e}", tracked.display())),
        TRACKED_CONTENT,
        "the tracked file must stay as git wrote it"
    );
    assert_only_line_naming(&run.stderr, VIAL, &already_there_line(VIAL));
}

/// The body of a `post-checkout` hook that writes the regular file [`VIAL`]
/// into the new worktree.
#[cfg(unix)]
fn file_hook_body() -> String {
    format!(
        "printf '%s' {} > {VIAL}\n",
        shellquote::shell_quote(HOOK_FILE_CONTENT)
    )
}

/// A container with the ignored child [`VIAL`], and a `post-checkout` hook with
/// `body`.
///
/// Hand back the temporary directories of the container and of the hook (keep
/// both alive) and the container.
#[cfg(unix)]
fn container_with_hook(body: &str) -> (TempDir, TempDir, PathBuf) {
    let (temp, repo) = container(&[IGNORE_VIAL], &[VIAL]);
    let hooks = TempDir::new().expect("create the hooks directory");
    support::install_post_checkout_hook(&repo, hooks.path(), body);
    (temp, hooks, repo)
}

/// Prove that `<worktree>/vial` is the regular file that [`file_hook_body`]
/// writes.
#[cfg(unix)]
fn assert_hook_file(worktree: &Path) {
    let file = worktree.join(VIAL);
    let kind = fs::symlink_metadata(&file)
        .unwrap_or_else(|e| panic!("{} is not there: {e}", file.display()))
        .file_type();
    assert!(
        kind.is_file(),
        "{} must stay a regular file",
        file.display()
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap_or_else(|e| panic!("read {}: {e}", file.display())),
        HOOK_FILE_CONTENT,
        "the file must stay as the hook wrote it"
    );
}

/// A regular file that a `post-checkout` hook writes at the path of a child
/// stays as the hook wrote it, and one line says that the child has no link.
///
/// Git runs the hook in the new worktree during `git worktree add`, so the file
/// is there before `nwt` links the children. `/vial` ignores the file.
#[cfg(unix)]
#[test]
fn a_file_from_a_post_checkout_hook_stays_and_one_line_says_so() {
    let (temp, _hooks, repo) = container_with_hook(&file_hook_body());
    let branch = unique_branch("hook-file");

    let run = successful_run(&temp, &repo, &branch, &[], None);

    assert_hook_file(&run.worktree);
    assert_only_line_naming(&run.stderr, VIAL, &already_there_line(VIAL));
}

/// `-q` removes the line for a path that the new worktree already holds,
/// because that line names no defect. The path stays.
#[cfg(unix)]
#[test]
fn quiet_removes_the_line_for_a_path_that_is_already_there() {
    let (temp, _hooks, repo) = container_with_hook(&file_hook_body());
    let branch = unique_branch("hook-file-quiet");

    let run = successful_run(&temp, &repo, &branch, &[QUIET], None);

    assert_hook_file(&run.worktree);
    assert!(
        lines_naming(&run.stderr, VIAL).is_empty(),
        "-q must remove each line about {VIAL}, but stderr reads:\n{}",
        run.stderr
    );
}

/// A broken symlink that a `post-checkout` hook makes at the path of a child
/// stays as the hook made it, and one line says that the child has no link.
///
/// The symlink points at a path that does not exist. A check that follows the
/// link sees nothing there, so `nwt` must read the entry itself.
#[cfg(unix)]
#[test]
fn a_broken_symlink_from_a_post_checkout_hook_stays_and_one_line_says_so() {
    let (temp, repo) = container(&[IGNORE_VIAL], &[VIAL]);
    let missing = temp.path().join(MISSING_TARGET);
    let body = format!(
        "ln -s {} {VIAL}\n",
        shellquote::shell_quote(missing.to_str().expect("utf-8 target path"))
    );
    let hooks = TempDir::new().expect("create the hooks directory");
    support::install_post_checkout_hook(&repo, hooks.path(), &body);
    let branch = unique_branch("hook-broken-link");

    let run = successful_run(&temp, &repo, &branch, &[], None);

    let link = run.worktree.join(VIAL);
    let kind = fs::symlink_metadata(&link)
        .unwrap_or_else(|e| panic!("{} is not there: {e}", link.display()))
        .file_type();
    assert!(kind.is_symlink(), "{} must stay a symlink", link.display());
    assert_eq!(
        fs::read_link(&link).unwrap_or_else(|e| panic!("read link {}: {e}", link.display())),
        missing,
        "the symlink must keep the target that the hook gave it"
    );
    assert!(
        fs::symlink_metadata(&missing).is_err(),
        "the target {} must not exist",
        missing.display()
    );
    assert_only_line_naming(&run.stderr, VIAL, &already_there_line(VIAL));
}

/// Run `nwt` in `repo` once without `-q` and once with it, and prove each
/// time that [`VIAL`] has no link, that `warning` is the only line about it,
/// and that [`VIAL_QMK`] still has its link.
///
/// `-q` keeps the warning, because the warning names a defect in the
/// repository. [`VIAL`] comes before [`VIAL_QMK`] in the order of the names,
/// so the link of [`VIAL_QMK`] proves that one child without a link does not
/// stop the others.
fn assert_not_linked_with_warning(temp: &TempDir, repo: &Path, label: &str, warning: &str) {
    let main = main_worktree(repo);

    for extra in [&[][..], &[QUIET][..]] {
        let branch = unique_branch(label);

        let run = successful_run(temp, repo, &branch, extra, None);

        assert_not_there(&run.worktree, VIAL);
        assert_only_line_naming(&run.stderr, VIAL, warning);
        assert_linked(&run.worktree, &main, repo, VIAL_QMK);
    }
}

/// A child that only a pattern with a trailing slash ignores gets no link, and
/// a warning that names the pattern, its file, its line, and the fix.
///
/// Git sees a symlink as a file, so `vial/` does not ignore a link at `vial`.
/// The first question to git thus says that git does not ignore the path. The
/// second question asks about `vial/`, which the pattern matches, so git names
/// the pattern.
#[test]
fn a_pattern_with_a_trailing_slash_gives_a_warning_that_names_it() {
    let (temp, repo) = container(&[DIRECTORY_ONLY_VIAL, IGNORE_VIAL_QMK], &[VIAL, VIAL_QMK]);

    assert_not_linked_with_warning(&temp, &repo, "slash", DIRECTORY_ONLY_WARNING);
}

/// A child that no pattern ignores gets no link, and a warning that gives the
/// fix.
#[test]
fn a_child_that_no_pattern_ignores_gives_a_warning_with_the_fix() {
    let (temp, repo) = container(&[IGNORE_VIAL_QMK], &[VIAL, VIAL_QMK]);

    assert_not_linked_with_warning(&temp, &repo, "no-pattern", NO_PATTERN_WARNING);
}

/// A child that git gives no answer about gets no link, and a warning that
/// repeats the error of git. `-q` keeps the warning, and the run still
/// succeeds.
///
/// A `post-checkout` hook writes a `.git` file into the new worktree that names
/// a git directory that does not exist. Each later git command in the new
/// worktree then fails with "not a git repository" and the status 128. The
/// `.env` copy and the hook bootstrap stay off, and the check for a missing
/// hooks directory ignores a git that fails, so nothing else stops the run.
/// The test takes the expected error from the same failed question.
#[cfg(unix)]
#[test]
fn a_git_that_gives_no_answer_gives_a_warning_that_quiet_keeps() {
    let (temp, repo) = container(&[IGNORE_VIAL], &[VIAL]);
    let missing = temp.path().join(MISSING_GIT_DIR);
    let body = format!(
        "printf 'gitdir: %s\\n' {} > .git\n",
        shellquote::shell_quote(missing.to_str().expect("utf-8 git directory path"))
    );
    let hooks = TempDir::new().expect("create the hooks directory");
    support::install_post_checkout_hook(&repo, hooks.path(), &body);

    for extra in [&[NO_COPY_ENV][..], &[NO_COPY_ENV, QUIET][..]] {
        let branch = unique_branch("no-answer");

        let run = successful_run(&temp, &repo, &branch, extra, None);

        assert_not_there(&run.worktree, VIAL);
        let git_error = support::git_failure_stderr(
            &run.worktree,
            &["check-ignore", "-q", "--", &format!("./{VIAL}")],
        );
        let first_line = git_error
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_else(|| panic!("git wrote no error in {}", run.worktree.display()));
        let warning = format!("{NOT_LINKED_PREFIX} {VIAL} ({GIT_FAILED_REASON} {first_line})");
        assert_only_line_naming(&run.stderr, VIAL, &warning);
    }
}

/// Only a directory one level below the main worktree that holds a `.git`
/// entry is a child. A directory without `.git`, a regular file, and a child
/// of a child get no link and no line, even when `.gitignore` ignores each of
/// them.
///
/// [`INNER`] is a repository, but it sits below [`OUTER_DIR`], which is not
/// one. `nwt` links the children of the main worktree only, and not a child of
/// a child.
#[test]
fn only_a_directory_that_holds_git_is_a_child() {
    let (temp, repo) = container(&IGNORE_NOT_CHILDREN, &[VIAL]);
    write_file(&repo.join(PLAIN_DIR), CHILD_FILE, "not a repository\n");
    write_file(&repo, PLAIN_FILE, "a file\n");
    let outer = repo.join(OUTER_DIR);
    fs::create_dir(&outer).unwrap_or_else(|e| panic!("create {}: {e}", outer.display()));
    make_child(&outer, INNER);
    let main = main_worktree(&repo);
    let branch = unique_branch("not-children");

    let run = successful_run(&temp, &repo, &branch, &[], None);

    assert_linked(&run.worktree, &main, &repo, VIAL);
    for name in [PLAIN_DIR, PLAIN_FILE, OUTER_DIR, INNER] {
        assert_not_there(&run.worktree, name);
        assert!(
            lines_naming(&run.stderr, name).is_empty(),
            "stderr must not name {name}, but it reads:\n{}",
            run.stderr
        );
    }
    assert_eq!(
        linked_lines(&run.stderr),
        vec![linked_line(&main, VIAL).as_str(), SUMMARY_OF_ONE],
        "stderr must name only the link of {VIAL}, but it reads:\n{}",
        run.stderr
    );
}

/// `nwt` in a linked worktree links the children of the main worktree, and not
/// the links of the worktree it runs in.
///
/// The first run makes the worktree W1, and W1 holds the link [`VIAL`]. The
/// second run starts in W1. W1 holds a `vial` that looks like a child, because
/// the link reaches a directory that holds `.git`. A run that reads the
/// children of the checkout it stands in thus links `W2/vial` to `W1/vial`, a
/// chain through W1. That chain breaks when W1 goes away. The target must be
/// the main worktree that git names, joined with the child, and the new
/// worktree must land beside the main worktree.
#[test]
fn a_run_from_a_linked_worktree_links_the_children_of_the_main_worktree() {
    let (temp, repo) = container(&[IGNORE_VIAL], &[VIAL]);
    let main = main_worktree(&repo);
    let first = successful_run(&temp, &repo, &unique_branch("first"), &[], None);
    assert_linked(&first.worktree, &main, &repo, VIAL);
    let second_branch = unique_branch("second");

    let second = successful_run(&temp, &first.worktree, &second_branch, &[], None);

    assert_linked(&second.worktree, &main, &repo, VIAL);
    assert_eq!(
        linked_lines(&second.stderr),
        vec![linked_line(&main, VIAL).as_str(), SUMMARY_OF_ONE],
        "stderr must name the target in the main worktree, but it reads:\n{}",
        second.stderr
    );
}
