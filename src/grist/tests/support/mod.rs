//! Throwaway git repositories with known conflict shapes.
//!
//! Every repo lives in its own `TempDir`, so concurrent `cargo test` runs (the
//! pre-commit hook's and yours) never share a path.

// Each integration test binary compiles its own copy of this module and uses a
// different subset of the helpers.
#![allow(dead_code, reason = "shared fixture helpers, used per test binary")]

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

pub struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    /// Initialise a repo with `main` checked out and identity configured.
    pub fn init() -> Self {
        let dir = TempDir::new().expect("create temp dir");
        let repo = Self { dir };

        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.email", "grist@example.com"]);
        repo.git(&["config", "user.name", "grist test"]);
        // A commit-signing config in the developer's global gitconfig would
        // otherwise make every fixture commit prompt or fail.
        repo.git(&["config", "commit.gpgsign", "false"]);

        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Run a git command in the repo, panicking on failure.
    pub fn git(&self, args: &[&str]) -> String {
        self.git_in(self.dir.path(), args)
    }

    fn git_in(&self, cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap_or_else(|e| panic!("failed to spawn git {args:?}: {e}"));

        assert!(
            output.status.success(),
            "git {args:?} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Write `contents` to `name` and commit it.
    pub fn commit_file(&self, name: &str, contents: &str, message: &str) {
        std::fs::write(self.dir.path().join(name), contents).expect("write fixture file");
        self.git(&["add", name]);
        self.git(&["commit", "-q", "-m", message]);
    }

    /// Create `name` at HEAD and check it out.
    pub fn branch(&self, name: &str) {
        self.git(&["checkout", "-q", "-b", name]);
    }

    pub fn checkout(&self, name: &str) {
        self.git(&["checkout", "-q", name]);
    }

    pub fn rev_parse(&self, reference: &str) -> String {
        self.git(&["rev-parse", reference])
    }

    /// Check `branch` out into a second worktree, the way a real developer
    /// juggling several branches would have it.
    pub fn add_worktree(&self, branch: &str) -> PathBuf {
        let path = self.dir.path().join(format!("wt-{branch}"));
        self.git(&[
            "worktree",
            "add",
            "-q",
            path.to_str().expect("utf-8 worktree path"),
            branch,
        ]);
        path
    }
}

/// A numbered file with `count` lines, so edits can be placed far enough apart
/// that git's 3-line diff context does not make them overlap by accident.
pub fn numbered_lines(count: usize) -> String {
    (1..=count)
        .map(|n| format!("line{n}\n"))
        .collect::<Vec<_>>()
        .join("")
}

/// Replace the 1-indexed `line` of `numbered_lines`-style content.
pub fn replace_line(contents: &str, line: usize, replacement: &str) -> String {
    let mut lines: Vec<&str> = contents.lines().collect();
    lines[line - 1] = replacement;
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// A branch that rewrites the same region three times, and one that touches it
/// once. Landing the iterated branch second makes each of its three commits
/// collide with the already-squashed change; landing it first means only the
/// single-commit branch has to be replayed.
pub fn contested_region_repo() -> TestRepo {
    const CONTESTED_LINE: usize = 15;

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    repo.commit_file("shared.txt", &base, "base");

    repo.branch("iterated");
    for revision in 1..=3 {
        let contents = replace_line(&base, CONTESTED_LINE, &format!("iterated-v{revision}"));
        repo.commit_file("shared.txt", &contents, &format!("iterate {revision}"));
    }

    repo.checkout("main");
    repo.branch("single");
    let contents = replace_line(&base, CONTESTED_LINE, "single-edit");
    repo.commit_file("shared.txt", &contents, "single edit");

    repo.checkout("main");
    repo
}

/// `built-on-top` was branched from `groundwork`, not from main - the stacked
/// shape that makes squash merging different from a real merge, because
/// squashing `built-on-top` destroys the commit identity of the `groundwork`
/// commits buried inside it.
pub fn stacked_branches_repo() -> TestRepo {
    const CONTESTED_LINE: usize = 15;

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    repo.commit_file("shared.txt", &base, "base");

    repo.branch("groundwork");
    let groundwork = replace_line(&base, CONTESTED_LINE, "groundwork-edit");
    repo.commit_file("shared.txt", &groundwork, "groundwork");

    repo.branch("built-on-top");
    let stacked = replace_line(&groundwork, CONTESTED_LINE, "built-on-top-edit");
    repo.commit_file("shared.txt", &stacked, "built on top");

    repo.checkout("main");
    repo
}

/// Two branches that both rewrite the same line, so the simulation is
/// guaranteed to actually conflict and resolve rather than no-op.
pub fn conflicting_repo() -> TestRepo {
    const CONTESTED_LINE: usize = 15;

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    repo.commit_file("shared.txt", &base, "base");

    repo.branch("left");
    repo.commit_file(
        "shared.txt",
        &replace_line(&base, CONTESTED_LINE, "left-edit"),
        "left work",
    );

    repo.checkout("main");
    repo.branch("right");
    repo.commit_file(
        "shared.txt",
        &replace_line(&base, CONTESTED_LINE, "right-edit"),
        "right work",
    );

    repo.checkout("main");
    repo
}

