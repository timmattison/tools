//! Shared test-support for `swt`'s integration tests: throwaway git
//! repositories to run `swt`'s git code against, and the single entrance for
//! spawning the real binary.
//!
//! Every integration test that needs a repository builds one here rather than
//! reaching for the checkout the suite happens to be running in. Three
//! properties are stated and enforced in exactly one place, because a copy that
//! drifted would reintroduce each failure mode silently:
//!
//! - **The repository is a subdirectory of its [`TempDir`], never the temp dir
//!   itself.** `swt create` places a new worktree at
//!   `<repo>/../<repo name>--<name>-<token>.swt` — a *sibling* of the repo
//!   root. With the
//!   repo at the temp dir root that sibling
//!   would land in the shared system temp directory, where it escapes cleanup
//!   and where two concurrent runs of the same test collide on it.
//!   [`TestRepo::siblings`] is that parent directory, and it is inside the
//!   `TempDir`.
//! - **The host is scrubbed out of every child the suite spawns.** Git exports
//!   its own `GIT_` variables into the environment of a hook, and the
//!   pre-commit hook of this repository runs `cargo test`. Git obeys those
//!   variables before the working directory of a command. A leaked `GIT_DIR` or
//!   `GIT_INDEX_FILE` thus aims a fixture's `git init`/`add`/`commit` at *this*
//!   repository, and that has happened here before. A leaked
//!   `GIT_OBJECT_DIRECTORY` sends the objects of a fixture into another store,
//!   and a leaked `GIT_CONFIG_PARAMETERS` injects configuration into the child.
//!   So [`sandboxed`] sheds the whole `GIT_` prefix through
//!   [`gitscratch::shed_inherited_git_environment`]. It holds no list of names,
//!   because a list misses every variable that git adds later. The host's
//!   global and system gitconfig is a quieter version of the same problem: it
//!   decides hooks paths, aliases and credential helpers for a suite that must
//!   depend on its own fixture and nothing else. [`sandboxed`] points both at
//!   an empty file. It applies both rules at the one place that [`git_command`],
//!   [`swt_command`] and [`shell_command`] share. [`TestRepo::new`] also
//!   refuses to build a fixture while the git location variables are set. The
//!   tests that call the git functions of `swt` *in process* inherit the
//!   environment of this process, and the harness cannot protect them from the
//!   outside.
//! - **Every name is process-unique.** Two copies of this test binary run
//!   concurrently in this repo — the pre-commit hook's `cargo test` racing a
//!   manual one — so every worktree path and branch name is keyed on
//!   [`std::process::id`] plus a nanosecond timestamp via [`unique`].
//!
//! Each integration test file is compiled as its own crate that pulls this
//! module in via `mod support;`, so not every binary uses every helper — hence
//! the crate-level dead-code allowance below.
#![allow(
    dead_code,
    reason = "shared across integration-test crates; not every test binary uses every helper"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::TempDir;

/// The git location variables git exports into a hook's environment. A fixture
/// that inherits any of them operates on the real repository instead of itself.
const INHERITED_GIT_ENV: [&str; 4] = ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_PREFIX"];

/// Name of the repository directory inside each fixture's `TempDir`. The repo is
/// a subdirectory so that a worktree created beside it stays inside the temp dir.
const REPO_DIR: &str = "repo";

/// The branch that every fixture repository starts on. A test reads it back as
/// the parent branch of a child that `swt create` makes in the main checkout.
pub const MAIN_BRANCH: &str = "main";

/// The one file every fixture repository has committed, for tests that need a
/// tracked path to modify, stage or delete.
pub const TRACKED_FILE: &str = "tracked.txt";

/// Basename of the per-developer green-check override script.
pub const SWT_CHECK: &str = ".swt-check";

/// Arguments that look like options but are values. `swt` has no options of its
/// own beyond `--help`, so each of these is a worktree name or a worktree path
/// that happens to start with a hyphen, and the command that owns it — not clap —
/// must be the one to answer for it.
///
/// The list lives here because `create` and `merge` both take such a value, and
/// each pins it separately: `create` refuses the name against its naming rule,
/// `merge` resolves the path and reports that nothing is there. One list keeps
/// the two halves from drifting into covering different spellings.
pub const OPTION_LOOKING_NAMES: [&str; 3] = ["-b", "-rf", "--force"];

/// Suffix every worktree directory `swt create` builds carries.
pub const WORKTREE_SUFFIX: &str = ".swt";

/// Local git config every fixture pins, so a fixture behaves the same whatever
/// the developer's global config says. `core.excludesFile` matters as much as
/// the identity: a global gitignore would otherwise hide the untracked files the
/// dirt tests are built on.
const FIXTURE_CONFIG: [(&str, &str); 4] = [
    ("user.email", "swt-test@example.com"),
    ("user.name", "swt test"),
    ("commit.gpgsign", "false"),
    ("core.excludesFile", "/dev/null"),
];

/// Nanosecond timestamp for building process-unique, parallel-safe names.
pub fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos()
}

/// Builds a name no concurrent run can also be using: `<label>-<pid>-<nanos>`.
///
/// Every branch name and worktree path in the suite goes through this, so two
/// copies of the same test never contend for one shared resource.
pub fn unique(label: &str) -> String {
    format!("{label}-{}-{}", std::process::id(), nanos())
}

/// Fails loudly when the ambient environment would send fixture git commands at
/// the real repository.
///
/// The suite calls `swt`'s git functions in process, so those inherit this
/// process's environment and cannot be sandboxed by the harness. The repo's
/// pre-commit hook already scrubs these before `cargo test`; if that ever
/// regresses, this turns "the fixtures quietly rewrote the real branch" into an
/// immediate, self-explaining failure.
pub fn assert_git_env_is_sandboxed() {
    for var in INHERITED_GIT_ENV {
        assert!(
            std::env::var_os(var).is_none(),
            "{var} is set, so fixture git commands would target the real repository \
             instead of the temp one — run the suite with `env -u {}` (this is what \
             the pre-commit hook does)",
            INHERITED_GIT_ENV.join(" -u ")
        );
    }
}

/// Applies the isolation rules to a child the suite is about to spawn, whether
/// that child is a fixture's git, the real `swt` binary, or a shell that runs
/// git.
///
/// The single place the rules live, so the three entrances that build children
/// ([`git_command`], [`swt_command`] and [`shell_command`]) cannot drift apart —
/// a rule applied at only some of them leaves part of the suite reading the
/// host's git configuration while the harness reads as sandboxed. Two rules, in
/// this order:
///
/// - **Every inherited `GIT_` variable is removed**, through
///   [`gitscratch::shed_inherited_git_environment`]. The rule is the prefix,
///   not a list of names. Git obeys `GIT_OBJECT_DIRECTORY` and
///   `GIT_CONFIG_PARAMETERS` as it obeys `GIT_DIR`, and a list misses every
///   variable that git adds later. The child then acts on its working
///   directory and nothing else.
/// - **The host's global and system config is replaced with an empty file.**
///   Otherwise `core.hooksPath`, `pull.rebase`, aliases, advice settings and
///   credential helpers from the developer's or CI machine's gitconfig decide
///   what the child does. A fixture pins [`FIXTURE_CONFIG`] locally, which
///   covers those four keys and nothing else. This rule comes after the sweep.
///   The sweep removes both names when this process holds them, and a value
///   set after the sweep wins.
fn sandboxed(cmd: &mut Command) -> &mut Command {
    gitscratch::shed_inherited_git_environment(cmd);
    cmd.stdin(Stdio::null())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
}

/// A git invocation in `dir`, sandboxed from the host by [`sandboxed`] exactly
/// as every spawn of the binary under test is.
pub fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    sandboxed(&mut cmd).args(args).current_dir(dir);
    cmd
}

/// The shell that [`shell_command`] runs, by its absolute path. Every POSIX
/// system has a shell there.
const SHELL: &str = "/bin/sh";

/// A shell in `dir` that runs `script`, sandboxed from the host by
/// [`sandboxed`] exactly as every other spawn of the suite is.
///
/// A test uses it to run a command line as a person or a hook types it, with
/// quotes and command substitutions. `script` goes to the shell as the one
/// argument after `-c`. Every git that the script runs inherits the
/// environment of the shell, so the one sweep covers each of them too.
pub fn shell_command(dir: &Path, script: &str) -> Command {
    let mut cmd = Command::new(SHELL);
    sandboxed(&mut cmd).arg("-c").arg(script).current_dir(dir);
    cmd
}

/// Runs git in `dir`, tolerating a non-zero exit.
///
/// Returns whether git succeeded and its combined stdout/stderr — some fixtures
/// are *built* out of a git failure, so asserting success would fail the test on
/// the very thing it is arranging.
pub fn git_allowing_failure(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = git_command(dir, args)
        .output()
        .unwrap_or_else(|err| panic!("could not run git {args:?}: {err}"));
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), combined)
}

/// Runs git in `dir`, asserting success, and returns its trimmed combined output.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let (ok, out) = git_allowing_failure(dir, args);
    assert!(ok, "git {args:?} failed in {}: {out}", dir.display());
    out.trim().to_string()
}

/// A linked worktree added to a [`TestRepo`], and the branch checked out in it.
///
/// A linked worktree's `.git` is a regular *file* holding `gitdir: …` rather
/// than a directory, which is the shape `swt merge` actually runs in: the
/// workflow this tool serves never works in the main repo.
pub struct LinkedWorktree {
    /// Absolute path to the worktree directory.
    pub path: PathBuf,
    /// Branch checked out in it.
    pub branch: String,
}

/// A throwaway git repository with one commit, living inside its own [`TempDir`].
///
/// Dropping it deletes the repository *and* everything created beside it, so
/// callers must hold it for as long as they read from it.
pub struct TestRepo {
    /// Owns the temp dir; dropping it removes the whole fixture.
    _temp: TempDir,
    root: PathBuf,
    siblings: PathBuf,
}

impl TestRepo {
    /// Creates a repository on branch `main` with `tracked.txt` committed, an
    /// identity configured, signing disabled and global excludes neutralized.
    ///
    /// The paths handed back are symlink-resolved: `getcwd(2)` always answers
    /// with the physical path and on macOS `$TMPDIR` is a symlink, so every path
    /// git and `swt` print is already resolved. Comparing those against an
    /// unresolved temp path would otherwise never match.
    pub fn new() -> Self {
        assert_git_env_is_sandboxed();

        let temp = TempDir::new().expect("fixture temp dir");
        let siblings = fs::canonicalize(temp.path()).expect("canonical fixture temp dir");
        let root = siblings.join(REPO_DIR);
        fs::create_dir(&root).expect("repo subdirectory");

        git(&root, &["init", "--quiet", "-b", MAIN_BRANCH]);
        for (key, value) in FIXTURE_CONFIG {
            git(&root, &["config", "--local", key, value]);
        }
        fs::write(root.join(TRACKED_FILE), "original\n").expect("tracked fixture file");
        git(&root, &["add", "--", TRACKED_FILE]);
        git(&root, &["commit", "--quiet", "-m", "fixture"]);

        Self {
            _temp: temp,
            root,
            siblings,
        }
    }

    /// The repository root — an absolute, symlink-resolved path.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// The directory the repository sits *in*, and therefore where anything
    /// `swt` creates beside it lands. Inside the `TempDir`, so it is cleaned up
    /// and cannot collide with a concurrent run.
    pub fn siblings(&self) -> &Path {
        &self.siblings
    }

    /// Names a process-unique path beside the repository. Nothing is created.
    pub fn sibling(&self, label: &str) -> PathBuf {
        self.siblings.join(unique(label))
    }

    /// Runs git in the repository, asserting success; returns trimmed output.
    pub fn git(&self, args: &[&str]) -> String {
        git(&self.root, args)
    }

    /// Runs git in the repository, tolerating a non-zero exit.
    pub fn git_allowing_failure(&self, args: &[&str]) -> (bool, String) {
        git_allowing_failure(&self.root, args)
    }

    /// Writes a file at a repo-relative path, creating parent directories.
    /// Returns its absolute path. The file is left untracked.
    pub fn write(&self, rel_path: &str, contents: &str) -> PathBuf {
        let full = self.root.join(rel_path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("fixture parent directory");
        }
        fs::write(&full, contents).expect("fixture file");
        full
    }

    /// Writes, stages and commits a file, and returns its absolute path.
    pub fn commit_file(&self, rel_path: &str, contents: &str) -> PathBuf {
        let full = self.write(rel_path, contents);
        self.git(&["add", "--", rel_path]);
        self.git(&["commit", "--quiet", "-m", &format!("add {rel_path}")]);
        full
    }

    /// Adds a linked worktree beside the repository, on a fresh branch at `HEAD`.
    pub fn add_worktree(&self, label: &str) -> LinkedWorktree {
        self.add_worktree_on(label, &unique(&format!("swt/{label}")))
    }

    /// Adds a linked worktree beside the repository, on a new branch named
    /// `branch` at `HEAD`.
    ///
    /// A test that pins the names of a child needs to choose the branch of the
    /// parent, and [`TestRepo::add_worktree`] chooses a branch of its own. Each
    /// fixture is a private temporary repository, so a fixed branch name cannot
    /// meet a concurrent run. The directory name still comes from [`unique`], as
    /// every other path in the suite does.
    pub fn add_worktree_on(&self, label: &str, branch: &str) -> LinkedWorktree {
        let path = self.sibling(label);
        let path_arg = path.to_str().expect("utf-8 fixture path");
        self.git(&["worktree", "add", "--quiet", "-b", branch, path_arg, "HEAD"]);
        LinkedWorktree {
            path,
            branch: branch.to_string(),
        }
    }

    /// Every directory beside the repository that a `swt create <name>` could
    /// have left behind, sorted.
    ///
    /// The worktree path carries a uniqueness token minted inside the child
    /// process, so a test cannot predict it and has to go looking. The scan
    /// keeps each `.swt` entry whose file name contains `name`. That finds the
    /// current format, `<parent>--<name>-<token>.swt`, for a parent of any
    /// name. It also finds the older `<name>-<token>.swt` and the untokenized
    /// `<name>.swt`. A regression to an older format thus cannot leave an
    /// orphan that the "nothing survived" assertions do not see. Every name
    /// comes from [`unique`], so a match on containment cannot find the
    /// directory of another test.
    pub fn created_worktrees(&self, name: &str) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(&self.siblings)
            .expect("the fixture's sibling directory should be readable")
            .filter_map(|entry| {
                let entry = entry.expect("sibling directory entry");
                let file_name = entry.file_name().to_string_lossy().into_owned();
                let belongs = file_name.ends_with(WORKTREE_SUFFIX) && file_name.contains(name);
                belongs.then(|| entry.path())
            })
            .collect();
        found.sort();
        found
    }

    /// Every branch that a `swt create <name>` could have left behind, sorted.
    ///
    /// The `git branch --list` pattern is `swt/*<name>-*`. In that command a `*`
    /// also matches a `/`, so the pattern finds `swt/<parent>/<name>-<token>`
    /// for a parent branch of any depth. It also finds the older
    /// `swt/<name>-<token>`. A regression to that format thus cannot leave a
    /// branch that the "nothing survived" assertions do not see.
    pub fn created_branches(&self, name: &str) -> Vec<String> {
        let mut found = self.branches(&format!("swt/*{name}-*"));
        found.sort();
        found
    }

    /// The one worktree `swt create <name>` left beside the repository.
    ///
    /// Panics when there is not exactly one, naming what was found instead —
    /// "none" and "two" are different bugs and deserve different messages.
    pub fn sole_created_worktree(&self, name: &str) -> PathBuf {
        let mut found = self.created_worktrees(name);
        assert_eq!(
            found.len(),
            1,
            "expected exactly one worktree for {name:?} beside the repository, found {found:?}"
        );
        found.remove(0)
    }

    /// Lists the branches matching a `git branch --list` pattern.
    pub fn branches(&self, pattern: &str) -> Vec<String> {
        self.git(&["branch", "--list", "--format=%(refname:short)", pattern])
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect()
    }
}

impl Default for TestRepo {
    fn default() -> Self {
        Self::new()
    }
}

/// Drops an executable `.swt-check` override at a repository root, the way a
/// developer does. It is deliberately left untracked — that is exactly how the
/// escape hatch is documented, and why untracked files are not parent dirt.
///
/// Returns the path written.
#[cfg(unix)]
pub fn write_swt_check(root: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = root.join(SWT_CHECK);
    fs::write(&path, body).expect("write .swt-check");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod .swt-check");
    path
}

/// The body of a `.swt-check` that does nothing and exits with `status`.
pub fn exiting_check(status: i32) -> String {
    format!("#!/bin/sh\nexit {status}\n")
}

/// Builds a [`Command`] that runs the real `swt` binary in `cwd`.
///
/// The single, mandatory entrance for spawning `swt`. It is sandboxed by
/// [`sandboxed`], so the binary under test gets the same treatment a fixture's
/// own git gets: an empty global and system git config, no inherited `GIT_`
/// variable, and a nulled stdin so an unexpected prompt cannot hang the suite.
pub fn swt_command(cwd: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_swt"));
    sandboxed(&mut cmd).current_dir(cwd);
    cmd
}

/// Runs the real `swt` binary in `cwd` and captures its status, stdout and stderr.
pub fn run_swt(cwd: &Path, args: &[&str]) -> Output {
    swt_command(cwd)
        .args(args)
        .output()
        .expect("failed to run swt")
}

/// Standard output of a run, as the visible glyphs a user reads.
///
/// clap paints its help and its usage errors when it believes the run writes to
/// a terminal, and `CLICOLOR_FORCE=1` makes it paint whatever the run writes
/// to. The escape codes wrap whole words, so a match on one word survives them
/// and a match on a phrase does not: `Usage: swt create` carries codes between
/// `Usage:` and `swt create`, because clap gives the two spans different
/// styles.
///
/// A test that compares such a phrase against plain text thus passes in a
/// redirected run and fails under a hand-typed `git commit`, where the
/// pre-commit hook passes its terminal straight through. The codes come out
/// here rather than at each call site, so every assertion reads what a user
/// reads and none of them depends on the color decision of the run. See
/// "Colored Output in Tests" in CLAUDE.md.
pub fn stdout(output: &Output) -> String {
    testcolor::strip_ansi(&String::from_utf8_lossy(&output.stdout))
}

/// Standard error of a run, as the visible glyphs a user reads.
///
/// clap writes its usage errors here, so this side needs the same treatment as
/// [`stdout`].
pub fn stderr(output: &Output) -> String {
    testcolor::strip_ansi(&String::from_utf8_lossy(&output.stderr))
}

/// Runs the real `swt` binary in an empty directory of its own, outside any
/// repository, and captures its status, stdout and stderr.
///
/// For the invocations that must fail *before* git is ever reached — a bad
/// worktree name, a command line that does not parse. There is nothing here for
/// git to act on, so a pass is evidence the refusal came first rather than
/// after `swt` had already gone looking for a repository. The directory is
/// removed when the run returns, and its path is symlink-resolved for the same
/// reason [`TestRepo::new`]'s is: on macOS `$TMPDIR` is a symlink, so every path
/// the child reports back is already resolved.
pub fn run_swt_outside_a_repository(args: &[&str]) -> Output {
    let dir = TempDir::new().expect("scratch temp dir for a repository-free swt run");
    let canonical = fs::canonicalize(dir.path()).expect("canonical scratch temp dir");
    run_swt(&canonical, args)
}
