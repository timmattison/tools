//! End-to-end: drive the real `gsw` binary against a fresh temp git repo.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        // Make sure user/email config from $HOME doesn't bleed in.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .expect("failed to invoke git");
    assert!(status.success(), "git {args:?} failed");
}

fn run_gsw(dir: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_gsw"))
        .arg("--no-color")
        .current_dir(dir)
        .output()
        .expect("failed to invoke gsw");
    assert!(
        output.status.success(),
        "gsw exited non-zero: stderr = {}",
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn setup_repo() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path();
    run_git(p, &["init", "-q", "-b", "main"]);
    run_git(p, &["config", "user.email", "test@example.com"]);
    run_git(p, &["config", "user.name", "Test"]);
    run_git(p, &["config", "commit.gpgsign", "false"]);
    fs::write(p.join("a.txt"), "initial\n").unwrap();
    run_git(p, &["add", "a.txt"]);
    run_git(p, &["commit", "-q", "-m", "initial"]);
    dir
}

#[test]
fn shows_branch_and_header() {
    let dir = setup_repo();
    let out = run_gsw(dir.path());
    assert!(
        out.contains("main"),
        "output should include the branch name: {out}",
    );
    assert!(
        out.contains("commit"),
        "output should include a commit-count phrase: {out}",
    );
}

#[test]
fn shows_staged_modification() {
    let dir = setup_repo();
    fs::write(dir.path().join("a.txt"), "changed line one\nchanged line two\n").unwrap();
    run_git(dir.path(), &["add", "a.txt"]);

    let out = run_gsw(dir.path());
    assert!(out.contains("a.txt"), "should mention a.txt: {out}");
    // Staged modification → filled circle icon.
    assert!(out.contains('●'), "should mark staged with ●: {out}");
}

#[test]
fn shows_unstaged_modification() {
    let dir = setup_repo();
    fs::write(dir.path().join("a.txt"), "edited\n").unwrap();

    let out = run_gsw(dir.path());
    assert!(out.contains("a.txt"));
    assert!(out.contains('○'), "should mark unstaged with ○: {out}");
}

#[test]
fn shows_untracked_file() {
    let dir = setup_repo();
    fs::write(dir.path().join("new.txt"), "hello\n").unwrap();

    let out = run_gsw(dir.path());
    assert!(out.contains("new.txt"), "should list untracked file: {out}");
    assert!(out.contains('?'), "should mark untracked with ?: {out}");
}

#[test]
fn outside_git_repo_prints_friendly_header_and_exits_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Deliberately no `git init`. Set GIT_CEILING_DIRECTORIES so git won't
    // walk upward into whatever happens to be above /tmp on this host.
    let parent = dir.path().parent().unwrap_or(Path::new("/"));
    let output = Command::new(env!("CARGO_BIN_EXE_gsw"))
        .arg("--no-color")
        .current_dir(dir.path())
        .env("GIT_CEILING_DIRECTORIES", parent)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("failed to invoke gsw");
    assert!(
        output.status.success(),
        "gsw should exit 0 outside a repo: stderr = {}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("not a git repository"),
        "expected a friendly header outside a repo: {stdout}",
    );
}

#[test]
fn bare_repo_prints_friendly_header_and_exits_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A bare repo has no working tree, so gsw can't render a per-file view.
    // It should bail out cleanly the same way it does outside any repo.
    run_git(dir.path(), &["init", "--bare", "-q"]);
    let output = Command::new(env!("CARGO_BIN_EXE_gsw"))
        .arg("--no-color")
        .current_dir(dir.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("failed to invoke gsw");
    assert!(
        output.status.success(),
        "gsw should exit 0 in a bare repo: stderr = {}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("not a git repository"),
        "expected a friendly header in a bare repo: {stdout}",
    );
}

#[test]
fn shows_age_for_modified_file_when_run_from_subdir() {
    // git status --porcelain=v2 -z reports paths relative to the repo root,
    // not the cwd. If gsw resolves those paths against cwd, every fs::metadata
    // lookup fails when gsw runs from a subdirectory — every age becomes None
    // and the row collapses to a placeholder. Make sure gsw resolves paths
    // against the repo root so ages still appear from a subdirectory.
    let dir = setup_repo();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("a.txt"), "edited from sub\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_gsw"))
        .arg("--no-color")
        .current_dir(dir.path().join("sub"))
        .output()
        .expect("failed to invoke gsw");
    assert!(output.status.success(), "gsw exited non-zero");
    let out = String::from_utf8_lossy(&output.stdout);

    let row = out
        .lines()
        .find(|l| l.contains("a.txt"))
        .unwrap_or_else(|| panic!("expected a row for a.txt: {out}"));
    assert!(
        !row.contains('\u{2014}'),
        "modified file should show an age, not the em-dash placeholder: {row}",
    );
    assert!(
        row.contains('s') || row.contains('m') || row.contains('h') || row.contains('d'),
        "modified file row should include a duration suffix (s/m/h/d): {row}",
    );
}

#[test]
fn shows_untracked_directory_with_slash() {
    let dir = setup_repo();
    fs::create_dir(dir.path().join("new-dir")).unwrap();
    fs::write(dir.path().join("new-dir").join("inside.txt"), "x\n").unwrap();

    let out = run_gsw(dir.path());
    assert!(
        out.contains("new-dir/"),
        "untracked dir should show with trailing slash: {out}",
    );
}
