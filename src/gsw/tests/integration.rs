//! End-to-end: drive the real `gsw` binary against a fresh temp git repo.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Scrub the git-location env vars that git exports when it invokes a hook.
///
/// In a *worktree*, git exports absolute `GIT_DIR`/`GIT_WORK_TREE`/
/// `GIT_INDEX_FILE` to the pre-commit hook. Those leak into child `git` and
/// `gsw` processes (the latter via `gix::discover`) and pin them to the *real*
/// repo regardless of `current_dir(tempdir)`, so fixture commits land in the
/// real repo and `gsw` reports the real repo's status. Every git and gsw
/// invocation here routes through this so the per-test tempdir is the target,
/// in both the main checkout (relative env, harmless) and worktrees (absolute).
fn scrub_git_env(cmd: &mut Command) -> &mut Command {
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
}

/// Build a `gsw` binary command with the git env already scrubbed and
/// `current_dir` set, ready for callers to add flags/env before running.
fn gsw_command(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gsw"));
    cmd.current_dir(dir);
    scrub_git_env(&mut cmd);
    cmd
}

fn run_git(dir: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    let status = scrub_git_env(&mut cmd)
        .status()
        .expect("failed to invoke git");
    assert!(status.success(), "git {args:?} failed");
}

/// Like `run_git` but does NOT assert success — for commands that exit
/// non-zero as part of normal operation (e.g. `git merge` on a conflict).
fn run_git_allow_fail(dir: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    let _ = scrub_git_env(&mut cmd)
        .status()
        .expect("failed to invoke git");
}

fn run_gsw(dir: &Path) -> String {
    run_gsw_args(dir, &[])
}

/// Run `gsw --no-color <extra…>` in `dir` with stdout captured (not a TTY) and
/// return its stdout. Capturing the output makes stdout a pipe, which is the
/// non-TTY path: watch mode auto-falls-back to a single one-shot render.
fn run_gsw_args(dir: &Path, extra: &[&str]) -> String {
    let output = gsw_command(dir)
        .arg("--no-color")
        .args(extra)
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
fn does_not_rewrite_the_index_so_a_concurrent_rebase_keeps_the_lock() {
    // Regression: under `viddy gsw`, gsw fires every couple of seconds. Any
    // code path that refreshes the index's cached stat data writes `.git/index`
    // back — taking `.git/index.lock` for the duration. A rebase running at
    // that instant loses the race for the lock and aborts with a "another git
    // process seems to be running" / index.lock error. gsw is a read-only
    // monitor and must never take the index lock. All git operations now go
    // through gix in-process, which reads the index but never writes it.
    let dir = setup_repo();
    let index_path = dir.path().join(".git").join("index");

    // Make the index's cached stat data unambiguously stale: backdate the
    // tracked file's mtime to a fixed time in the distant past — well before
    // the commit, so it sits *outside* git's "racy" window (which only kicks in
    // when a file's mtime is at or after the index's own timestamp). A plain
    // `git status` then re-stats a.txt, sees the mtime no longer matches the
    // index, re-hashes it, finds the content unchanged, and rewrites
    // `.git/index` to refresh the cached stat — taking `.git/index.lock` to do
    // so. If gsw ever regresses to shelling out to git for status/diff, the
    // stale mtime will trigger that write and this test will catch it. A fixed
    // backdate (rather than touch-with-now) keeps the trigger deterministic:
    // inside the racy window the refresh write is timing-dependent and the test
    // flakes.
    let stale = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_577_836_800);
    let file = std::fs::File::options()
        .write(true)
        .open(dir.path().join("a.txt"))
        .expect("open a.txt to backdate mtime");
    file.set_times(std::fs::FileTimes::new().set_modified(stale))
        .expect("backdate a.txt mtime");
    drop(file);

    let before = fs::read(&index_path).expect("read .git/index before gsw");
    let _ = run_gsw(dir.path());
    let after = fs::read(&index_path).expect("read .git/index after gsw");

    assert_eq!(
        before, after,
        "gsw rewrote .git/index. gsw reads the repo in-process via gix and must \
         never write the index — writing it takes .git/index.lock, which races a \
         concurrent rebase. A gix status/diff read must never touch .git/index.",
    );
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
    fs::write(
        dir.path().join("a.txt"),
        "changed line one\nchanged line two\n",
    )
    .unwrap();
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
    let output = gsw_command(dir.path())
        .arg("--no-color")
        .env("GIT_CEILING_DIRECTORIES", parent)
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
    let output = gsw_command(dir.path())
        .arg("--no-color")
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

    let output = gsw_command(&dir.path().join("sub"))
        .arg("--no-color")
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

    let output = gsw_command(dir.path())
        .env("COLUMNS", "50")
        // Make the test independent of the host's NO_COLOR / CLICOLOR setup.
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR")
        .env_remove("CLICOLOR_FORCE")
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

    let output = gsw_command(dir.path())
        .env("COLUMNS", "50")
        .env("NO_COLOR", "1")
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
    let output = gsw_command(dir.path())
        .args(["--no-color", "--no-log"])
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
    let output = gsw_command(dir.path())
        .args(["--no-color", "--log-lines", "1"])
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
    let output = gsw_command(dir.path())
        .args(["--no-color", "--log-lines", "2"])
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

    run_git(
        local,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
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
fn shows_behind_base_count_in_header_when_base_advances_past_fork_point() {
    // End-to-end: when the base branch has moved on past the fork point, the
    // live header must show the needs-rebase segment `, {m} behind` after
    // `ahead of {base}`. No remote is needed — this is purely local divergence
    // between `feature` and its base `main`. Fork `feature` off `main`, commit
    // on each, then sit on `feature`: it is 1 ahead of and 1 behind `main`.
    let dir = setup_repo();
    let p = dir.path();

    // Fork `feature` off the initial commit and land a commit on it.
    run_git(p, &["checkout", "-q", "-b", "feature"]);
    fs::write(p.join("feature.txt"), "feature work\n").unwrap();
    run_git(p, &["add", "feature.txt"]);
    run_git(p, &["commit", "-q", "-m", "feature commit"]);

    // Advance `main` past the fork point with its own commit.
    run_git(p, &["checkout", "-q", "main"]);
    fs::write(p.join("main.txt"), "main work\n").unwrap();
    run_git(p, &["add", "main.txt"]);
    run_git(p, &["commit", "-q", "-m", "main commit"]);

    // Back on `feature`: 1 ahead of `main`, 1 behind it — needs a rebase.
    run_git(p, &["checkout", "-q", "feature"]);

    let out = run_gsw(p);
    let header = out.lines().next().unwrap_or("");
    assert!(
        header.contains("1 commit ahead of main"),
        "header should show 1 commit ahead of the base: {header}",
    );
    assert!(
        header.contains(", 1 behind"),
        "header should show the needs-rebase segment when the base advanced: {header}",
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

    let baseline = gsw_command(dir.path())
        .arg("--no-color")
        .env("COLUMNS", "80")
        .output()
        .expect("baseline gsw failed");
    assert!(baseline.status.success());
    let baseline_str = String::from_utf8_lossy(&baseline.stdout);
    assert!(
        !baseline_str.contains('…'),
        "baseline width should fit the long path without truncation: {baseline_str}",
    );

    let with_offset = gsw_command(dir.path())
        .arg("--no-color")
        .arg("--width-offset")
        .arg("30")
        .env("COLUMNS", "80")
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
fn lines_env_under_watch_wrapper_keeps_output_within_content_area() {
    // viddy/watch capture stdout (no TTY) and export the terminal height via
    // LINES. But the wrapper paints its own chrome and only hands the command
    // a smaller content area: viddy 1.3.0 reserves 4 rows (measured — a
    // 30-row terminal shows 26 lines of output). gsw must fit its whole frame
    // within LINES minus that chrome, or the file list — which renders at the
    // bottom — scrolls off the fold and the user can't see their own changes.
    const VIDDY_CHROME_ROWS: usize = 4;
    let dir = setup_repo();
    // Many changed files so the frame *wants* far more than a short terminal.
    for i in 0..40 {
        fs::write(dir.path().join(format!("file_{i:02}.txt")), "x\n").unwrap();
    }
    let lines = 15_usize;
    let output = gsw_command(dir.path())
        .arg("--no-color")
        .env("COLUMNS", "80")
        .env("LINES", lines.to_string())
        .output()
        .expect("failed to invoke gsw");
    assert!(
        output.status.success(),
        "gsw exited non-zero: stderr = {}",
        String::from_utf8_lossy(&output.stderr),
    );
    let raw = String::from_utf8_lossy(&output.stdout);
    let count = raw.lines().count();
    let budget = lines - VIDDY_CHROME_ROWS;
    assert!(
        count <= budget,
        "gsw emitted {count} lines but viddy's content area is only LINES-{VIDDY_CHROME_ROWS}={budget}; the bottom file list would be clipped:\n{raw}",
    );
}

#[test]
fn short_file_list_renders_in_full_in_a_short_terminal() {
    // The user's report: with a couple of changed files and a long commit
    // log in a short terminal, the file list at the bottom was squeezed to a
    // single row with a "+N more file" footer (and clipped off-screen). Files
    // are the primary content, so a short list must render in full and the
    // log must yield rows — no truncation footer, every file visible.
    let dir = setup_repo();
    // Build a long log so it competes hard for rows.
    for i in 0..12 {
        fs::write(dir.path().join("a.txt"), format!("rev {i}\n")).unwrap();
        run_git(dir.path(), &["add", "a.txt"]);
        run_git(
            dir.path(),
            &["commit", "-q", "-m", &format!("log-subject-{i}")],
        );
    }
    // Exactly two changed files.
    fs::write(dir.path().join("f1.txt"), "one\n").unwrap();
    fs::write(dir.path().join("f2.txt"), "two\n").unwrap();
    run_git(dir.path(), &["add", "f1.txt", "f2.txt"]);

    let output = gsw_command(dir.path())
        .arg("--no-color")
        .env("COLUMNS", "80")
        .env("LINES", "12")
        .output()
        .expect("failed to invoke gsw");
    assert!(output.status.success(), "gsw exited non-zero");
    let raw = String::from_utf8_lossy(&output.stdout);
    assert!(
        raw.contains("f1.txt") && raw.contains("f2.txt"),
        "both files should be visible, not squeezed behind a '+N more' footer:\n{raw}",
    );
    assert!(
        !raw.contains("more file"),
        "a 2-file list must not show a truncation footer in a 12-row terminal:\n{raw}",
    );
}

#[test]
fn one_shot_flag_matches_default_piped_output() {
    // Acceptance: `gsw --one-shot` must be byte-identical to the existing
    // one-shot render. With stdout captured (non-TTY), the default `gsw`
    // auto-falls-back to one-shot, so the two invocations must agree exactly.
    // This pins that adding watch mode did not perturb the one-shot bytes.
    let dir = setup_repo();
    fs::write(dir.path().join("b.txt"), "untracked\n").unwrap();

    // The two invocations run back-to-back but a fraction of a second apart, so
    // any age rendered at second granularity ("last commit 0s ago", a file row
    // ending in "0s") can tick over between them and make the byte comparison
    // flake. Backdate every age source — the HEAD commit's committer time and
    // the untracked file's mtime — to a fixed instant over a day in the past so
    // the formatter renders them as `XdYh` (smallest shown unit is hours);
    // sub-second drift between the runs then cannot change the bytes.
    const FIXED_PAST: &str = "2020-01-01T12:34:56 +0000";
    let mut amend_cmd = Command::new("git");
    amend_cmd
        .args(["commit", "--amend", "--no-edit", "--date", FIXED_PAST])
        .env("GIT_COMMITTER_DATE", FIXED_PAST)
        .current_dir(dir.path());
    let amend = scrub_git_env(&mut amend_cmd)
        .status()
        .expect("failed to backdate commit");
    assert!(amend.success(), "git commit --amend (backdate) failed");

    // 2020-01-01T12:34:56 UTC in unix seconds — matches FIXED_PAST so the file
    // row and the commit age land in the same day-granular bucket.
    let fixed_mtime =
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_577_882_096);
    let file = std::fs::File::options()
        .write(true)
        .open(dir.path().join("b.txt"))
        .expect("open b.txt to backdate mtime");
    file.set_times(std::fs::FileTimes::new().set_modified(fixed_mtime))
        .expect("backdate b.txt mtime");
    drop(file);

    let default_piped = run_gsw_args(dir.path(), &[]);
    let explicit_one_shot = run_gsw_args(dir.path(), &["--one-shot"]);

    assert_eq!(
        default_piped, explicit_one_shot,
        "`gsw --one-shot` must be byte-identical to default piped `gsw`",
    );
}

#[test]
fn non_tty_renders_once_and_exits() {
    // Acceptance: with stdout not a TTY, watch mode behaves exactly like
    // one-shot — it renders the status once and exits zero, never entering the
    // event loop (which would block forever and hang this captured `output()`
    // call). Reaching the assertions at all proves it exited.
    let dir = setup_repo();
    let out = run_gsw_args(dir.path(), &[]);
    assert!(
        out.contains("main"),
        "a single render should still include the branch name: {out}",
    );
}

#[test]
fn shows_merge_indicator_with_conflict_count_during_a_conflicted_merge() {
    // Drive a real merge conflict: two branches edit the same line of the same
    // file, then merge — git stops with a.txt unmerged. gsw must surface a
    // dedicated indicator line between the header and the separator:
    // `⚠ merge · 1 conflict to resolve` (one conflicted path → singular).
    let dir = setup_repo(); // on `main`, a.txt = "initial\n"
    let p = dir.path();
    run_git(p, &["checkout", "-q", "-b", "other"]);
    fs::write(p.join("a.txt"), "from other\n").unwrap();
    run_git(p, &["commit", "-q", "-am", "other edit"]);
    run_git(p, &["checkout", "-q", "main"]);
    fs::write(p.join("a.txt"), "from main\n").unwrap();
    run_git(p, &["commit", "-q", "-am", "main edit"]);
    // `git merge` exits non-zero on conflict — expected, don't assert success.
    run_git_allow_fail(p, &["merge", "other"]);

    let out = run_gsw(p);
    assert!(
        out.contains("⚠ merge"),
        "output should show the merge-in-progress indicator: {out}",
    );
    assert!(
        out.contains("· 1 conflict to resolve"),
        "indicator should report the singular conflict count: {out}",
    );
}
