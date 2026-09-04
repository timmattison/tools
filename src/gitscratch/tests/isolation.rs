//! Isolation from a git environment this process did not ask for.
//!
//! Git exports `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and `GIT_PREFIX`
//! into every hook it runs, and a child of that hook inherits them. Anything
//! this crate spawns then targets the *hook's* repository rather than the
//! directory it was pointed at — so a fixture's `git init` re-initialises the
//! developer's real repository, its `git config` overwrites their identity, and
//! its `git add` stages a phantom entry in their index.
//!
//! The suite has one route to a leaked environment that does not involve
//! [`std::env::set_var`], which is process-global, `unsafe`, and would race
//! every other test in the binary: re-execute this binary with the variables
//! set on the *child* command. That is the leak shape verbatim — a whole
//! process whose environment says the repository is somewhere else — and it is
//! parallel-safe, because nothing outside the child ever sees the variables.
//!
//! Each test builds its own victim repository in its own `TempDir`, snapshots
//! the file the leak would corrupt, and asserts the bytes are identical
//! afterwards. A snapshot beats re-interrogating the victim with git: once a
//! leak has written a phantom index entry pointing at an object the victim does
//! not have, git's own answers stop being trustworthy.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use gitscratch::testing::{conflicting_repo, not_a_repository, DetachedGitDirRepo, TestRepo};
use gitscratch::Files;

/// The test re-executed as the child, by exact name.
///
/// A filter matching nothing exits zero, so a rename here that missed the
/// function below would leave every test in this file passing over a child that
/// ran nothing at all. [`run_in_child`] therefore checks the count too.
const CHILD_TEST: &str = "fixtures_and_replays_stay_inside_their_own_temporary_directories";

/// What libtest prints when exactly one test ran and passed.
const ONE_TEST_PASSED: &str = "1 passed";

/// The subject of the control commit the child makes through
/// [`TestRepo::try_git`], so the fixture's own log can be asked whether that
/// commit landed *here*.
const CONTROL_MESSAGE: &str = "try_git control";

/// An author name the control commit carries on its own command line.
///
/// `GIT_AUTHOR_NAME` wears the `GIT_` prefix the scrub sweeps on, so it is also
/// the assertion that a caller's variables are applied *after* the sweep rather
/// than before it — applied first, this one would be swept away and the commit
/// would be stamped with the fixture's configured name instead.
const CONTROL_AUTHOR: &str = "the control's own author";

/// The only file [`conflicting_repo`] commits, and therefore the whole of the
/// tree a commit made in that fixture must be built from.
const FIXTURE_FILE: &str = "shared.txt";

/// A file name that is not valid UTF-8, which is the one name a fixture can
/// only commit through [`TestRepo::commit_file_named_by_bytes`] — and so the
/// one route to the spawn that pipes its argument in on stdin.
const UNDECODABLE_NAME: &[u8] = b"\xff-byte-named.txt";

/// The branch the detached-git-directory fixture checks out into a linked
/// worktree here. Any name it does not already carry does.
const DETACHED_BRANCH: &str = "side";

/// Everything this crate spawns git for, exercised in whatever environment the
/// process happens to have.
///
/// Run directly by `cargo test` like any other test — where it simply proves
/// the fixtures build — and re-executed by the tests below with a leaked git
/// environment, where it is the assertion. Every spawn site is covered in one
/// pass because a leak reaches every one of them at once:
///
/// - [`TestRepo`]'s builder, which is the spawn every fixture is made of.
/// - [`TestRepo::commit_file_named_by_bytes`], which reaches the spawn that
///   pipes a record in on stdin — the one way to name a path git records as
///   bytes, and a spawn that writes straight into an object database.
/// - [`TestRepo::try_git`] — the spawn a control run needs, because it is
///   allowed to fail.
/// - The [`not_a_repository`] probe.
/// - [`DetachedGitDirRepo`], whose runner names the git directory and the work
///   tree on the command line. Those two arguments outrank `GIT_DIR` and
///   `GIT_WORK_TREE`, and nothing on that command line outranks
///   `GIT_INDEX_FILE`, so the sweep is the whole of what keeps the fixture's
///   own `git add` out of the developer's index.
/// - The crate's own `Git`, which `TestRepo::scratch` reaches twice over,
///   first through `Repo::open`'s pre-flight and then through the `Scratch` it
///   hands back.
///
/// The list carries no count. A number written here is a fact nothing checks,
/// and this one had gone stale: it said four while the crate had six, and both
/// unlisted spawns were the two most recently added. Deriving the number means
/// scanning `src/testing.rs` for the shape of a spawn, which is a matcher whose
/// own spelling list is the next thing to go quietly out of date — the defect
/// this list already had, moved into a guard. So the sites are named instead,
/// where a reader can hold the list against the file.
#[test]
fn fixtures_and_replays_stay_inside_their_own_temporary_directories() {
    let repo = conflicting_repo();

    assert!(
        repo.path().join(".git").is_dir(),
        "the fixture builder must have made a repository in {}",
        repo.path().display()
    );
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]),
        "main",
        "the fixture's own branch, not whichever one the environment names"
    );

    let scratch = repo.scratch("main");
    scratch
        .testing_git()
        .run("checkout", &["-q", "--detach", "right"])
        .expect("check out the branch detached in the scratch worktree");
    let conflicts = scratch
        .replay_rebase("left")
        .expect("replay the branch onto the simulated base");
    assert_eq!(
        conflicts.files(),
        Files::new(1),
        "the fixture's own contested file should have conflicted"
    );

    // The spawn that pipes its record in on stdin, which is how a fixture names
    // a path git records as bytes. It writes a blob, a tree and a commit
    // straight into an object database, so a leaked `GIT_DIR` puts all three in
    // the developer's own store — and the commit is then read back through the
    // scrubbed reader, which finds nothing.
    let byte_named = repo.commit_file_named_by_bytes(
        UNDECODABLE_NAME,
        "content under a name no filesystem here can hold\n",
        "a name git records as bytes",
    );
    assert_eq!(
        repo.git(&["cat-file", "-t", &byte_named]),
        "commit",
        "the blob, the tree and the commit went to whichever repository the \
         environment named rather than to the fixture they were aimed at"
    );

    let elsewhere = not_a_repository();
    assert!(
        elsewhere.path().is_dir(),
        "the probe must hand back a directory that exists"
    );

    // The detached-git-directory fixture, whose every spawn names the git
    // directory and the work tree on the command line. Those arguments outrank
    // `GIT_DIR` and `GIT_WORK_TREE`; nothing on the command line outranks
    // `GIT_INDEX_FILE`, so without the sweep this fixture's own `git add`
    // stages into whichever index the environment names.
    let detached = DetachedGitDirRepo::nested();
    let linked = detached
        .work_tree()
        .parent()
        .expect("the work tree sits under the fixture's temporary directory")
        .join("linked-worktree");
    detached.add_worktree(&linked, DETACHED_BRANCH);

    assert!(
        linked.is_dir(),
        "the linked worktree must exist at {}",
        linked.display()
    );
    assert_eq!(
        detached.git(&["rev-parse", "--abbrev-ref", "HEAD"]),
        "main",
        "the detached fixture's own branch, not whichever one the environment names"
    );

    // The spawn a control run needs, which is a git command that is allowed to
    // fail — failing is what proves the hazard is armed — and which
    // `TestRepo::git` cannot give it, because it panics on a non-zero exit. A
    // control that reached around the fixture for a raw `Command` would get
    // that permission and lose this scrub with it, and `.current_dir()` does
    // not settle which repository git uses: `GIT_DIR` outranks it. So the
    // control commit below is made the way `try_git` makes one, and the two
    // questions asked of it afterwards are *where* it landed and *what tree* it
    // was built from — the damage a leaked location and a leaked index each do.
    let control = repo.try_git(
        &["commit", "--allow-empty", "-q", "-m", CONTROL_MESSAGE],
        &[
            ("GIT_AUTHOR_NAME", CONTROL_AUTHOR),
            // Set for the reason the real controls set it: a control that is
            // expected to fail has to fail rather than stop on a prompt.
            ("GIT_TERMINAL_PROMPT", "0"),
        ],
    );
    assert!(
        control.status.success(),
        "the control commit should have landed in the fixture:\n{}\n{}",
        String::from_utf8_lossy(&control.stdout),
        String::from_utf8_lossy(&control.stderr),
    );
    // Read back through `repo.git`, which is scrubbed, so the answers are the
    // fixture's own however the environment is pointed.
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        CONTROL_MESSAGE,
        "the control commit went to whichever repository the environment named \
         rather than to the fixture it was aimed at"
    );
    assert_eq!(
        repo.git(&["ls-tree", "--name-only", "-r", "HEAD"]),
        FIXTURE_FILE,
        "the control commit was built from an index the environment named \
         rather than from the fixture's own"
    );
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an"]),
        CONTROL_AUTHOR,
        "a variable the caller set on the command must survive the scrub, which \
         sweeps on the `GIT_` prefix and would take this one with it if it ran \
         second"
    );
}

/// Re-execute this binary running only [`CHILD_TEST`], with `leaked` added to
/// its environment.
///
/// # Panics
///
/// Panics if the child could not be spawned, if it failed, or if it did not run
/// exactly one test — the last of which is what stops a stale [`CHILD_TEST`]
/// from turning every caller green over a child that ran nothing.
fn run_in_child(leaked: &[(&str, PathBuf)]) -> Output {
    let mut command = Command::new(std::env::current_exe().expect("the running test binary"));
    command.args([CHILD_TEST, "--exact"]);
    for (name, value) in leaked {
        command.env(name, value);
    }

    let output = command.output().expect("re-run this test binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "the fixtures did not survive {}:\n{stdout}\n{stderr}",
        leaked
            .iter()
            .map(|(name, value)| format!("{name}={}", value.display()))
            .collect::<Vec<_>>()
            .join(" "),
    );
    assert!(
        stdout.contains(ONE_TEST_PASSED),
        "the child must have run exactly one test, got:\n{stdout}"
    );

    output
}

/// A repository standing in for the developer's own, with an identity of its
/// own so a fixture writing *its* identity over the top is visible.
fn victim() -> TestRepo {
    let repo = TestRepo::init();
    repo.git(&["config", "user.email", "victim@example.com"]);
    repo.git(&["config", "user.name", "the developer"]);
    repo.commit_file("victim.txt", "work that must survive\n", "real work");
    repo
}

/// Read a file that must come back byte-identical later.
///
/// # Panics
///
/// Panics if the file cannot be read.
fn snapshot(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The severe shape: with the repository's location leaked, a fixture's
/// `git init` re-initialises the developer's real repository, the fixture
/// directory never gets a `.git` at all, and the `git config` calls that follow
/// write `user.email`, `user.name` and `commit.gpgsign` into the real repo's
/// config — permanently changing that repository's identity before anything
/// fails.
///
/// `.git/config` is therefore the file to watch: it is where the damage lands,
/// and it lands there *first*, before the `git add` that finally errors out.
#[test]
fn a_leaked_repository_location_never_reaches_the_repository_it_names() {
    let victim = victim();
    let git_dir = victim.path().join(".git");
    let config = snapshot(&git_dir.join("config"));
    let head = victim.rev_parse("HEAD");

    run_in_child(&[
        ("GIT_DIR", git_dir.clone()),
        ("GIT_WORK_TREE", victim.path().to_path_buf()),
        // Exported by every hook alongside the other two. Harmless on its own
        // here, but the shape a hook actually produces is all four at once.
        ("GIT_PREFIX", PathBuf::new()),
    ]);

    assert_eq!(
        snapshot(&git_dir.join("config")),
        config,
        "a fixture wrote into the config of the repository the environment named"
    );
    assert_eq!(
        victim.rev_parse("HEAD"),
        head,
        "a fixture moved HEAD in the repository the environment named"
    );
    assert!(
        !git_dir.join("worktrees").exists(),
        "a replay added a worktree to the repository the environment named"
    );
}

/// The common shape: `.husky/pre-commit` is a hook, so `GIT_INDEX_FILE` alone
/// is what a `cargo test` run from it inherits. `git init` then succeeds in the
/// fixture, but every `git add` writes into the *real* repository's index,
/// leaving a phantom staged entry for a file that does not exist there. That is
/// verbatim the incident the hook's own comment documents.
#[test]
fn a_leaked_index_file_never_has_a_fixture_staged_into_it() {
    let victim = victim();
    let index = victim.path().join(".git").join("index");
    let before = snapshot(&index);

    run_in_child(&[("GIT_INDEX_FILE", index.clone())]);

    assert_eq!(
        snapshot(&index),
        before,
        "a fixture staged itself into the index the environment named"
    );
}
