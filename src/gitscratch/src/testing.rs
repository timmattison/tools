//! Throwaway git repositories with known conflict shapes.
//!
//! Every tool built on `gitscratch` needs the same handful of repositories to
//! test against — a contested region, a stacked branch, a pair of branches that
//! tie on hunks but not on stops — so they live here once instead of being
//! rebuilt per crate and drifting apart.
//!
//! Every repo lives in its own `TempDir`, so concurrent `cargo test` runs (the
//! pre-commit hook's and yours) never share a path.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// A throwaway repository that deletes itself when dropped.
pub struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    /// Initialise a repo with `main` checked out and identity configured.
    ///
    /// # Panics
    ///
    /// Panics if the temporary directory cannot be created, if `git` is not
    /// installed, or if any of the setup commands fail.
    pub fn init() -> Self {
        let dir = TempDir::new().expect("create temp dir");
        let repo = Self { dir };

        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.email", "gitscratch@example.com"]);
        repo.git(&["config", "user.name", "gitscratch test"]);
        // A commit-signing config in the developer's global gitconfig would
        // otherwise make every fixture commit prompt or fail.
        repo.git(&["config", "commit.gpgsign", "false"]);

        repo
    }

    /// The repository's working directory.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Run a git command in the repo, panicking on failure.
    ///
    /// # Panics
    ///
    /// Panics if `git` cannot be spawned or exits non-zero; the panic message
    /// carries the command's stdout and stderr.
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
    ///
    /// # Panics
    ///
    /// Panics if the fixture file cannot be written or if git fails.
    pub fn commit_file(&self, name: &str, contents: &str, message: &str) {
        self.commit_files(&[(name, contents)], message);
    }

    /// Write every `(name, contents)` pair and land them in one commit, so a
    /// fixture can control whether an edit set arrives as one commit or several.
    ///
    /// # Panics
    ///
    /// Panics if a fixture file cannot be written or if git fails.
    pub fn commit_files(&self, files: &[(&str, &str)], message: &str) {
        for (name, contents) in files {
            std::fs::write(self.dir.path().join(name), contents).expect("write fixture file");
            self.git(&["add", name]);
        }
        self.git(&["commit", "-q", "-m", message]);
    }

    /// Create `name` at HEAD and check it out.
    ///
    /// # Panics
    ///
    /// Panics if git fails — most likely because `name` already exists.
    pub fn branch(&self, name: &str) {
        self.git(&["checkout", "-q", "-b", name]);
    }

    /// Check `name` out.
    ///
    /// # Panics
    ///
    /// Panics if git fails — most likely because `name` does not exist.
    pub fn checkout(&self, name: &str) {
        self.git(&["checkout", "-q", name]);
    }

    /// Resolve `reference` to its full object id.
    ///
    /// # Panics
    ///
    /// Panics if git fails — most likely because `reference` does not resolve.
    pub fn rev_parse(&self, reference: &str) -> String {
        self.git(&["rev-parse", reference])
    }

    /// Check `branch` out into a second worktree, the way a real developer
    /// juggling several branches would have it.
    ///
    /// # Panics
    ///
    /// Panics if the worktree path is not valid UTF-8 or if git fails — most
    /// likely because `branch` is already checked out somewhere.
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
///
/// # Panics
///
/// Panics if `line` is zero or beyond the end of `contents`.
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
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
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
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
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

/// Two branches that make the same two edits, packaged differently: `one` lands
/// both in a single commit, `two` splits them across two commits. Every
/// ordering hand-merges the same two hunks in the same two files, but replaying
/// `two`'s two commits halts the rebase twice where `one`'s single commit halts
/// it once - so the orderings tie on hunks and files while differing on stops.
///
/// That is the shape that catches anything treating equal hunks as equal cost.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
pub fn equal_hunks_unequal_stops_repo() -> TestRepo {
    const CONTESTED_LINE: usize = 15;

    let repo = TestRepo::init();
    let base = numbered_lines(30);
    repo.commit_files(&[("x.txt", &base), ("y.txt", &base)], "base");

    repo.branch("one");
    repo.commit_files(
        &[
            ("x.txt", &replace_line(&base, CONTESTED_LINE, "one-x")),
            ("y.txt", &replace_line(&base, CONTESTED_LINE, "one-y")),
        ],
        "one edits both files at once",
    );

    repo.checkout("main");
    repo.branch("two");
    repo.commit_file(
        "x.txt",
        &replace_line(&base, CONTESTED_LINE, "two-x"),
        "two edits x",
    );
    repo.commit_file(
        "y.txt",
        &replace_line(&base, CONTESTED_LINE, "two-y"),
        "two edits y",
    );

    repo.checkout("main");
    repo
}

/// Two branches that each add a file of their own and touch nothing else, so no
/// ordering conflicts and every ordering genuinely costs zero of everything.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
pub fn independent_branches_repo() -> TestRepo {
    let repo = TestRepo::init();
    repo.commit_file("shared.txt", &numbered_lines(30), "base");

    repo.branch("alpha");
    repo.commit_file("alpha.txt", "alpha work\n", "alpha work");

    repo.checkout("main");
    repo.branch("beta");
    repo.commit_file("beta.txt", "beta work\n", "beta work");

    repo.checkout("main");
    repo
}

/// Two branches that both rewrite the same line, so the simulation is
/// guaranteed to actually conflict and resolve rather than no-op.
///
/// # Panics
///
/// Panics if the repository cannot be built — git missing, or a command failing.
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
