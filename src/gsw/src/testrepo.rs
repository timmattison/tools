//! Test-only git fixtures: throwaway repositories to run gsw's git code against.
//!
//! Every unit test that needs a real repository builds one here rather than
//! reaching for the checkout the suite happens to be running in. The helpers
//! are shared across modules ([`crate::repo`] exercises the git reads,
//! [`crate::watch`] exercises the refresh loop) so the *isolation* rules below
//! are stated and enforced in exactly one place — a copy that drifted would
//! reintroduce the fixture-writes-to-the-real-repo failure mode silently.
//!
//! Every fixture lives in its own [`tempfile::TempDir`], so the suite stays
//! parallel-safe: two concurrent runs of the same test never share a path.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// Run a git command in `dir`, isolated from the host's global/system config,
/// asserting success.
///
/// Scrubs inherited `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE` so the fixture
/// repo under `dir` is the one git operates on. Without this, when the suite
/// runs from inside this repo's own pre-commit hook (git exports those vars for
/// the hook), the fixture's commits would land in the *real* repo despite
/// `current_dir(dir)`.
///
/// # Panics
///
/// Panics if git cannot be invoked, or if it exits non-zero — use
/// [`git_allowing_failure`] for commands whose failure is part of the fixture.
pub(crate) fn git(dir: &Path, args: &[&str]) {
    let status = command(dir, args).status().expect("invoke git");
    assert!(status.success(), "git {args:?} failed");
}

/// Run a git command in `dir` with the same isolation as [`git`], but tolerate a
/// non-zero exit.
///
/// Some fixtures are *built* out of git failures: `git merge` exits non-zero
/// when it leaves conflicts behind, which is precisely the state a test of
/// conflict rendering needs. Asserting success there would fail the test on the
/// very thing it is arranging.
///
/// # Panics
///
/// Panics only if git cannot be invoked at all.
pub(crate) fn git_allowing_failure(dir: &Path, args: &[&str]) {
    let _ = command(dir, args).status().expect("invoke git");
}

/// A git invocation in `dir` with the host's config and any hook-exported git
/// location scrubbed. The single place the isolation rules are applied; both
/// [`git`] and [`git_allowing_failure`] differ only in how they treat the exit
/// status.
fn command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    cmd
}

/// A fresh repo on branch `main` with one commit (`a.txt` = `"initial\n"`).
/// Parallel-safe: unique tempdir.
///
/// The returned [`TempDir`] owns the repo — dropping it deletes the fixture, so
/// callers must hold it for as long as they read the repository.
pub(crate) fn init_repo() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    identity(p);
    std::fs::write(p.join("a.txt"), "initial\n").expect("write a.txt");
    git(p, &["add", "a.txt"]);
    git(p, &["commit", "-q", "-m", "initial"]);
    dir
}

/// Clone [`init_repo`]'s repo so the clone has a real `origin/main` upstream,
/// returning `(origin, clone)`.
///
/// Both [`TempDir`]s must be held: dropping the origin deletes the remote the
/// clone's `origin` points at, which breaks any later fetch or push.
pub(crate) fn init_repo_with_upstream() -> (TempDir, TempDir) {
    let origin = init_repo();
    let clone = tempfile::tempdir().expect("tempdir");
    // Both paths are absolute, so the cwd only has to exist; `clone` is an
    // empty directory, which `git clone` accepts as the destination.
    git(
        clone.path(),
        &[
            "clone",
            "-q",
            origin.path().to_str().expect("utf-8 tempdir path"),
            clone.path().to_str().expect("utf-8 tempdir path"),
        ],
    );
    identity(clone.path());
    (origin, clone)
}

/// An [`init_repo`] repo plus a linked worktree checked out on its own branch,
/// returning `(repo, linked_worktree_path)`.
///
/// A linked worktree is the only layout where the work-tree root holds a `.git`
/// *file* — a `gitdir:` pointer at `<repo>/.git/worktrees/<name>` — instead of a
/// `.git` directory, and following that pointer to the shared config is a
/// distinct code path from reading `<root>/.git/config` directly. It is also the
/// layout this repository mandates all development happen in, so fixtures built
/// only from [`init_repo`] and [`init_repo_with_upstream`] leave the path gsw
/// runs against most as the one path nothing covers.
///
/// The worktree is deliberately nested *inside* the repo's tempdir so a single
/// [`TempDir`] owns both halves: dropping it removes the checkout and the
/// `.git/worktrees` administrative directory that describes it together, with no
/// second directory to leak. That [`TempDir`] must therefore be held for as long
/// as the caller reads *either* repository — the returned path points inside it,
/// so dropping the [`TempDir`] invalidates the path as well.
pub(crate) fn init_repo_with_worktree() -> (TempDir, PathBuf) {
    let repo = init_repo();
    let linked = repo.path().join("linked");
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "linked",
            linked.to_str().expect("utf-8 tempdir path"),
        ],
    );
    (repo, linked)
}

/// The address every fixture commit carries.
///
/// Spelled once, because [`identity`] writes it into the repository and the
/// tests read it back off a commit. Two spellings that drifted apart would
/// leave a test that asserts an address nothing sets.
pub(crate) const FIXTURE_EMAIL: &str = "t@example.com";

/// Give the repo at `dir` a committer identity and disable signing, so commits
/// succeed no matter how the host's (scrubbed) global config is set up.
fn identity(dir: &Path) {
    git(dir, &["config", "user.email", FIXTURE_EMAIL]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use tempfile::NamedTempFile;

    use super::{command, init_repo, FIXTURE_EMAIL};

    /// The variable that tells this test binary it is the child, and that the
    /// child must do the work rather than start a child of its own.
    ///
    /// The name carries no `GIT_` prefix, so the sweep under test leaves it in
    /// place and the child reads it.
    const HOSTILE_MARKER: &str = "GSW_TESTREPO_HOSTILE_GIT_ENVIRONMENT";

    /// The line the child prints after its last assertion holds.
    ///
    /// libtest exits 0 when a filter names no test, so a child that ran no
    /// test reads exactly like a child that passed. The parent looks for this
    /// line as well as for the exit status.
    const CHILD_RAN: &str = "gsw-testrepo-hostile-environment-child-ran";

    /// How long the parent waits for the child.
    ///
    /// The child builds one small repository, so a healthy child takes well
    /// under a second. The bound is here for the child that hangs: a test that
    /// waits for such a child holds the run for the life of the session.
    const CHILD_DEADLINE: Duration = Duration::from_secs(60);

    /// How often the parent asks whether the child is done.
    const CHILD_POLL: Duration = Duration::from_millis(25);

    /// The variable that moves the objects git writes into another store.
    ///
    /// It is not a location variable in the sense the old list understood, so
    /// no amount of names beside `GIT_DIR` catches it. Issue #415 aimed it at
    /// a decoy and watched `git hash-object -w` write the object there.
    const OBJECT_DIRECTORY: &str = "GIT_OBJECT_DIRECTORY";

    /// The variable that injects configuration into every git command.
    ///
    /// Git exports it to every `pre-commit` hook, and it outranks each
    /// configuration file. The two `/dev/null` pins therefore do nothing about
    /// it.
    const CONFIG_PARAMETERS: &str = "GIT_CONFIG_PARAMETERS";

    /// The address the hostile configuration puts on every commit it reaches.
    const INJECTED_EMAIL: &str = "injected@example.invalid";

    /// The hostile value of [`CONFIG_PARAMETERS`], in the form git reads: one
    /// pair, inside single quotes.
    fn injected_config() -> String {
        format!("'user.email={INJECTED_EMAIL}'")
    }

    /// The name of a test, the way libtest spells it.
    ///
    /// `module_path!` starts with the name of the crate and a test name does
    /// not, so the first part goes. The rest of the path comes from the
    /// compiler, so a module that moves needs no edit here.
    fn test_name(function: &str) -> String {
        let module = module_path!();
        let module = module.split_once("::").map_or(module, |(_, rest)| rest);
        format!("{module}::{function}")
    }

    /// How many loose objects sit in the object store at `store`.
    ///
    /// A loose object lives in a directory named for the first two hex digits
    /// of its id, so the count is the files under every such directory. A
    /// store that does not exist holds none.
    ///
    /// The count is the measure rather than the presence of the directory,
    /// because `git init` creates `info` and `pack` in whatever store it is
    /// pointed at. Those two say where git wrote its administrative files. The
    /// loose objects say where git wrote the content of the fixture.
    fn loose_objects(store: &Path) -> usize {
        let Ok(entries) = std::fs::read_dir(store) else {
            return 0;
        };

        entries
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.chars().count() == 2 && name.chars().all(|digit| digit.is_ascii_hexdigit())
            })
            .map(|entry| {
                std::fs::read_dir(entry.path())
                    .map_or(0, |objects| objects.filter_map(Result::ok).count())
            })
            .sum()
    }

    /// Run git in `dir` through [`command`] and hand back what it said.
    ///
    /// The read goes through the helper under test on purpose. A read through
    /// a git invocation of its own would answer about a repository this test
    /// never asked about.
    fn read(dir: &Path, args: &[&str]) -> String {
        let output = command(dir, args).output().expect("invoke git");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Start this test binary again, with `test` named and the hostile
    /// environment on it, and fail where that child fails.
    ///
    /// The hostile values go on the child and never on this process. A `GIT_`
    /// variable is process-global state, and this binary runs many real git
    /// commands at once, so a variable set here changes what an unrelated test
    /// reads. A child holds an environment of its own, so the hostile values
    /// reach the code under test and reach nothing else. That is also what
    /// makes the answer the same under a shell and under the pre-commit hook
    /// of this repository, which exports `GIT_` variables into `cargo test`.
    ///
    /// The decoy object store is a temporary directory of this process, and
    /// the child learns where it is from the variable itself. This process
    /// holds it until the child is done, so the child has a real directory to
    /// write into. A decoy that git cannot write to fails the fixture for the
    /// wrong reason.
    ///
    /// The child writes to two files rather than to two pipes, so the parent
    /// never waits for an end of file a grandchild holds open.
    ///
    /// The wait is bounded. A child that hangs is killed and reaped, and the
    /// test then fails, because a test that waits for such a child holds the
    /// run for the life of the session.
    fn a_child_of_this_test_passes(test: &str) {
        let workdir = tempfile::tempdir().expect("tempdir");
        let decoy = tempfile::tempdir().expect("tempdir");
        let stdout = NamedTempFile::new().expect("a file for what the child says");
        let stderr = NamedTempFile::new().expect("a file for why the child stopped");
        let mut child_command =
            Command::new(std::env::current_exe().expect("the path of this binary"));
        child_command
            .args(["--exact", "--nocapture", test])
            .current_dir(workdir.path())
            .env(HOSTILE_MARKER, "1")
            .env(OBJECT_DIRECTORY, decoy.path())
            .env(CONFIG_PARAMETERS, injected_config())
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                stdout.as_file().try_clone().expect("clone the file"),
            ))
            .stderr(Stdio::from(
                stderr.as_file().try_clone().expect("clone the file"),
            ));
        let mut child = child_command.spawn().expect("start this test binary again");

        let give_up_at = Instant::now() + CHILD_DEADLINE;
        let status = loop {
            match child.try_wait().expect("ask about the child") {
                Some(status) => break status,
                None => {
                    if Instant::now() >= give_up_at {
                        // The kill comes before the panic. A panic ends the
                        // test where it stands, and the process this test
                        // started is the one thing that must not outlive it.
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!(
                            "the child did not finish within {}s",
                            CHILD_DEADLINE.as_secs(),
                        );
                    }
                    std::thread::sleep(CHILD_POLL);
                }
            }
        };

        let said = format!(
            "{}{}",
            std::fs::read_to_string(stdout.path()).unwrap_or_default(),
            std::fs::read_to_string(stderr.path()).unwrap_or_default(),
        );
        assert!(status.success(), "the child failed ({status}):\n{said}");
        assert!(
            said.contains(CHILD_RAN),
            "the child ran no test, so it passed for the wrong reason. libtest exits 0 when a \
             filter names no test, and the filter was {test:?}:\n{said}",
        );
    }

    /// A fixture obeys no git variable out of a hostile environment.
    ///
    /// **The rule is the `GIT_` prefix, and never a list of names.** A list
    /// strips nothing new the day git adds a variable, and from then on it
    /// gives the same clean-looking answer as a list that works. The two
    /// variables below walked straight through the three-name list this helper
    /// carried, and neither one is a location variable, so no number of
    /// location names catches them.
    ///
    /// **The two assertions are the two failures, not two guesses.**
    /// [`OBJECT_DIRECTORY`] moves the content of the fixture into another
    /// store, so the fixture ends with an empty store of its own and a decoy
    /// full of its objects. [`CONFIG_PARAMETERS`] puts another address on the
    /// commit, over the address the fixture writes into the repository,
    /// because git reads the environment ahead of every configuration file.
    ///
    /// **The armed control comes first.** The child asserts that it really
    /// holds both variables. An assertion that a variable did nothing passes
    /// just as readily where there was nothing to do.
    ///
    /// **A test that reads the removals off the [`Command`] proves less.** A
    /// sweep of the `GIT_` prefix records a removal only for a variable this
    /// process holds, so such a test is empty under a shell and full under the
    /// pre-commit hook of this repository. This test measures the repository
    /// the fixture built instead, which is the same answer in both.
    #[test]
    fn a_fixture_obeys_no_git_variable_out_of_a_hostile_environment() {
        if std::env::var_os(HOSTILE_MARKER).is_none() {
            a_child_of_this_test_passes(&test_name(
                "a_fixture_obeys_no_git_variable_out_of_a_hostile_environment",
            ));
            return;
        }

        let decoy = PathBuf::from(std::env::var_os(OBJECT_DIRECTORY).unwrap_or_else(|| {
            panic!(
                "the child must really hold {OBJECT_DIRECTORY}, or there is nothing here to \
                 remove and the assertions below are measured against nothing",
            )
        }));
        assert_eq!(
            std::env::var(CONFIG_PARAMETERS).ok().as_deref(),
            Some(injected_config().as_str()),
            "the child must really hold {CONFIG_PARAMETERS}, or there is nothing here to remove \
             and the assertion below is measured against nothing",
        );

        let fixture = init_repo();
        let store = fixture.path().join(".git").join("objects");

        assert!(
            loose_objects(&store) > 0,
            "the fixture wrote no object of its own. {OBJECT_DIRECTORY} reached git, so the \
             content of the fixture went to the store that variable names, and every later read \
             of the fixture answers about another repository",
        );
        assert_eq!(
            loose_objects(&decoy),
            0,
            "the fixture wrote its objects into the decoy store at {}. {OBJECT_DIRECTORY} \
             reached git, and a run under the pre-commit hook of this repository aims that \
             variable at the real repository",
            decoy.display(),
        );

        assert_eq!(
            read(fixture.path(), &["log", "-1", "--format=%ae"]),
            FIXTURE_EMAIL,
            "the fixture commit carries the injected address. {CONFIG_PARAMETERS} reached git, \
             and git reads it ahead of every configuration file, so the two `/dev/null` pins do \
             nothing about it",
        );

        println!("{CHILD_RAN}");
    }
}
