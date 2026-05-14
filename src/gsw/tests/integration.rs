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
