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

/// Test 4 of issue #487: two flags exclude two directories, and the one stderr
/// line names both.
///
/// Each excluded directory has a sibling that stays, so a pattern that takes
/// out the parent directory fails too.
#[test]
fn two_flags_exclude_two_directories_and_the_notice_names_both() {
    const EXCLUDED: &[&str] = &["assets/video", "fixtures/large"];
    const EXCLUDED_FILES: &[&str] = &["assets/video/clip.txt", "fixtures/large/blob.txt"];
    const KEPT: &[&str] = &["README.md", "assets/other.txt", "fixtures/small/tiny.txt"];
    const NOTICE: &str = "Excluded assets/video/, fixtures/large/ (sparse checkout). Run \
                          'git sparse-checkout disable' in the worktree to get them.";

    let files: Vec<&str> = EXCLUDED_FILES
        .iter()
        .chain(KEPT)
        .copied()
        .filter(|file| *file != "README.md")
        .collect();
    let (_temp, repo) = repo_with_files(&files);
    let branch = unique_branch("sparse-two");

    let output = run_nwt(
        &repo,
        &branch,
        &[
            "--sparse-exclude",
            EXCLUDED[0],
            "--sparse-exclude",
            EXCLUDED[1],
        ],
    );
    let worktree = created_worktree(&output);

    for dir in EXCLUDED {
        assert!(
            !worktree.join(dir).exists(),
            "--sparse-exclude {dir} must keep {dir}/ out of {}",
            worktree.display()
        );
    }
    assert_files_present(&worktree, KEPT);

    let status = git_stdout(&worktree, &["status", "--short"]);
    assert!(
        status.is_empty(),
        "a sparse worktree has no change to report, but git status says:\n{status}"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.lines().any(|line| line == NOTICE),
        "stderr must hold the one notice line {NOTICE:?}, but it holds:\n{stderr}"
    );
}

/// The exit code `nwt` returns when it refuses a `--sparse-exclude` value.
const INVALID_SPARSE_EXCLUDE: i32 = 15;

/// The suffix `nwt` adds to the repository name to name the directory that
/// holds every new worktree.
const WORKTREES_SUFFIX: &str = "-worktrees";

/// Resolve a path before a comparison reads it.
///
/// Git prints resolved paths, and macOS reaches every temporary directory
/// through a symbolic link: `/var` resolves to `/private/var`.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
}

/// The directory `nwt -b <branch>` makes in `repo`, resolved.
///
/// A branch name that holds only letters, digits, and `-` is also the name of
/// the directory.
fn expected_worktree(repo: &Path, branch: &str) -> PathBuf {
    let repo = canonical(repo);
    let name = repo
        .file_name()
        .expect("the repository has a name")
        .to_str()
        .expect("utf-8 repository name");
    repo.with_file_name(format!("{name}{WORKTREES_SUFFIX}"))
        .join(branch)
}

/// Test 2 of issue #487, the question that started it: a later `nwt` without
/// the flag gives a full worktree, and the sparse worktree and the main
/// worktree keep what they had.
#[test]
fn a_later_worktree_without_the_flag_is_full() {
    let (_temp, repo) = repo_with_heavy_dir();

    let sparse = created_worktree(&run_nwt(
        &repo,
        &unique_branch("sparse-first"),
        &["--sparse-exclude", HEAVY_DIR],
    ));
    let full = created_worktree(&run_nwt(&repo, &unique_branch("full-second"), &[]));

    assert_files_present(&full, KEPT_FILES);
    assert_files_present(&full, HEAVY_FILES);
    let status = git_stdout(&full, &["status", "--short"]);
    assert!(
        status.is_empty(),
        "a full worktree has no change to report, but git status says:\n{status}"
    );

    assert!(
        !sparse.join(HEAVY_DIR).exists(),
        "the full worktree must not give {HEAVY_DIR}/ back to the sparse worktree at {}",
        sparse.display()
    );
    assert_files_present(&repo, HEAVY_FILES);
}

/// Test 3 of issue #487: the sparse settings belong to the new worktree only.
///
/// `git sparse-checkout set` in the new worktree writes `core.sparseCheckout`
/// into `.git/worktrees/<name>/config.worktree` and the patterns into
/// `.git/worktrees/<name>/info/sparse-checkout`. The main worktree reads
/// `core.sparseCheckout` as unset and has no pattern file. The test asks git
/// for `<name>`, and does not guess it from the branch.
#[test]
fn the_sparse_settings_are_in_the_new_worktree_only() {
    let (_temp, repo) = repo_with_heavy_dir();
    let worktree = created_worktree(&run_nwt(
        &repo,
        &unique_branch("sparse-scope"),
        &["--sparse-exclude", HEAVY_DIR],
    ));

    let worktree_git_dir =
        PathBuf::from(git_stdout(&worktree, &["rev-parse", "--absolute-git-dir"]).trim_end());
    assert_eq!(
        canonical(&worktree_git_dir).parent(),
        Some(canonical(&repo.join(".git").join("worktrees")).as_path()),
        "the git directory of the new worktree must be under .git/worktrees"
    );

    let config_worktree = worktree_git_dir.join("config.worktree");
    let config_worktree = config_worktree.to_str().expect("utf-8 config path");
    assert_eq!(
        git_stdout(
            &repo,
            &[
                "config",
                "--file",
                config_worktree,
                "--get",
                "core.sparseCheckout"
            ]
        )
        .trim_end(),
        "true",
        "config.worktree of the new worktree must turn sparse checkout on"
    );

    let patterns = std::fs::read_to_string(worktree_git_dir.join("info").join("sparse-checkout"))
        .expect("read the sparse-checkout file of the new worktree");
    let expected_pattern = format!("!/{HEAVY_DIR}/");
    assert!(
        patterns.lines().any(|line| line == expected_pattern),
        "the pattern file of the new worktree must hold {expected_pattern:?}:\n{patterns}"
    );

    // `run_git` answers with a bool. `git config --get` exits 1 for a key that
    // is not set, and `git_stdout` panics on that.
    assert!(
        !run_git(&repo, &["config", "--get", "core.sparseCheckout"]),
        "the main worktree must read core.sparseCheckout as unset"
    );
    assert!(
        !repo
            .join(".git")
            .join("info")
            .join("sparse-checkout")
            .exists(),
        "the main worktree must have no sparse-checkout pattern file"
    );
    assert_files_present(&repo, HEAVY_FILES);
}

/// Test 10 of issue #487: stdout holds the worktree path and one line break,
/// and nothing else.
///
/// The shell wrapper runs `dir=$(command nwt "$@")` and changes to `$dir`. A
/// word from a git child or from the notice on stdout breaks that.
#[test]
fn stdout_is_only_the_worktree_path() {
    let (_temp, repo) = repo_with_heavy_dir();
    let branch = unique_branch("sparse-stdout");

    let output = run_nwt(&repo, &branch, &["--sparse-exclude", HEAVY_DIR]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "nwt failed ({:?}):\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        stdout,
        format!("{}\n", expected_worktree(&repo, &branch).display()),
        "stdout must be the worktree path and one line break"
    );
}

/// Test 11 of issue #487: `git sparse-checkout disable` in the new worktree
/// writes the excluded directory. This is the way back that the notice names.
#[test]
fn sparse_checkout_disable_writes_the_excluded_directory() {
    let (_temp, repo) = repo_with_heavy_dir();
    let worktree = created_worktree(&run_nwt(
        &repo,
        &unique_branch("sparse-disable"),
        &["--sparse-exclude", HEAVY_DIR],
    ));
    assert!(
        !worktree.join(HEAVY_DIR).exists(),
        "the fixture must start without {HEAVY_DIR}/"
    );

    assert!(
        run_git(&worktree, &["sparse-checkout", "disable"]),
        "git sparse-checkout disable failed in {}",
        worktree.display()
    );

    assert_files_present(&worktree, HEAVY_FILES);
    assert_files_present(&worktree, KEPT_FILES);
    let status = git_stdout(&worktree, &["status", "--short"]);
    assert!(
        status.is_empty(),
        "a worktree made full again has no change to report, but git status says:\n{status}"
    );
}

/// Demand that `output` is a run that `nwt` refused with exit `code`: it
/// printed no path, and stderr holds `named`.
fn assert_refused(output: &Output, code: i32, named: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(code),
        "the run must exit {code}.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.is_empty(),
        "a refused run prints no path. stdout: {stdout:?}"
    );
    assert!(
        stderr.contains(named),
        "stderr must hold {named:?}:\n{stderr}"
    );
}

/// Demand that a refused run in `repo` made nothing: no worktrees directory
/// beside the repository, and no worktree that git knows about other than the
/// main worktree.
///
/// `init_repo` puts the repository in a directory of its own, so the
/// temporary directory holds only `repo` until `nwt` makes something.
fn assert_made_nothing(temp: &tempfile::TempDir, repo: &Path) {
    let left: Vec<String> = std::fs::read_dir(temp.path())
        .expect("read the temporary directory")
        .map(|entry| {
            entry
                .expect("read one directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        left,
        vec!["repo".to_string()],
        "a refused run must make no worktrees directory"
    );

    let listed: Vec<PathBuf> = git_stdout(repo, &["worktree", "list", "--porcelain"])
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect();
    assert_eq!(
        listed,
        vec![canonical(repo)],
        "a refused run must leave the main worktree as the only worktree"
    );
}

/// Demand that `repo` has no branch named `branch`.
fn assert_no_branch(repo: &Path, branch: &str) {
    assert!(
        !run_git(
            repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}")
            ]
        ),
        "a refused run must make no branch {branch}"
    );
}

/// The ref that a run without `-c` checks each directory at.
const HEAD_REF: &str = "HEAD";

/// The tag of the commit before [`HEAD_ONLY_DIR`] takes the place of
/// [`TAG_ONLY_DIR`].
const OLD_TAG: &str = "before-move";

/// A directory that git tracks at [`OLD_TAG`] and not at `HEAD`.
const TAG_ONLY_DIR: &str = "old-heavy";

/// A directory that git tracks at `HEAD` and not at [`OLD_TAG`].
const HEAD_ONLY_DIR: &str = "new-heavy";

/// Run `nwt -c <reference>` in `repo` with `extra` arguments, without the
/// `.env` copy and the hook bootstrap, and hand back what it wrote.
fn run_nwt_checkout(repo: &Path, reference: &str, extra: &[&str]) -> Output {
    nwt_command(repo)
        .args(["-c", reference, "--no-copy-env", "--no-bootstrap-hooks"])
        .args(extra)
        .output()
        .expect("run the nwt binary")
}

/// The stderr line of a run that refuses `dir`, because git does not track it
/// as a directory at `at_ref`.
fn not_tracked_message(dir: &str, at_ref: &str) -> String {
    format!(
        "Error: --sparse-exclude '{dir}' is not a directory that git tracks at '{at_ref}'. \
         Give a directory that git tracks at that ref."
    )
}

/// Make a repository whose tag [`OLD_TAG`] tracks [`TAG_ONLY_DIR`], and whose
/// `HEAD` commit replaces that directory with [`HEAD_ONLY_DIR`].
///
/// Thus the main worktree holds [`HEAD_ONLY_DIR`] on disk and not
/// [`TAG_ONLY_DIR`]. A check that reads the disk or `HEAD` gives the opposite
/// answer to a check that reads the tag.
fn repo_with_a_tag_before_a_move() -> (tempfile::TempDir, PathBuf) {
    let tag_only_file = format!("{TAG_ONLY_DIR}/big.txt");
    let (temp, repo) = repo_with_files(&["heavy.txt", &tag_only_file]);
    assert!(run_git(&repo, &["tag", OLD_TAG]), "git tag failed");

    assert!(
        run_git(&repo, &["rm", "-r", "--quiet", "--", TAG_ONLY_DIR]),
        "git rm failed"
    );
    write_file(&repo, &format!("{HEAD_ONLY_DIR}/big.txt"), "big\n");
    assert!(run_git(&repo, &["add", "--", "."]), "git add failed");
    assert!(
        run_git(
            &repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "move the heavy dir"
            ]
        ),
        "git commit failed"
    );

    (temp, repo)
}

/// Test 6 of issue #487, the half that asks git: a value that is not a
/// tracked directory at `HEAD` exits with its own code, names the value and
/// the ref, and makes nothing.
///
/// `git ls-tree -d` prints nothing for a missing path and for a file, and it
/// exits 0 for both. The untracked `scratch/` is a directory on disk in the
/// main worktree, so its refusal proves that the check reads the ref and not
/// the disk. Without the check, git takes each pattern, excludes nothing, and
/// `nwt` reports success.
#[test]
fn a_value_that_is_not_a_tracked_directory_is_refused_and_makes_nothing() {
    for raw in ["nope", "heavy.txt", "scratch"] {
        let (temp, repo) = repo_with_heavy_dir();
        write_file(&repo, "scratch/notes.txt", "untracked\n");
        let branch = unique_branch("sparse-untracked");

        let output = run_nwt(&repo, &branch, &["--sparse-exclude", raw]);

        assert_refused(
            &output,
            INVALID_SPARSE_EXCLUDE,
            &not_tracked_message(raw, HEAD_REF),
        );
        assert_made_nothing(&temp, &repo);
        assert_no_branch(&repo, &branch);
    }
}

/// With `-c <ref>`, the check reads the ref that the worktree checks out.
///
/// [`HEAD_ONLY_DIR`] is on disk and at `HEAD`, but not at [`OLD_TAG`]. So a
/// worktree of [`OLD_TAG`] has no such directory to exclude, and `nwt`
/// refuses the value with the tag in the message.
#[test]
fn a_directory_that_is_not_at_the_checkout_ref_is_refused() {
    let (temp, repo) = repo_with_a_tag_before_a_move();

    let output = run_nwt_checkout(&repo, OLD_TAG, &["--sparse-exclude", HEAD_ONLY_DIR]);

    assert_refused(
        &output,
        INVALID_SPARSE_EXCLUDE,
        &not_tracked_message(HEAD_ONLY_DIR, OLD_TAG),
    );
    assert_made_nothing(&temp, &repo);
}

/// Demand that `worktree` does not hold `excluded`, holds each of `kept`, and
/// has no change for `git status` to report.
fn assert_sparse_worktree(worktree: &Path, excluded: &str, kept: &[&str]) {
    assert!(
        !worktree.join(excluded).exists(),
        "{excluded}/ must be out of the worktree at {}",
        worktree.display()
    );
    assert_files_present(worktree, kept);

    let status = git_stdout(worktree, &["status", "--short"]);
    assert!(
        status.is_empty(),
        "a sparse worktree has no change to report, but git status says:\n{status}"
    );
}

/// A tracked directory passes the check at the ref in each form that names
/// it.
///
/// - `heavy/` names the directory `heavy`, but `git ls-tree -d` prints the
///   children of `heavy/` and not `heavy`. The check must ask git about the
///   normalized value.
/// - `heavy/sub` is a directory below a directory. Git prints its full path.
/// - `café` holds a character that is not ASCII. Without `-z`, git prints the
///   name in quotes with octal escapes, and no entry is equal to the value.
#[test]
fn a_tracked_directory_passes_the_check_in_each_form() {
    const MULTIBYTE_DIR: &str = "café";
    const MULTIBYTE_FILE: &str = "café/menu.txt";

    let files: Vec<&str> = KEPT_FILES
        .iter()
        .chain(HEAVY_FILES)
        .chain(&[MULTIBYTE_FILE])
        .copied()
        .filter(|file| *file != "README.md")
        .collect();
    let (_temp, repo) = repo_with_files(&files);

    for (raw, excluded) in [
        ("heavy/", HEAVY_DIR),
        ("heavy/sub", "heavy/sub"),
        (MULTIBYTE_DIR, MULTIBYTE_DIR),
    ] {
        let output = run_nwt(
            &repo,
            &unique_branch("sparse-form"),
            &["--sparse-exclude", raw],
        );
        let worktree = created_worktree(&output);

        assert_sparse_worktree(&worktree, excluded, KEPT_FILES);
    }
}

/// With `-c <ref>`, a directory that git tracks at that ref passes the check,
/// although `HEAD` does not track it and the disk does not hold it.
#[test]
fn a_directory_at_the_checkout_ref_passes_although_head_lacks_it() {
    let (_temp, repo) = repo_with_a_tag_before_a_move();
    assert!(
        !repo.join(TAG_ONLY_DIR).exists(),
        "the main worktree must not hold {TAG_ONLY_DIR}/ on disk"
    );

    let output = run_nwt_checkout(&repo, OLD_TAG, &["--sparse-exclude", TAG_ONLY_DIR]);
    let worktree = created_worktree(&output);

    assert_sparse_worktree(&worktree, TAG_ONLY_DIR, &["README.md", "heavy.txt"]);
}

/// Test 8 of issue #487: `-c <tag>` gives a detached sparse worktree at the
/// commit of the tag.
#[test]
fn a_checkout_of_a_tag_gives_a_detached_sparse_worktree() {
    const TAG: &str = "v1";

    let (_temp, repo) = repo_with_heavy_dir();
    assert!(run_git(&repo, &["tag", TAG]), "git tag failed");

    let output = run_nwt_checkout(&repo, TAG, &["--sparse-exclude", HEAVY_DIR]);
    let worktree = created_worktree(&output);

    assert_eq!(
        git_stdout(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]).trim_end(),
        "HEAD",
        "a worktree of a tag has a detached HEAD"
    );
    assert_eq!(
        git_stdout(&worktree, &["rev-parse", "HEAD"]),
        git_stdout(&repo, &["rev-parse", &format!("{TAG}^{{commit}}")]),
        "the worktree must check out the commit of the tag"
    );
    assert_sparse_worktree(&worktree, HEAVY_DIR, KEPT_FILES);
}

/// The exit code `nwt` returns when git cannot make the worktree. A `-c <ref>`
/// that names no commit gets it without `--sparse-exclude` too.
const WORKTREE_FAILED: i32 = 7;

/// A `-c <ref>` that git cannot read is a bad ref and not a bad directory. So
/// the run exits with the code that a bad ref gets without the flag, names the
/// ref, and makes nothing.
///
/// `git ls-tree` exits with a status that is not zero for such a ref, and a
/// check that reads only its output takes the ref for a ref without the
/// directory.
#[test]
fn a_checkout_ref_that_git_cannot_read_is_a_failed_add_and_makes_nothing() {
    const MISSING_REF: &str = "no-such-ref";

    let (temp, repo) = repo_with_heavy_dir();

    let output = run_nwt_checkout(&repo, MISSING_REF, &["--sparse-exclude", HEAVY_DIR]);

    assert_refused(
        &output,
        WORKTREE_FAILED,
        &format!("Error: git cannot read the ref '{MISSING_REF}' to check --sparse-exclude: "),
    );
    assert_made_nothing(&temp, &repo);
}

/// Test 5 of issue #487: a directory whose name holds a gitignore special
/// character is excluded, and only that directory.
///
/// An absent directory is not sufficient proof. The unescaped pattern `!/a*/`
/// also excludes `a*`, because `*` matches the `*` too. So each case has a
/// sibling that the unescaped pattern gets wrong, and the sibling must stay:
///
/// - `!/we[ir]d dir/` matches `weid dir` and `werd dir`, and not the
///   directory itself.
/// - `!/a*/` matches `ab` too.
/// - `!/q?/` matches `qx` too.
/// - `!/back\slash/` reads `\s` as `s`, so it matches `backslash`, and not the
///   directory itself.
///
/// After the `!/` prefix, a `!` or a `#` is not at the start of the pattern,
/// and git does not read it as special. So `!bang` and `#hash` cannot fail
/// when the escape is removed. They prove that the escaped form still
/// excludes such a directory.
///
/// A name with `\`, `*`, or `?` is not a legal file name on Windows.
#[cfg(unix)]
#[test]
fn a_directory_named_with_each_glob_character_is_excluded_alone() {
    const EXCLUDED: &[&str] = &["we[ir]d dir", "a*", "q?", r"back\slash", "!bang", "#hash"];
    const SIBLINGS: &[&str] = &["weid dir", "ab", "qx", "backslash"];
    const FILE: &str = "file.txt";

    let files: Vec<String> = EXCLUDED
        .iter()
        .chain(SIBLINGS)
        .map(|dir| format!("{dir}/{FILE}"))
        .chain(std::iter::once("heavy.txt".to_owned()))
        .collect();
    let file_refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let (_temp, repo) = repo_with_files(&file_refs);

    let flags: Vec<&str> = EXCLUDED
        .iter()
        .flat_map(|dir| ["--sparse-exclude", dir])
        .collect();
    let output = run_nwt(&repo, &unique_branch("sparse-glob"), &flags);
    let worktree = created_worktree(&output);

    for dir in EXCLUDED {
        assert!(
            !worktree.join(dir).exists(),
            "--sparse-exclude {dir:?} must keep that directory out of {}",
            worktree.display()
        );
    }
    let kept: Vec<String> = SIBLINGS
        .iter()
        .map(|dir| format!("{dir}/{FILE}"))
        .chain(["README.md".to_owned(), "heavy.txt".to_owned()])
        .collect();
    let kept_refs: Vec<&str> = kept.iter().map(String::as_str).collect();
    assert_files_present(&worktree, &kept_refs);

    let status = git_stdout(&worktree, &["status", "--short"]);
    assert!(
        status.is_empty(),
        "a sparse worktree has no change to report, but git status says:\n{status}"
    );
}

/// The stderr line of a run that does not copy the untracked `heavy/.env`,
/// because it is under the excluded `heavy/`.
const SKIPPED_HEAVY_ENV: &str = "Skipped: heavy/.env (under excluded heavy/)";

/// Run `nwt -b <branch> --sparse-exclude heavy` in `repo` with the `.env` copy
/// on and `extra` arguments, and hand back what it wrote.
fn run_nwt_with_env_copy(repo: &Path, extra: &[&str]) -> Output {
    nwt_command(repo)
        .args(["-b", &unique_branch("sparse-env"), "--no-bootstrap-hooks"])
        .args(["--sparse-exclude", HEAVY_DIR])
        .args(extra)
        .output()
        .expect("run the nwt binary")
}

/// Test 9 of issue #487: an untracked `.env` under an excluded directory is
/// not copied, and stderr names it.
///
/// The `.env` copy walks the main worktree. A copy of `heavy/.env` makes
/// `heavy/` in the new worktree again, and that undoes the exclusion. The
/// top-level `.env` is not under `heavy/`, so the copy takes it as before.
#[test]
fn an_untracked_env_under_an_excluded_directory_is_not_copied() {
    const TOP_ENV: &str = ".env";
    const TOP_ENV_CONTENTS: &str = "TOP=1\n";

    let (_temp, repo) = repo_with_heavy_dir();
    write_file(&repo, "heavy/.env", "HEAVY=1\n");
    write_file(&repo, TOP_ENV, TOP_ENV_CONTENTS);

    let output = run_nwt_with_env_copy(&repo, &[]);
    let worktree = created_worktree(&output);

    assert!(
        !worktree.join(HEAVY_DIR).exists(),
        "the .env copy must not make {HEAVY_DIR}/ in {}",
        worktree.display()
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join(TOP_ENV))
            .ok()
            .as_deref(),
        Some(TOP_ENV_CONTENTS),
        "the top-level .env must be copied into the new worktree"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.lines().any(|line| line == SKIPPED_HEAVY_ENV),
        "stderr must hold the line {SKIPPED_HEAVY_ENV:?}, but it holds:\n{stderr}"
    );
}

/// With `--quiet`, the copy still skips `heavy/.env`, and stderr holds no
/// `Skipped:` line.
#[test]
fn quiet_skips_an_env_under_an_excluded_directory_without_a_line() {
    let (_temp, repo) = repo_with_heavy_dir();
    write_file(&repo, "heavy/.env", "HEAVY=1\n");

    let output = run_nwt_with_env_copy(&repo, &["--quiet"]);
    let worktree = created_worktree(&output);

    assert!(
        !worktree.join(HEAVY_DIR).exists(),
        "the .env copy must not make {HEAVY_DIR}/ in {}",
        worktree.display()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Skipped:"),
        "--quiet must print no Skipped: line, but stderr holds:\n{stderr}"
    );
}

/// The branch that a clone holds only as a remote-tracking branch.
const REMOTE_ONLY_BRANCH: &str = "foo";

/// Make a source repository whose branch [`REMOTE_ONLY_BRANCH`] tracks
/// [`KEPT_FILES`] and [`HEAVY_FILES`], and whose checked-out branch does not
/// track [`HEAVY_DIR`].
///
/// A check that reads `HEAD` of a clone thus finds no [`HEAVY_DIR`] to
/// exclude.
fn source_with_a_heavy_remote_branch() -> (tempfile::TempDir, PathBuf) {
    let (temp, repo) = repo_with_heavy_dir();
    assert!(
        run_git(&repo, &["branch", REMOTE_ONLY_BRANCH]),
        "git branch failed"
    );
    assert!(
        run_git(&repo, &["rm", "-r", "--quiet", "--", HEAVY_DIR]),
        "git rm failed"
    );
    assert!(
        run_git(
            &repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "drop the heavy dir"
            ]
        ),
        "git commit failed"
    );

    (temp, repo)
}

/// Clone `source` into a new temporary directory, and hand back the temporary
/// directory (keep it alive) and the clone.
///
/// The clone holds the checked-out branch of `source` as a local branch, and
/// every other branch only as `origin/<branch>`. The clone is named `repo`, as
/// `init_repo` names a repository, so [`assert_made_nothing`] can read it.
fn clone_of(source: &Path) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::TempDir::new().expect("create a temporary directory");
    let clone = temp.path().join("repo");
    let source = source.to_str().expect("utf-8 source path");
    let target = clone.to_str().expect("utf-8 clone path");

    assert!(
        run_git(temp.path(), &["clone", "--quiet", source, target]),
        "git clone failed"
    );

    (temp, clone)
}

/// Demand that `clone` has no local branch [`REMOTE_ONLY_BRANCH`], so only
/// git's checkout DWIM can find it.
fn assert_only_a_remote_holds_the_branch(clone: &Path) {
    let local = format!("refs/heads/{REMOTE_ONLY_BRANCH}");
    assert!(
        !run_git(clone, &["show-ref", "--verify", "--quiet", &local]),
        "the fixture clone must not hold {local}"
    );
}

/// With `-c <branch>` for a branch that the clone holds only as
/// `origin/<branch>`, the check reads that remote-tracking branch.
///
/// A plain `nwt -c foo` works in such a clone, because `git worktree add`
/// makes a local `foo` that tracks `origin/foo`. `git ls-tree foo` cannot read
/// `foo`, so a check that reads only the name the user typed refuses a run
/// that works without the flag.
#[test]
fn a_branch_that_only_a_remote_holds_passes_like_a_plain_checkout() {
    let (_source_temp, source) = source_with_a_heavy_remote_branch();
    let (_temp, clone) = clone_of(&source);
    assert_only_a_remote_holds_the_branch(&clone);

    let output = run_nwt_checkout(&clone, REMOTE_ONLY_BRANCH, &["--sparse-exclude", HEAVY_DIR]);
    let worktree = created_worktree(&output);

    assert_sparse_worktree(&worktree, HEAVY_DIR, KEPT_FILES);
    assert_eq!(
        git_stdout(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]).trim_end(),
        REMOTE_ONLY_BRANCH,
        "the worktree must check out a local {REMOTE_ONLY_BRANCH}, as git's DWIM makes it"
    );
}

/// The second remote of a clone that two remotes give [`REMOTE_ONLY_BRANCH`].
const SECOND_REMOTE: &str = "upstream";

/// A clone that two remotes give [`REMOTE_ONLY_BRANCH`].
struct TwoRemoteClone {
    /// The temporary directory that holds only the clone.
    temp: tempfile::TempDir,
    /// The clone.
    repo: PathBuf,
    /// The two source repositories, kept alive for the life of the clone.
    _sources: [tempfile::TempDir; 2],
}

/// Make a clone whose `origin` holds a [`REMOTE_ONLY_BRANCH`] without
/// [`HEAVY_DIR`], and whose [`SECOND_REMOTE`] holds one with it.
///
/// So a check that reads `origin` refuses [`HEAVY_DIR`] as a directory that
/// git does not track, and a check that reads [`SECOND_REMOTE`] accepts it.
fn clone_with_two_remotes_holding_the_branch() -> TwoRemoteClone {
    let (light_temp, light) = repo_with_files(&["heavy.txt", "src/heavy/lib.txt"]);
    assert!(
        run_git(&light, &["branch", REMOTE_ONLY_BRANCH]),
        "git branch failed"
    );
    let (heavy_temp, heavy) = source_with_a_heavy_remote_branch();

    let (temp, repo) = clone_of(&light);
    let heavy = heavy.to_str().expect("utf-8 source path");
    assert!(
        run_git(&repo, &["remote", "add", SECOND_REMOTE, heavy]),
        "git remote add failed"
    );
    assert!(
        run_git(&repo, &["fetch", "--quiet", SECOND_REMOTE]),
        "git fetch failed"
    );
    assert_only_a_remote_holds_the_branch(&repo);

    TwoRemoteClone {
        temp,
        repo,
        _sources: [light_temp, heavy_temp],
    }
}

/// When two remotes hold the branch and nothing names a default remote, git
/// refuses the add with `fatal: invalid reference: foo`. So the run exits as a
/// failed add, and it makes nothing.
///
/// A check that takes one of the two branches reads `origin/foo`, which does
/// not track `heavy/`, and exits 15. The run reads no global and no system
/// configuration, so a `checkout.defaultRemote` of the host cannot pick a
/// remote.
#[cfg(unix)]
#[test]
fn two_remotes_that_hold_the_branch_fail_like_the_add_and_make_nothing() {
    let fixture = clone_with_two_remotes_holding_the_branch();

    let output = nwt_command(&fixture.repo)
        .args([
            "-c",
            REMOTE_ONLY_BRANCH,
            "--no-copy-env",
            "--no-bootstrap-hooks",
        ])
        .args(["--sparse-exclude", HEAVY_DIR])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run the nwt binary");

    assert_refused(
        &output,
        WORKTREE_FAILED,
        &format!(
            "Error: git cannot read the ref '{REMOTE_ONLY_BRANCH}' to check --sparse-exclude: "
        ),
    );
    assert_made_nothing(&fixture.temp, &fixture.repo);
    assert_only_a_remote_holds_the_branch(&fixture.repo);
}

/// When two remotes hold the branch, `checkout.defaultRemote` names the remote
/// whose branch git checks out, and the check reads that branch.
///
/// `origin/foo` does not track `heavy/`, and `upstream/foo` does. A check that
/// reads `origin/foo` refuses the run with exit 15. A check that reads neither
/// exits 7.
#[test]
fn checkout_default_remote_picks_the_branch_that_two_remotes_hold() {
    let fixture = clone_with_two_remotes_holding_the_branch();
    assert!(
        run_git(
            &fixture.repo,
            &["config", "checkout.defaultRemote", SECOND_REMOTE]
        ),
        "git config failed"
    );

    let output = run_nwt_checkout(
        &fixture.repo,
        REMOTE_ONLY_BRANCH,
        &["--sparse-exclude", HEAVY_DIR],
    );
    let worktree = created_worktree(&output);

    assert_sparse_worktree(&worktree, HEAVY_DIR, KEPT_FILES);
    assert_eq!(
        git_stdout(&worktree, &["rev-parse", "HEAD"]),
        git_stdout(
            &fixture.repo,
            &[
                "rev-parse",
                &format!("refs/remotes/{SECOND_REMOTE}/{REMOTE_ONLY_BRANCH}")
            ]
        ),
        "the worktree must check out the branch of {SECOND_REMOTE}"
    );
}

/// The file name of the log that [`install_recording_hook`] writes.
const HOOK_LOG: &str = "post-checkout.log";

/// The word the recording hook writes when its working directory holds
/// [`HEAVY_DIR`].
const SAW_HEAVY: &str = "heavy";

/// The word the recording hook writes when its working directory does not hold
/// [`HEAVY_DIR`].
const SAW_NO_HEAVY: &str = "no-heavy";

/// Install a `post-checkout` hook into `hooks_dir` that appends one line to
/// `log` each time it runs, and point `core.hooksPath` of `repo` at it.
///
/// The line is the arguments of the hook, a `|`, and [`SAW_HEAVY`] or
/// [`SAW_NO_HEAVY`] for what the working directory of the hook holds. Git runs
/// the hook in the new worktree.
#[cfg(unix)]
fn install_recording_hook(repo: &Path, hooks_dir: &Path, log: &Path) {
    let log = log.to_str().expect("utf-8 log path");
    assert!(
        !log.contains('\''),
        "the log path goes into single quotes, so it cannot hold one: {log}"
    );

    support::install_post_checkout_hook(
        repo,
        hooks_dir,
        &format!(
            "if [ -e {HEAVY_DIR} ]; then seen={SAW_HEAVY}; else seen={SAW_NO_HEAVY}; fi\n\
             printf '%s|%s\\n' \"$*\" \"$seen\" >> '{log}'\n"
        ),
    );
}

/// Test 7 of issue #487: the `post-checkout` hook of a sparse run gets the
/// arguments that a plain add gives, runs one time, and sees the tree without
/// the excluded directory.
///
/// `git worktree add --no-checkout` runs no hook, so the sparse path runs it
/// with `git hook run` after the files are written. A hook that reads the null
/// old ref to find a new worktree must see the same list as before:
/// `<null object id> <HEAD> 1`. The null object id has the length of the hash.
#[cfg(unix)]
#[test]
fn the_post_checkout_hook_gets_the_arguments_of_a_plain_add_and_sees_the_sparse_tree() {
    let (_temp, repo) = repo_with_heavy_dir();
    let hooks = tempfile::TempDir::new().expect("create the hooks directory");
    let log = hooks.path().join(HOOK_LOG);
    install_recording_hook(&repo, hooks.path(), &log);

    created_worktree(&run_nwt(&repo, &unique_branch("hook-plain"), &[]));
    created_worktree(&run_nwt(
        &repo,
        &unique_branch("hook-sparse"),
        &["--sparse-exclude", HEAVY_DIR],
    ));

    let head = git_stdout(&repo, &["rev-parse", "HEAD"])
        .trim_end()
        .to_owned();
    let arguments = format!("{} {head} 1", "0".repeat(head.len()));
    let recorded = std::fs::read_to_string(&log).unwrap_or_default();

    assert_eq!(
        recorded.lines().collect::<Vec<&str>>(),
        vec![
            format!("{arguments}|{SAW_HEAVY}"),
            format!("{arguments}|{SAW_NO_HEAVY}"),
        ],
        "the plain run and then the sparse run must each run the hook one time, with the \
         same arguments, and the sparse run must hide {HEAVY_DIR}/ from it"
    );
}

/// A repository without a `post-checkout` hook gives a sparse run that works.
///
/// `git hook run` without `--ignore-missing` exits 1 with
/// `cannot find a hook named post-checkout`, and the run then reports a failed
/// hook. The fixture points `core.hooksPath` at an empty directory, so a hook
/// that the host configuration names cannot take the place of the missing hook.
#[test]
fn a_repository_without_a_post_checkout_hook_gives_a_sparse_run_that_works() {
    let (_temp, repo) = repo_with_heavy_dir();
    let hooks = tempfile::TempDir::new().expect("create the empty hooks directory");
    let hooks_dir = canonical(hooks.path());
    assert!(
        run_git(
            &repo,
            &[
                "config",
                "core.hooksPath",
                hooks_dir.to_str().expect("utf-8 hooks directory")
            ]
        ),
        "git config core.hooksPath failed"
    );

    let output = run_nwt(
        &repo,
        &unique_branch("no-hook"),
        &["--sparse-exclude", HEAVY_DIR],
    );
    let worktree = created_worktree(&output);

    assert_sparse_worktree(&worktree, HEAVY_DIR, KEPT_FILES);
}

/// The first executable `git` on the `PATH` of this test process.
///
/// The test finds git where the shell finds it, and names no fixed path.
#[cfg(unix)]
fn real_git() -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = std::env::var_os("PATH").expect("PATH is set");
    std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
        .expect("an executable git on PATH")
}

/// A `git` program in a temporary directory that reacts when an argument of a
/// call is one of its trigger words, and hands each call to the real git.
///
/// A test puts [`FakeGit::path_env`] into the `PATH` of the `nwt` child only,
/// and never into the environment of the test process. `nwt` finds `git`
/// through `PATH`, so each git child of `nwt` runs the fake. A git child of git
/// itself does not, because git puts its own exec path first in `PATH`.
///
/// Unix only: the fake is a POSIX `sh` script that the Unix permission bits
/// make executable.
#[cfg(unix)]
struct FakeGit {
    /// The directory that holds the fake. The fake goes away with it.
    dir: tempfile::TempDir,
}

#[cfg(unix)]
impl FakeGit {
    /// A fake that writes `word` to its stdout when an argument is `trigger`,
    /// and then runs the real git.
    fn writing_stdout(trigger: &str, word: &str) -> Self {
        Self::reacting(&[trigger], &format!("echo {word}"))
    }

    /// Write the fake: for each argument that is one of `triggers`, run the
    /// shell text `reaction`. Then run the real git with every argument.
    fn reacting(triggers: &[&str], reaction: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let real = real_git();
        let real = real.to_str().expect("utf-8 git path");
        assert!(
            !real.contains('\''),
            "the git path goes into single quotes, so it cannot hold one: {real}"
        );

        let dir = tempfile::TempDir::new().expect("create the fake git directory");
        let fake = dir.path().join("git");
        let script = format!(
            "#!/bin/sh\n\
             for argument in \"$@\"; do\n\
             \x20 case \"$argument\" in\n\
             \x20   {}) {reaction} ;;\n\
             \x20 esac\n\
             done\n\
             exec '{real}' \"$@\"\n",
            triggers.join("|")
        );
        std::fs::write(&fake, script).expect("write the fake git");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
            .expect("make the fake git executable");

        Self { dir }
    }

    /// A `PATH` value with the directory of the fake first, and the `PATH` of
    /// this test process after it.
    fn path_env(&self) -> std::ffi::OsString {
        let mut dirs = vec![self.dir.path().to_path_buf()];
        if let Some(existing) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&existing));
        }
        std::env::join_paths(dirs).expect("join the PATH value")
    }
}

/// Run `nwt` in `repo` with `arguments`, without the `.env` copy and the hook
/// bootstrap, with `fake` first on the `PATH` of the child, and hand back what
/// it wrote.
#[cfg(unix)]
fn run_nwt_with_fake_git(repo: &Path, fake: &FakeGit, arguments: &[&str]) -> Output {
    nwt_command(repo)
        .args(arguments)
        .args(["--no-copy-env", "--no-bootstrap-hooks"])
        .env("PATH", fake.path_env())
        .output()
        .expect("run the nwt binary")
}

/// Nothing that the hook step writes to stdout reaches the stdout of `nwt`,
/// which holds only the worktree path.
///
/// Git 2.55 sends the stdout of a hook to its own stderr, so a hook alone
/// cannot prove where `nwt` sends the stdout of `git hook run`. The fake git
/// writes to its own stdout when it runs `hook`, and that word must show on
/// stderr. The word of the hook must show on stderr too, which proves that the
/// hook ran.
#[cfg(unix)]
#[test]
fn nothing_the_hook_step_writes_to_stdout_reaches_the_stdout_of_nwt() {
    const HOOK_WORD: &str = "the-hook-wrote-this";
    const GIT_WORD: &str = "git-hook-run-wrote-this";

    let (_temp, repo) = repo_with_heavy_dir();
    let hooks = tempfile::TempDir::new().expect("create the hooks directory");
    support::install_post_checkout_hook(&repo, hooks.path(), &format!("echo {HOOK_WORD}\n"));
    let fake = FakeGit::writing_stdout("hook", GIT_WORD);
    let branch = unique_branch("hook-stdout");

    let output = run_nwt_with_fake_git(
        &repo,
        &fake,
        &["-b", &branch, "--sparse-exclude", HEAVY_DIR],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "nwt failed ({:?}):\n{stderr}",
        output.status.code()
    );
    assert_eq!(
        stdout,
        format!("{}\n", expected_worktree(&repo, &branch).display()),
        "stdout must be the worktree path and one line break"
    );
    for word in [HOOK_WORD, GIT_WORD] {
        assert!(
            stderr.lines().any(|line| line == word),
            "{word} must show on stderr, but stderr holds:\n{stderr}"
        );
    }
}

/// In a repository whose object ids are SHA-256, the hook of a sparse run gets
/// a null object id of 64 zeros, as a plain add gives it.
///
/// A null object id of 40 zeros is the SHA-1 value. A hook that compares the
/// old ref with the null object id of its own repository does not find a new
/// worktree with it.
#[cfg(unix)]
#[test]
fn a_sha256_repository_gives_the_hook_a_null_object_id_of_64_zeros() {
    const SHA256_HEX_LENGTH: usize = 64;

    let temp = tempfile::TempDir::new().expect("create a temporary directory");
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).expect("create the repository directory");
    for arguments in [
        &["init", "--quiet", "--object-format=sha256"][..],
        &["config", "user.email", "test@example.com"],
        &["config", "user.name", "Test User"],
        &["config", "maintenance.auto", "false"],
    ] {
        assert!(run_git(&repo, arguments), "git {arguments:?} failed");
    }
    for file in KEPT_FILES.iter().chain(HEAVY_FILES) {
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

    let hooks = tempfile::TempDir::new().expect("create the hooks directory");
    let log = hooks.path().join(HOOK_LOG);
    install_recording_hook(&repo, hooks.path(), &log);

    created_worktree(&run_nwt(
        &repo,
        &unique_branch("hook-sha256"),
        &["--sparse-exclude", HEAVY_DIR],
    ));

    let head = git_stdout(&repo, &["rev-parse", "HEAD"])
        .trim_end()
        .to_owned();
    assert_eq!(
        head.len(),
        SHA256_HEX_LENGTH,
        "the fixture must be a SHA-256 repository"
    );
    let recorded = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        recorded.lines().collect::<Vec<&str>>(),
        vec![format!(
            "{} {head} 1|{SAW_NO_HEAVY}",
            "0".repeat(SHA256_HEX_LENGTH)
        )],
        "the hook must get the SHA-256 null object id"
    );
}

/// A value that the lexical rules refuse exits with its own code, names the
/// value on stderr, prints no path, and makes nothing: no worktrees directory,
/// no worktree, and no branch.
///
/// Each case pairs the value with the form the message shows. A control
/// character shows as its escape, so the message stays on one line.
#[test]
fn a_lexically_bad_value_is_refused_and_makes_nothing() {
    let cases = [
        ("/abs", "/abs"),
        ("..", ".."),
        ("a/../b", "a/../b"),
        ("./", "./"),
        ("a\nb", "a\\nb"),
    ];

    for (raw, shown) in cases {
        let (temp, repo) = repo_with_heavy_dir();
        let branch = unique_branch("sparse-refused");

        let output = run_nwt(&repo, &branch, &["--sparse-exclude", raw]);

        assert_refused(
            &output,
            INVALID_SPARSE_EXCLUDE,
            &format!("Error: --sparse-exclude '{shown}'"),
        );
        assert_made_nothing(&temp, &repo);
        assert_no_branch(&repo, &branch);
    }
}
