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
//! site passes a `GIT_`-prefixed name to `env_remove` as a literal, and a list
//! of those names held in a constant and applied in a loop walks through even
//! that. It does not prove a given spawn sheds anything at all. The only thing
//! that proves *that* is a run of the binary in a hostile environment, with the
//! damage measured on the file system afterwards. Removing the sweep from a
//! production spawn left every test of `nwt` and of `repo-guards` green, which
//! is how this file came to exist.
//!
//! Two tests are the two halves of one rule, which
//! `gitscratch::shed_inherited_git_environment_keeping_user_intent` states:
//! shed the whole `GIT_` prefix, and keep the six names of
//! `gitscratch::USER_INTENT_GIT_ENVIRONMENT`. A test of the shedding half alone
//! passes for a tool that sheds everything, and such a tool breaks the user who
//! states `GIT_SSH_COMMAND` or `GIT_CONFIG_GLOBAL` on purpose. So the keep half
//! is pinned beside it.
//!
//! `--sparse-exclude` adds git children of its own: `git ls-tree` for the
//! check, `git sparse-checkout set`, `git read-tree -mu HEAD`,
//! `git rev-parse HEAD`, and `git hook run`. With `-c`, the run also asks git
//! which remote branch its checkout DWIM takes. Two more tests hold the
//! shedding half for each of those children: one with `-b` and a
//! `post-checkout` hook, and one with `-c` for a branch that only a remote
//! holds.
//!
//! Every variable below is set on the **child command**, and nothing here
//! touches the environment of this process. Cargo runs the tests of one binary
//! on parallel threads, so a process-wide variable would aim the git children
//! of a sibling thread at this file's decoy. That is why this file may hold
//! more than one test where `tests/git-env-isolation.rs`, which does mutate the
//! process environment, holds exactly one.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use support::{
    clone_of, difference, git_stdout, init_repo, nanos, nwt_command, repo_with_files, run_git,
    snapshot, Snapshot,
};

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

/// The `nwt` command for `source`, with five git variables that aim git at the
/// repository at `decoy`.
///
/// The five variables cover four distinct ways a leaked variable redirects
/// git: the repository it finds (`GIT_DIR`, `GIT_WORK_TREE`), the index it
/// stages into (`GIT_INDEX_FILE`), the store it writes objects to
/// (`GIT_OBJECT_DIRECTORY`), and the configuration it reads
/// (`GIT_CONFIG_PARAMETERS`). The last one carries no path at all, so no list
/// of location names ever catches it; only a sweep of the whole prefix does.
///
/// Every variable goes on the child, never on this process: a sibling test
/// thread spawns git of its own, and a process-wide variable would aim it at
/// the decoy.
fn hostile_nwt_command(source: &Path, decoy: &Path) -> Command {
    let decoy_git_dir = decoy.join(".git");
    let mut command = nwt_command(source);
    command
        .env("GIT_DIR", &decoy_git_dir)
        .env("GIT_WORK_TREE", decoy)
        .env("GIT_INDEX_FILE", decoy_git_dir.join("index"))
        .env("GIT_OBJECT_DIRECTORY", decoy_git_dir.join("objects"))
        .env("GIT_CONFIG_PARAMETERS", "'nwt.envleakprobe=leaked'");
    command
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
/// The five variables of [`hostile_nwt_command`] are aimed at a decoy
/// repository. [`DecoyWatch::assert_untouched`] holds the three assertions of
/// this rule, so the sparse runs below hold the same three.
#[test]
fn the_binary_leaves_the_repository_the_environment_names_untouched() {
    // Two repositories under two temporary directories. The source is the one
    // `nwt` runs in and the only one it may touch. The decoy stands in for the
    // repository a launching hook exported, which on a real machine is the
    // developer's own.
    let (_source_temp, source) = init_repo();
    let (_decoy_temp, decoy) = init_repo();

    let watch = DecoyWatch::before_the_run(&source, &decoy);
    let branch = unique_branch("hostile-env");

    let output = hostile_nwt_command(&source, &decoy)
        .args(["-b", &branch, "--no-copy-env", "--no-bootstrap-hooks"])
        .output()
        .expect("run the nwt binary");

    watch.assert_untouched(&output, Some(&branch));
}

/// What a decoy test reads before its run, so that it can say after the run
/// what the run changed.
struct DecoyWatch {
    /// The repository `nwt` runs in, and the only one it may touch.
    source: PathBuf,
    /// The repository the hostile environment names.
    decoy: PathBuf,
    /// Every path of the decoy, with its bytes, before the run.
    before: Snapshot,
    /// The commit `HEAD` of the source named before the run.
    source_head_before: String,
}

impl DecoyWatch {
    /// Read the decoy and the `HEAD` of the source, before the run.
    fn before_the_run(source: &Path, decoy: &Path) -> Self {
        Self {
            source: source.to_path_buf(),
            decoy: decoy.to_path_buf(),
            before: snapshot(decoy),
            source_head_before: head_commit(source),
        }
    }

    /// Demand that the run that wrote `output` left the decoy byte-identical,
    /// and made a worktree of the source in the worktrees directory of the
    /// source. Hand back the worktree path that the run printed.
    ///
    /// `worktree_name` is the name of the worktree directory when the run
    /// states it with `-b`. A run without `-b` gets a random name, and the
    /// assertion then reads only the directory that holds the worktree.
    ///
    /// Three assertions, and each one has a job. The decoy must be
    /// byte-identical, which is the damage. The worktree must land beside the
    /// source repository *and* be a worktree of it, which stops the first
    /// assertion passing merely because the run made nothing anywhere. The
    /// history of the source repository must be unchanged, because a leaked
    /// `GIT_INDEX_FILE` writes into a repository as readily as it reads from
    /// one.
    fn assert_untouched(&self, output: &Output, worktree_name: Option<&str>) -> PathBuf {
        let source = &self.source;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let after = snapshot(&self.decoy);

        // The damage. `git worktree add` is the spawn that writes, and it writes a
        // branch ref, a reflog and a whole `worktrees/<name>` directory into
        // whichever repository it reached.
        let changed = difference(&self.before, &after);
        assert!(
            changed.is_empty(),
            "nwt acted on the repository the environment named instead of the one it \
             stands in. A run started from a git hook therefore writes into the \
             repository being committed to. {} path(s) of the decoy at {} changed:\n{}\n\
             nwt stdout:\n{stdout}\nnwt stderr:\n{stderr}",
            changed.len(),
            self.decoy.display(),
            changed.join("\n"),
        );

        // The control against a vacuous pass. A run that made nothing anywhere
        // leaves the decoy byte-identical too.
        assert!(
            output.status.success(),
            "nwt failed in {}:\n{stdout}\n{stderr}",
            source.display(),
        );

        let printed = PathBuf::from(stdout.trim());
        assert!(
            printed.is_dir(),
            "nwt printed {}, which is no directory:\n{stdout}\n{stderr}",
            printed.display(),
        );
        let resolved = canonical(&printed);
        let worktrees_dir = expected_worktrees_dir(source);
        match worktree_name {
            Some(name) => assert_eq!(
                resolved,
                worktrees_dir.join(name),
                "nwt must put the worktree beside {}",
                source.display(),
            ),
            None => assert_eq!(
                resolved.parent(),
                Some(worktrees_dir.as_path()),
                "nwt must put the worktree beside {}",
                source.display(),
            ),
        }

        // Where the directory sits is only half of "landed beside the source
        // repository". The other half is whose worktree it is: a `git worktree add`
        // that reached the decoy still makes the directory in the right place, and
        // checks the decoy out into it.
        assert!(
            listed_worktrees(source).contains(&canonical(&printed)),
            "git must count {} as a worktree of the repository at {}, and it counts \
             these instead: {:?}",
            printed.display(),
            source.display(),
            listed_worktrees(source),
        );

        assert_eq!(
            head_commit(source),
            self.source_head_before,
            "nwt must add no commit to the repository it runs in",
        );

        printed
    }
}

/// The tracked directory the sparse decoy runs exclude.
const HEAVY_DIR: &str = "heavy";

/// A tracked file under [`HEAVY_DIR`].
const HEAVY_FILE: &str = "heavy/big.txt";

/// A tracked file that a sparse decoy run keeps.
const KEPT_FILE: &str = "light.txt";

/// The branch that the clone of the `-c` decoy run holds only as
/// `origin/<branch>`.
const REMOTE_ONLY_BRANCH: &str = "foo";

/// Demand that the worktree at `worktree` holds [`KEPT_FILE`] and no
/// [`HEAVY_DIR`].
fn assert_heavy_dir_excluded(worktree: &Path) {
    assert!(
        !worktree.join(HEAVY_DIR).exists(),
        "--sparse-exclude {HEAVY_DIR} must keep {HEAVY_DIR}/ out of {}",
        worktree.display(),
    );
    assert!(
        worktree.join(KEPT_FILE).is_file(),
        "the sparse worktree at {} must hold {KEPT_FILE}",
        worktree.display(),
    );
}

/// A sparse `-b` run leaves the repository the environment names untouched,
/// through each git child that the sparse path adds.
///
/// Those children are `git ls-tree` for the check, `git sparse-checkout set`,
/// `git read-tree -mu HEAD`, `git rev-parse HEAD`, and `git hook run`. The
/// source repository has a `post-checkout` hook, so the run starts
/// `git hook run` and the hook runs. The hook writes its arguments to a log
/// outside both repositories. A `git hook run` that reads the configuration of
/// the decoy finds no hook, and the log stays empty. So the log proves that the
/// hook step ran in the source.
#[cfg(unix)]
#[test]
fn a_sparse_run_leaves_the_repository_the_environment_names_untouched() {
    let (_source_temp, source) = repo_with_files(&[HEAVY_FILE, KEPT_FILE]);
    let (_decoy_temp, decoy) = init_repo();

    let hooks = tempfile::TempDir::new().expect("create the hooks directory");
    let logs = tempfile::TempDir::new().expect("create the hook log directory");
    let log = logs.path().join("post-checkout.log");
    let log_text = log.to_str().expect("utf-8 log path");
    assert!(
        !log_text.contains('\''),
        "the log path goes into single quotes, so it cannot hold one: {log_text}"
    );
    support::install_post_checkout_hook(
        &source,
        hooks.path(),
        &format!("printf '%s\\n' \"$*\" >> '{log_text}'\n"),
    );

    let watch = DecoyWatch::before_the_run(&source, &decoy);
    let branch = unique_branch("hostile-sparse");

    let output = hostile_nwt_command(&source, &decoy)
        .args(["-b", &branch, "--sparse-exclude", HEAVY_DIR])
        .args(["--no-copy-env", "--no-bootstrap-hooks"])
        .output()
        .expect("run the nwt binary");

    let worktree = watch.assert_untouched(&output, Some(&branch));
    assert_heavy_dir_excluded(&worktree);

    let head = head_commit(&source);
    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        recorded.lines().collect::<Vec<&str>>(),
        vec![format!("{} {head} 1", "0".repeat(head.len()))],
        "the post-checkout hook of the source must run one time, with the arguments of a \
         plain add"
    );
}

/// A sparse `-c <branch>` run for a branch that only a remote holds leaves the
/// repository the environment names untouched.
///
/// The run asks git which remote branch the checkout DWIM takes, checks the
/// directory at that branch, and lets `git worktree add` make the local branch.
/// Each of those git children must read the clone, not the decoy. A run
/// without `-b` gives the worktree a random name, so the test does not state
/// the name of the worktree directory.
#[test]
fn a_sparse_checkout_of_a_remote_branch_leaves_the_repository_the_environment_names_untouched() {
    let (_upstream_temp, upstream) = repo_with_files(&[HEAVY_FILE, KEPT_FILE]);
    assert!(
        run_git(&upstream, &["branch", REMOTE_ONLY_BRANCH]),
        "git branch failed"
    );
    let (_clone_temp, clone) = clone_of(&upstream);
    let local = format!("refs/heads/{REMOTE_ONLY_BRANCH}");
    assert!(
        !run_git(&clone, &["show-ref", "--verify", "--quiet", &local]),
        "the fixture clone must not hold {local}"
    );
    let (_decoy_temp, decoy) = init_repo();

    let watch = DecoyWatch::before_the_run(&clone, &decoy);

    let output = hostile_nwt_command(&clone, &decoy)
        .args(["-c", REMOTE_ONLY_BRANCH, "--sparse-exclude", HEAVY_DIR])
        .args(["--no-copy-env", "--no-bootstrap-hooks"])
        .output()
        .expect("run the nwt binary");

    let worktree = watch.assert_untouched(&output, None);
    assert_heavy_dir_excluded(&worktree);
    assert_eq!(
        git_stdout(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]).trim_end(),
        REMOTE_ONLY_BRANCH,
        "the worktree must check out the local {REMOTE_ONLY_BRANCH} that git's DWIM makes"
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
