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

/// Stage a deeply-nested file under a long path so the rendered row's
/// path column is wide enough that narrow terminals will visibly truncate
/// it with `…`. Untracked nested dirs would otherwise collapse to their
/// topmost segment in `git status` output and defeat the test.
fn make_long_staged(dir: &Path) -> String {
    let rel = "a/very/long/path/to/deeply/nested/file.txt";
    let full = dir.join(rel);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(&full, "x\n").unwrap();
    run_git(dir, &["add", rel]);
    rel.to_string()
}

#[test]
fn columns_env_with_piped_stdout_narrows_width_and_preserves_colors() {
    // Simulate viddy: stdout is captured (no TTY) and the wrapper exports
    // its terminal's width via COLUMNS. gsw should use COLUMNS-1 (reserving
    // one cell for the wrapper's scroll bar) and force colors through.
    let dir = setup_repo();
    make_long_staged(dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_gsw"))
        .env("COLUMNS", "50")
        // Make the test independent of the host's NO_COLOR / CLICOLOR setup.
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR")
        .env_remove("CLICOLOR_FORCE")
        .current_dir(dir.path())
        .output()
        .expect("failed to invoke gsw");
    assert!(
        output.status.success(),
        "gsw exited non-zero: stderr = {}",
        String::from_utf8_lossy(&output.stderr),
    );
    let raw = String::from_utf8_lossy(&output.stdout);
    assert!(
        raw.contains('\u{1b}'),
        "expected ANSI color escapes when COLUMNS is set: {raw:?}",
    );
    assert!(
        raw.contains('…'),
        "long path should be left-truncated to fit COLUMNS-1: {raw:?}",
    );
}

#[test]
fn columns_env_ignored_when_no_color_env_is_set() {
    // NO_COLOR must win even when we'd otherwise force colors on under
    // a watch wrapper. https://no-color.org/
    let dir = setup_repo();
    make_long_staged(dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_gsw"))
        .env("COLUMNS", "50")
        .env("NO_COLOR", "1")
        .current_dir(dir.path())
        .output()
        .expect("failed to invoke gsw");
    assert!(output.status.success(), "gsw exited non-zero");
    let raw = String::from_utf8_lossy(&output.stdout);
    assert!(
        !raw.contains('\u{1b}'),
        "NO_COLOR=1 should suppress colors even under a watch wrapper: {raw:?}",
    );
}

#[test]
fn shows_recent_commit_subject_in_log_section() {
    // By default gsw should append a `git log --oneline`-style block so
    // `viddy gsw` shows recent commits alongside status. The setup repo has
    // one commit with subject "initial".
    let dir = setup_repo();
    let out = run_gsw(dir.path());
    assert!(
        out.contains("initial"),
        "default output should include the recent commit subject: {out}",
    );
}

#[test]
fn no_log_flag_suppresses_log_section() {
    let dir = setup_repo();
    let output = Command::new(env!("CARGO_BIN_EXE_gsw"))
        .args(["--no-color", "--no-log"])
        .current_dir(dir.path())
        .output()
        .expect("failed to invoke gsw");
    assert!(output.status.success(), "gsw exited non-zero");
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        !out.contains("initial"),
        "--no-log should hide the log section: {out}",
    );
}

#[test]
fn log_row_ends_with_commit_age_in_detailed_format() {
    // Each rendered log row should end with the commit age in the same
    // two-unit format the file rows and the header use (e.g. `0s`, `5m23s`,
    // `2h14m`, `3d12h`). The "initial" commit is freshly minted, so its row
    // should end with a digit followed by `s`/`m`/`h`/`d`.
    let dir = setup_repo();
    let output = Command::new(env!("CARGO_BIN_EXE_gsw"))
        .args(["--no-color", "--log-lines", "1"])
        .current_dir(dir.path())
        .output()
        .expect("failed to invoke gsw");
    assert!(output.status.success(), "gsw exited non-zero");
    let out = String::from_utf8_lossy(&output.stdout);
    let log_line = out
        .lines()
        .find(|l| l.contains("initial"))
        .expect("log row should appear");
    let trimmed = log_line.trim_end();
    let chars: Vec<char> = trimmed.chars().collect();
    let last = chars.last().copied().unwrap_or(' ');
    let prev = chars.iter().rev().nth(1).copied().unwrap_or(' ');
    assert!(
        matches!(last, 's' | 'm' | 'h' | 'd'),
        "log row should end with an age unit (s/m/h/d): {log_line:?}",
    );
    assert!(
        prev.is_ascii_digit(),
        "log row age unit should be preceded by a digit: {log_line:?}",
    );
}

#[test]
fn log_lines_flag_caps_visible_commits() {
    let dir = setup_repo();
    // Add several commits so we can verify --log-lines actually caps.
    for i in 0..5 {
        fs::write(dir.path().join("a.txt"), format!("rev {i}\n")).unwrap();
        run_git(dir.path(), &["add", "a.txt"]);
        run_git(
            dir.path(),
            &["commit", "-q", "-m", &format!("rev-{i}-subject")],
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_gsw"))
        .args(["--no-color", "--log-lines", "2"])
        .current_dir(dir.path())
        .output()
        .expect("failed to invoke gsw");
    assert!(output.status.success(), "gsw exited non-zero");
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("rev-4-subject"),
        "newest commit should be visible with --log-lines 2: {out}",
    );
    assert!(
        out.contains("rev-3-subject"),
        "second-newest commit should be visible with --log-lines 2: {out}",
    );
    assert!(
        !out.contains("rev-2-subject"),
        "third-newest commit should be hidden by --log-lines 2: {out}",
    );
}

#[test]
fn shows_upstream_ahead_and_behind_counts_when_branch_tracks_remote() {
    // End-to-end: a repo whose local branch tracks an upstream should have
    // gsw report ↑M ↓N <upstream> in the header. Set up a bare repo to act
    // as the remote, push the initial commit, then create divergence by
    // landing a commit on the "remote" side (via a second clone) and another
    // commit on the local side. The local branch ends up 1 ahead and 1
    // behind its upstream.
    let dir = setup_repo();
    let local = dir.path();

    let remote_dir = tempfile::tempdir().expect("remote tempdir");
    let remote = remote_dir.path();
    run_git(remote, &["init", "--bare", "-q", "-b", "main"]);

    run_git(local, &["remote", "add", "origin", remote.to_str().unwrap()]);
    run_git(local, &["push", "-q", "-u", "origin", "main"]);

    // Land a "remote" commit by cloning the bare repo, committing there,
    // and pushing back. This is what would normally happen when a teammate
    // pushes while you've been working locally.
    let other_dir = tempfile::tempdir().expect("other tempdir");
    let other = other_dir.path();
    run_git(other, &["clone", "-q", remote.to_str().unwrap(), "."]);
    run_git(other, &["config", "user.email", "other@example.com"]);
    run_git(other, &["config", "user.name", "Other"]);
    run_git(other, &["config", "commit.gpgsign", "false"]);
    fs::write(other.join("b.txt"), "from other\n").unwrap();
    run_git(other, &["add", "b.txt"]);
    run_git(other, &["commit", "-q", "-m", "remote-side commit"]);
    run_git(other, &["push", "-q", "origin", "main"]);

    // Now make a local commit so we're both ahead AND behind the upstream
    // without ever fetching the remote-side change.
    fs::write(local.join("c.txt"), "local only\n").unwrap();
    run_git(local, &["add", "c.txt"]);
    run_git(local, &["commit", "-q", "-m", "local-side commit"]);

    // Fetch so the tracking ref knows about the remote-side commit, but
    // don't merge. Now `git rev-list --left-right --count @{u}...HEAD`
    // reports 1 behind, 1 ahead.
    run_git(local, &["fetch", "-q"]);

    let out = run_gsw(local);
    let header = out.lines().next().unwrap_or("");
    assert!(
        header.contains("origin/main"),
        "header should name the upstream tracking branch: {header}",
    );
    assert!(
        header.contains("↑1"),
        "header should show 1 commit ahead of upstream: {header}",
    );
    assert!(
        header.contains("↓1"),
        "header should show 1 commit behind upstream: {header}",
    );
}

#[test]
fn omits_upstream_field_when_branch_has_no_remote() {
    // No `git remote add`, no `git push -u`. The branch has no upstream
    // configured, so gsw should not invent one or print arrows for a
    // nonexistent tracking ref.
    let dir = setup_repo();
    let out = run_gsw(dir.path());
    let header = out.lines().next().unwrap_or("");
    assert!(
        !header.contains('↑') && !header.contains('↓'),
        "header should not show upstream arrows when no upstream exists: {header}",
    );
}

#[test]
fn width_offset_flag_narrows_render() {
    // With a fixed COLUMNS, --width-offset should subtract that many cells
    // on top of the auto-detection, narrowing the file-row path column
    // enough to force ellipsis truncation.
    let dir = setup_repo();
    make_long_staged(dir.path());

    let baseline = Command::new(env!("CARGO_BIN_EXE_gsw"))
        .arg("--no-color")
        .env("COLUMNS", "80")
        .current_dir(dir.path())
        .output()
        .expect("baseline gsw failed");
    assert!(baseline.status.success());
    let baseline_str = String::from_utf8_lossy(&baseline.stdout);
    assert!(
        !baseline_str.contains('…'),
        "baseline width should fit the long path without truncation: {baseline_str}",
    );

    let with_offset = Command::new(env!("CARGO_BIN_EXE_gsw"))
        .arg("--no-color")
        .arg("--width-offset")
        .arg("30")
        .env("COLUMNS", "80")
        .current_dir(dir.path())
        .output()
        .expect("offset gsw failed");
    assert!(with_offset.status.success());
    let offset_str = String::from_utf8_lossy(&with_offset.stdout);
    assert!(
        offset_str.contains('…'),
        "--width-offset 30 should narrow render enough to truncate path: {offset_str}",
    );
}

#[test]
fn lines_env_under_watch_wrapper_keeps_output_within_terminal_height() {
    // viddy/watch capture stdout (no TTY) and export the terminal height via
    // LINES. gsw must fit its whole frame within that height — otherwise the
    // file list, which renders at the bottom, scrolls off the fold and the
    // user can't see their own changes. Without honoring LINES, gsw falls
    // back to a 24-row budget and overflows a short terminal.
    let dir = setup_repo();
    // Many changed files so the frame *wants* far more than a short terminal.
    for i in 0..40 {
        fs::write(dir.path().join(format!("file_{i:02}.txt")), "x\n").unwrap();
    }
    let lines = 15usize;
    let output = Command::new(env!("CARGO_BIN_EXE_gsw"))
        .arg("--no-color")
        .env("COLUMNS", "80")
        .env("LINES", lines.to_string())
        .current_dir(dir.path())
        .output()
        .expect("failed to invoke gsw");
    assert!(
        output.status.success(),
        "gsw exited non-zero: stderr = {}",
        String::from_utf8_lossy(&output.stderr),
    );
    let raw = String::from_utf8_lossy(&output.stdout);
    let count = raw.lines().count();
    assert!(
        count <= lines,
        "gsw emitted {count} lines but LINES={lines}; bottom rows (the file list) would be clipped:\n{raw}",
    );
}
