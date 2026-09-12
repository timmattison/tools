//! Pins that the `nwt` **binary** acts on the repository it was pointed at,
//! whatever git the environment carries.
//!
//! `tests/git-env-isolation.rs` covers the shared test fixture. This file
//! covers production: it runs the built binary, so every git child `nwt` spawns
//! is measured, and a spawn that sheds nothing is caught here and nowhere else.
//!
//! The gap this closes is narrow and worth stating. A spawn that sheds nothing
//! is not written in a shape a rule can name, so `repo_guards::git_env_sweep`
//! cannot report one and says so in its own documentation. It proves no call
//! site is written as a hand-written list of variable names. It does not prove
//! a given spawn sheds anything at all. The only thing that proves *that* is a
//! run of the binary in a hostile environment, with the damage measured on the
//! file system afterwards. Removing the sweep from a production spawn left
//! every test of `nwt` and of `repo-guards` green, which is how this file came
//! to exist.
//!
//! The two tests are the two halves of one rule, which
//! `gitscratch::shed_inherited_git_environment_keeping_user_intent` states:
//! shed the whole `GIT_` prefix, and keep the six names of
//! `gitscratch::USER_INTENT_GIT_ENVIRONMENT`. A test of the shedding half alone
//! passes for a tool that sheds everything, and such a tool breaks the user who
//! states `GIT_SSH_COMMAND` or `GIT_CONFIG_GLOBAL` on purpose. So the keep half
//! is pinned beside it.
//!
//! Every variable below is set on the **child command**, and nothing here
//! touches the environment of this process. Cargo runs the tests of one binary
//! on parallel threads, so a process-wide variable would aim the git children
//! of a sibling thread at this file's decoy. That is why this file may hold
//! more than one test where `tests/git-env-isolation.rs`, which does mutate the
//! process environment, holds exactly one.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use support::{git_stdout, init_repo, nanos, nwt_command};

/// The suffix `nwt` adds to the repository name to name the directory that
/// holds every new worktree.
const WORKTREES_SUFFIX: &str = "-worktrees";

/// The prefix of the line that names a worktree in `git worktree list
/// --porcelain`.
const WORKTREE_LINE_PREFIX: &str = "worktree ";

/// The configuration key `nwt` reads to decide whether a new worktree runs
/// hooks at all.
const HOOKS_PATH_KEY: &str = "core.hooksPath";

/// The name of the directory the stated global configuration points
/// [`HOOKS_PATH_KEY`] at.
///
/// Nothing ever creates it. A hooks directory that is not there is what makes
/// `nwt` warn, so the absence is the whole fixture.
const ABSENT_HOOKS_DIR: &str = "no-such-hooks-directory";

/// The file name of the stated global configuration.
const GLOBAL_CONFIG_FILE: &str = "stated-global-config";

/// What one path of a snapshot holds.
///
/// A directory carries no content of its own, a file carries its bytes, and a
/// link carries the path it names. Reading a link rather than following it
/// keeps the snapshot a statement about this directory alone.
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

/// Resolve a path before an assertion reads it.
///
/// Git prints resolved paths, and every fixture lives under a temporary
/// directory that macOS reaches through a symbolic link: `/var` resolves to
/// `/private/var`.
///
/// # Panics
///
/// Panics if the path cannot be resolved.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
}

/// A branch name that no concurrent copy of this suite can also hold.
///
/// Two `cargo test` runs share one machine, and a branch name is a shared
/// resource. The process id and a nanosecond clock reading keep them apart.
fn unique_branch(label: &str) -> String {
    format!("{label}-{}-{}", std::process::id(), nanos())
}

/// The directory `nwt` puts every new worktree in: the name of `main_worktree`
/// plus [`WORKTREES_SUFFIX`], beside `main_worktree`.
///
/// # Panics
///
/// Panics if `main_worktree` has no name, or a name that is not UTF-8.
fn expected_worktrees_dir(main_worktree: &Path) -> PathBuf {
    let main_worktree = canonical(main_worktree);
    let name = main_worktree
        .file_name()
        .expect("the main worktree has a name")
        .to_str()
        .expect("utf-8 main worktree name");

    main_worktree.with_file_name(format!("{name}{WORKTREES_SUFFIX}"))
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
/// A count of refs or of objects is not enough to say a repository is
/// untouched: a write that replaces one object with another keeps the count.
/// The whole tree, content included, is what makes the word "byte-identical"
/// true.
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

/// Every worktree the repository at `main_worktree` holds, resolved.
fn listed_worktrees(main_worktree: &Path) -> Vec<PathBuf> {
    git_stdout(main_worktree, &["worktree", "list", "--porcelain"])
        .lines()
        .filter_map(|line| line.strip_prefix(WORKTREE_LINE_PREFIX))
        .map(|path| canonical(Path::new(path)))
        .collect()
}

/// The commit `HEAD` of the repository at `repo` names.
fn head_commit(repo: &Path) -> String {
    git_stdout(repo, &["rev-parse", "HEAD"]).trim().to_string()
}

/// The binary must act on the repository it stands in, and leave the repository
/// the environment names exactly as it found it.
///
/// Git obeys the environment before it obeys the directory a command was
/// pointed at, and it exports its own variables into every hook it runs. So a
/// `nwt` run started from a pre-commit hook, from `git bisect run`, or from
/// `rebase --exec` carries `GIT_DIR`, `GIT_INDEX_FILE` and
/// `GIT_CONFIG_PARAMETERS` that name the repository being committed to. A git
/// child of `nwt` that keeps them acts on that repository instead.
///
/// The five variables below are aimed at a decoy repository, and cover four
/// distinct ways a leaked variable redirects git: the repository it finds
/// (`GIT_DIR`, `GIT_WORK_TREE`), the index it stages into (`GIT_INDEX_FILE`),
/// the store it writes objects to (`GIT_OBJECT_DIRECTORY`), and the
/// configuration it reads (`GIT_CONFIG_PARAMETERS`). The last one carries no
/// path at all, so no list of location names ever catches it; only a sweep of
/// the whole prefix does.
///
/// Three assertions, and each one has a job. The decoy must be byte-identical,
/// which is the damage. The worktree must land beside the source repository
/// *and* be a worktree of it, which stops the first assertion passing merely
/// because the run made nothing anywhere. The history of the source repository
/// must be unchanged, because a leaked `GIT_INDEX_FILE` writes into a
/// repository as readily as it reads from one.
#[test]
fn the_binary_leaves_the_repository_the_environment_names_untouched() {
    // Two repositories under two temporary directories. The source is the one
    // `nwt` runs in and the only one it may touch. The decoy stands in for the
    // repository a launching hook exported, which on a real machine is the
    // developer's own.
    let (_source_temp, source) = init_repo();
    let (_decoy_temp, decoy) = init_repo();

    let decoy_git_dir = decoy.join(".git");
    let before = snapshot(&decoy);
    let source_head_before = head_commit(&source);
    let branch = unique_branch("hostile-env");

    // Every variable goes on the child, never on this process: a sibling test
    // thread spawns git of its own, and a process-wide variable would aim it
    // here.
    let output = nwt_command(&source)
        .args(["-b", &branch, "--no-copy-env", "--no-bootstrap-hooks"])
        .env("GIT_DIR", &decoy_git_dir)
        .env("GIT_WORK_TREE", &decoy)
        .env("GIT_INDEX_FILE", decoy_git_dir.join("index"))
        .env("GIT_OBJECT_DIRECTORY", decoy_git_dir.join("objects"))
        .env("GIT_CONFIG_PARAMETERS", "'nwt.envleakprobe=leaked'")
        .output()
        .expect("run the nwt binary");

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let after = snapshot(&decoy);

    // The damage. `git worktree add` is the spawn that writes, and it writes a
    // branch ref, a reflog and a whole `worktrees/<name>` directory into
    // whichever repository it reached.
    let changed = difference(&before, &after);
    assert!(
        changed.is_empty(),
        "nwt acted on the repository the environment named instead of the one it \
         stands in. A run started from a git hook therefore writes into the \
         repository being committed to. {} path(s) of the decoy at {} changed:\n{}\n\
         nwt stdout:\n{stdout}\nnwt stderr:\n{stderr}",
        changed.len(),
        decoy.display(),
        changed.join("\n"),
    );

    // The control against a vacuous pass. A run that made nothing anywhere
    // leaves the decoy byte-identical too.
    assert!(
        output.status.success(),
        "nwt -b {branch} failed in {}:\n{stdout}\n{stderr}",
        source.display(),
    );

    let printed = PathBuf::from(stdout.trim());
    assert!(
        printed.is_dir(),
        "nwt printed {}, which is no directory:\n{stdout}\n{stderr}",
        printed.display(),
    );
    assert_eq!(
        canonical(&printed),
        expected_worktrees_dir(&source).join(&branch),
        "nwt must put the worktree beside {}",
        source.display(),
    );

    // Where the directory sits is only half of "landed beside the source
    // repository". The other half is whose worktree it is: a `git worktree add`
    // that reached the decoy still makes the directory in the right place, and
    // checks the decoy out into it.
    assert!(
        listed_worktrees(&source).contains(&canonical(&printed)),
        "git must count {} as a worktree of the repository at {}, and it counts \
         these instead: {:?}",
        printed.display(),
        source.display(),
        listed_worktrees(&source),
    );

    assert_eq!(
        head_commit(&source),
        source_head_before,
        "nwt must add no commit to the repository it runs in",
    );
}

/// A `GIT_CONFIG_GLOBAL` the user stated must still reach the check that reads
/// [`HOOKS_PATH_KEY`].
///
/// This is the keep half of the rule, and it is the half a blanket sweep
/// breaks. `missing_hooks_path` predicts what git reads at commit time, and it
/// reads four steps of precedence: the worktree, the repository, the global
/// file and the system file. `GIT_CONFIG_GLOBAL` names the global file. Shed
/// it, and the prediction is made against a file the user replaced, so a
/// worktree whose hooks directory is missing is reported as gated and every
/// commit in it is silently ungated.
///
/// The keep is safe where a strip-list is not, because the two go stale in
/// opposite directions. A name git invents next year is shed by default, and
/// the cost of that is one setting a user states again. A strip-list that goes
/// stale inherits the new name, and the cost of that is a repository written
/// into by mistake.
///
/// Nothing hostile is set here. The point is the opposite of the test above:
/// one variable, stated by the user, that must survive.
#[test]
fn a_stated_global_configuration_still_reaches_the_hooks_check() {
    let (temp, repo) = init_repo();

    // A hooks directory that is not there. Git runs no hook at all in that
    // case, and says nothing about it, which is what `nwt` warns for.
    let absent_hooks_dir = temp.path().join(ABSENT_HOOKS_DIR);
    let global_config = temp.path().join(GLOBAL_CONFIG_FILE);
    fs::write(
        &global_config,
        format!(
            "[core]\n\thooksPath = {}\n",
            absent_hooks_dir
                .to_str()
                .expect("utf-8 absent hooks directory")
        ),
    )
    .unwrap_or_else(|e| panic!("write {}: {e}", global_config.display()));

    let branch = unique_branch("stated-global");
    let output = nwt_command(&repo)
        .args(["-b", &branch, "--no-copy-env", "--no-bootstrap-hooks"])
        .env("GIT_CONFIG_GLOBAL", &global_config)
        .output()
        .expect("run the nwt binary");

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        output.status.success(),
        "nwt -b {branch} failed in {}:\n{stdout}\n{stderr}",
        repo.display(),
    );

    // Git prints a path value back as the configuration file spells it, so the
    // warning names the path as written rather than as resolved.
    let stated = absent_hooks_dir
        .to_str()
        .expect("utf-8 absent hooks directory");
    assert!(
        stderr.contains(stated),
        "nwt must read {HOOKS_PATH_KEY} through the global configuration file the \
         user stated in GIT_CONFIG_GLOBAL, and warn that {stated} is not there. A \
         sweep that sheds that variable reads some other file, finds no key, and \
         reports an ungated worktree as gated.\nnwt stdout:\n{stdout}\nnwt \
         stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(HOOKS_PATH_KEY),
        "the warning must name the key the user has to fix, and it reads:\n{stderr}"
    );
}
