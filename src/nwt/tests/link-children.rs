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
    assert!(
        run_git(
            &repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "ignore the children"
            ]
        ),
        "git commit failed"
    );

    for child in children {
        make_child(&repo, child);
    }

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

/// Run `nwt -b <branch> --no-bootstrap-hooks <extra>` in `repo`.
///
/// `home`, when it is there, becomes the home directory of the child. It wins
/// over the private home of `support::nwt_command`, because this call sets
/// `HOME` after that function sets it.
fn run_nwt(repo: &Path, branch: &str, extra: &[&str], home: Option<&Path>) -> Output {
    let mut command = nwt_command(repo);
    command.args(["-b", branch, NO_BOOTSTRAP_HOOKS]).args(extra);
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

/// Run `nwt` as [`run_nwt`] does, and prove that it succeeded and that its
/// stdout holds only the path of the new worktree.
///
/// The shell wrapper does `dir=$(command nwt "$@")`, so any other line on
/// stdout breaks the `cd` into the worktree.
fn successful_run(
    temp: &TempDir,
    repo: &Path,
    branch: &str,
    extra: &[&str],
    home: Option<&Path>,
) -> Run {
    let output = run_nwt(repo, branch, extra, home);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        output.status.success(),
        "nwt -b {branch} {extra:?} failed in {}:\n{stdout}\n{stderr}",
        repo.display()
    );

    let printed = stdout.lines().next().unwrap_or_default().to_string();
    assert_eq!(
        stdout,
        format!("{printed}\n"),
        "stdout must hold only the worktree path, and stderr reads:\n{stderr}"
    );
    let worktree = PathBuf::from(printed);
    assert_eq!(
        canonical(&worktree),
        canonical(temp.path()).join(WORKTREES_DIR_NAME).join(branch),
        "nwt printed a path that is not the new worktree"
    );

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
