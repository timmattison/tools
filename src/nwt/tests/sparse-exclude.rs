//! End-to-end coverage for `nwt --sparse-exclude <DIR>` (issue #487).
//!
//! The flag makes the new worktree a sparse checkout that does not write
//! `<DIR>`. Only that one worktree changes. The main worktree and each later
//! worktree without the flag stay full.
//!
//! Every test runs the real binary through `support::nwt_command`, in a
//! throwaway repository that holds a heavy directory and three near misses:
//! `heavy.txt` (a file whose name starts with the directory name),
//! `src/heavy/` (a directory of the same name below the root), and
//! `heavy/sub/` (a directory below the heavy directory). The first two prove
//! that the pattern is anchored at the root and matches a directory only.

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

use support::{git_stdout, init_repo, nanos, nwt_command, run_git};

/// The directory the tests exclude.
const HEAVY_DIR: &str = "heavy";

/// Tracked files that stay in a worktree that excludes [`HEAVY_DIR`].
const KEPT_FILES: &[&str] = &["README.md", "heavy.txt", "src/heavy/lib.txt"];

/// Tracked files under [`HEAVY_DIR`], which an excluding worktree does not
/// write.
const HEAVY_FILES: &[&str] = &["heavy/big.txt", "heavy/sub/deep.txt"];

/// A branch name that no concurrent copy of this suite can also hold.
///
/// Two `cargo test` runs share one machine, and a branch name is a shared
/// resource. The process id and a nanosecond clock reading keep them apart.
fn unique_branch(label: &str) -> String {
    format!("{label}-{}-{}", std::process::id(), nanos())
}

/// Write `contents` to `relative` under `repo`, and make the parent
/// directories first.
fn write_file(repo: &Path, relative: &str, contents: &str) {
    let path = repo.join(relative);
    let parent = path.parent().expect("a file path has a parent");
    std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
    std::fs::write(&path, contents).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// Make a repository whose second commit adds `files`, and hand back the
/// temporary directory that holds it (keep it alive) and the repository.
fn repo_with_files(files: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let (temp, repo) = init_repo();

    for file in files {
        write_file(&repo, file, &format!("{file}\n"));
    }
    assert!(run_git(&repo, &["add", "--", "."]), "git add failed");
    assert!(
        run_git(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "add the tree"]
        ),
        "git commit failed"
    );

    (temp, repo)
}

/// Make a repository that holds [`KEPT_FILES`] and [`HEAVY_FILES`].
fn repo_with_heavy_dir() -> (tempfile::TempDir, PathBuf) {
    let files: Vec<&str> = KEPT_FILES
        .iter()
        .chain(HEAVY_FILES)
        .copied()
        .filter(|file| *file != "README.md")
        .collect();
    repo_with_files(&files)
}

/// Run `nwt -b <branch>` in `repo` with `extra` arguments, without the `.env`
/// copy and the hook bootstrap, and hand back what it wrote.
fn run_nwt(repo: &Path, branch: &str, extra: &[&str]) -> Output {
    nwt_command(repo)
        .args(["-b", branch, "--no-copy-env", "--no-bootstrap-hooks"])
        .args(extra)
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

/// Demand that each of `files` is a file in `worktree`.
fn assert_files_present(worktree: &Path, files: &[&str]) {
    for file in files {
        assert!(
            worktree.join(file).is_file(),
            "{file} must be in the worktree at {}",
            worktree.display()
        );
    }
}

/// Test 1 of issue #487: the excluded directory is absent, every other tracked
/// file is present, and git sees no change.
///
/// An empty `git status --short` proves that the files of the excluded
/// directory are not staged as deleted. A sequence that checks out without the
/// sparse patterns in place leaves exactly that.
#[test]
fn an_excluded_directory_is_absent_and_the_rest_is_present() {
    let (_temp, repo) = repo_with_heavy_dir();
    let branch = unique_branch("sparse");

    let output = run_nwt(&repo, &branch, &["--sparse-exclude", HEAVY_DIR]);
    let worktree = created_worktree(&output);

    assert!(
        !worktree.join(HEAVY_DIR).exists(),
        "--sparse-exclude {HEAVY_DIR} must keep {HEAVY_DIR}/ out of {}",
        worktree.display()
    );
    assert_files_present(&worktree, KEPT_FILES);

    let status = git_stdout(&worktree, &["status", "--short"]);
    assert!(
        status.is_empty(),
        "a sparse worktree has no change to report, but git status says:\n{status}"
    );
}
