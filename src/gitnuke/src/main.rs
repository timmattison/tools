use std::path::{Path, PathBuf};
use std::process::{exit, Command};

use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;
use repowalker::find_git_repo;

/// Exit codes for different error conditions.
mod exit_codes {
    /// Not in a git repository.
    pub const NOT_IN_REPO: i32 = 1;
    /// A git command failed to execute or returned an error.
    pub const GIT_COMMAND_ERROR: i32 = 2;
    /// No worktree matched the target.
    pub const WORKTREE_NOT_FOUND: i32 = 3;
    /// The target matched more than one worktree.
    pub const MULTIPLE_MATCHES: i32 = 4;
}

/// Remove a git worktree and force-delete its branch.
///
/// The target names a worktree, not a branch to delete in isolation: gitnuke
/// resolves it against `git worktree list`, so the branch it deletes is
/// whatever that worktree had checked out. A detached-HEAD worktree is removed
/// with no branch deletion.
///
/// # Usage
///
/// ```sh
/// gitnuke ../feature-wt        # by path
/// gitnuke feature-wt           # by directory name
/// gitnuke issue-42             # by branch name
/// ```
///
/// # Exit Codes
///
/// - 0: Success
/// - 1: Not in a git repository
/// - 2: A git command failed
/// - 3: No worktree matched the target
/// - 4: The target matched more than one worktree
#[derive(Parser)]
#[command(name = "gitnuke")]
#[command(about = "Remove a git worktree and force-delete its branch")]
#[command(version = version_string!())]
struct Cli {
    /// Worktree to nuke: its path, its directory name, or its branch name.
    #[arg(required = true)]
    targets: Vec<String>,
}

/// Represents a single git worktree.
#[derive(Debug, Clone)]
struct Worktree {
    /// The filesystem path to this worktree.
    path: PathBuf,
    /// The branch name (without `refs/heads/` prefix), or None for detached HEAD.
    branch: Option<String>,
}

impl Worktree {
    /// The final path component (e.g. `absurd-rock` from a full path).
    fn dir_name(&self) -> Option<&str> {
        self.path.file_name()?.to_str()
    }
}

/// Parses the output of `git worktree list --porcelain`.
///
/// The porcelain format looks like:
/// ```text
/// worktree /path/to/repo
/// HEAD abc123...
/// branch refs/heads/main
///
/// worktree /path/to/worktree
/// HEAD def456...
/// detached
/// ```
///
/// For a detached HEAD the `branch` line is absent. The main worktree is always
/// the first block, and the order here is preserved so callers can rely on it.
fn parse_worktree_list(output: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            // A new block starts: flush the previous one.
            if let Some(path) = current_path.take() {
                worktrees.push(Worktree {
                    path,
                    branch: current_branch.take(),
                });
            }
            current_path = Some(PathBuf::from(path));
        } else if let Some(branch) = line.strip_prefix("branch ") {
            current_branch = Some(
                branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .to_string(),
            );
        }
        // Ignore HEAD/bare/detached/locked/prunable lines.
    }

    if let Some(path) = current_path {
        worktrees.push(Worktree {
            path,
            branch: current_branch,
        });
    }

    worktrees
}

/// Lists the worktrees of the repository containing `repo_root`.
fn get_worktrees(repo_root: &Path) -> Result<Vec<Worktree>, String> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("failed to execute git: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "git worktree list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(parse_worktree_list(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// Result of resolving a target string against the worktree list.
#[derive(Debug, PartialEq, Eq)]
enum Resolution {
    /// Exactly one worktree matched (index into the list).
    Single(usize),
    /// Several worktrees matched (indices into the list).
    Multiple(Vec<usize>),
    /// Nothing matched.
    NotFound,
}

/// Resolves a target to exactly one worktree.
///
/// Matching is deliberately **exact only** — path, then directory name, then
/// branch name. gitnuke destroys whatever it resolves, so it must never guess:
/// a substring match like cwt's would happily nuke `issue-421` when asked for
/// `issue-42`.
///
/// `cwd` is the directory relative paths are resolved against.
fn resolve_target(worktrees: &[Worktree], target: &str, cwd: Option<&Path>) -> Resolution {
    if target.is_empty() {
        return Resolution::NotFound;
    }

    // First: a path that points at one of the worktrees. Canonicalizing both
    // sides makes `.`, `..`, trailing slashes, and symlinked temp dirs all
    // resolve to the same thing.
    if let Some(canonical) = canonicalize_target(target, cwd) {
        if let Some(idx) = worktrees.iter().position(|wt| {
            wt.path
                .canonicalize()
                .is_ok_and(|p| paths_equal(&p, &canonical))
        }) {
            return Resolution::Single(idx);
        }
    }

    // Then: an exact directory name or branch name. Both passes are folded
    // together so a name that hits one worktree's directory and another's
    // branch is reported as ambiguous instead of silently preferring one.
    let hits: Vec<usize> = worktrees
        .iter()
        .enumerate()
        .filter(|(_, wt)| wt.dir_name() == Some(target) || wt.branch.as_deref() == Some(target))
        .map(|(idx, _)| idx)
        .collect();

    match hits.len() {
        0 => Resolution::NotFound,
        1 => Resolution::Single(hits[0]),
        _ => Resolution::Multiple(hits),
    }
}

/// Canonicalizes a possibly-relative target path, or None if it doesn't exist.
fn canonicalize_target(target: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let as_path = Path::new(target);
    let absolute = if as_path.is_absolute() {
        as_path.to_path_buf()
    } else {
        cwd?.join(as_path)
    };
    absolute.canonicalize().ok()
}

/// Compares two paths, handling case-insensitivity on macOS.
fn paths_equal(a: &Path, b: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        // The default macOS filesystem is case-insensitive.
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    }

    #[cfg(not(target_os = "macos"))]
    {
        a == b
    }
}

/// A failure to nuke one target: the message to print and the exit code to use.
struct NukeError {
    code: i32,
    message: String,
}

impl NukeError {
    fn new(code: i32, message: impl Into<String>) -> Self {
        NukeError {
            code,
            message: message.into(),
        }
    }
}

/// Removes a worktree, then deletes the branch it had checked out.
///
/// The branch is only deleted once the removal has actually succeeded, so a
/// refused removal never leaves the branch destroyed and the worktree standing.
fn nuke(repo_root: &Path, worktree: &Worktree) -> Result<(), NukeError> {
    let output = Command::new("git")
        .args(["worktree", "remove"])
        .arg(&worktree.path)
        .current_dir(repo_root)
        .output()
        .map_err(|e| {
            NukeError::new(
                exit_codes::GIT_COMMAND_ERROR,
                format!("failed to execute git: {e}"),
            )
        })?;

    if !output.status.success() {
        return Err(NukeError::new(
            exit_codes::GIT_COMMAND_ERROR,
            format!(
                "could not remove worktree {}: {}",
                worktree.path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    println!(
        "{} removed worktree {}",
        "gitnuke:".green().bold(),
        worktree.path.display()
    );

    let Some(branch) = &worktree.branch else {
        println!(
            "{} {} had a detached HEAD, so there is no branch to delete",
            "gitnuke:".green().bold(),
            worktree.path.display()
        );
        return Ok(());
    };

    delete_branch(repo_root, branch)
}

/// Force-deletes a branch (`git branch -D`), echoing git's own report.
fn delete_branch(repo_root: &Path, branch: &str) -> Result<(), NukeError> {
    let output = Command::new("git")
        .args(["branch", "-D", branch])
        .current_dir(repo_root)
        .output()
        .map_err(|e| {
            NukeError::new(
                exit_codes::GIT_COMMAND_ERROR,
                format!("failed to execute git: {e}"),
            )
        })?;

    if !output.status.success() {
        return Err(NukeError::new(
            exit_codes::GIT_COMMAND_ERROR,
            format!(
                "worktree removed, but branch '{branch}' could not be deleted: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    println!(
        "{} {}",
        "gitnuke:".green().bold(),
        String::from_utf8_lossy(&output.stdout).trim()
    );
    Ok(())
}

/// Renders the "no worktree matched" message, listing what is available.
fn not_found_message(worktrees: &[Worktree], target: &str) -> String {
    let mut message = format!("no worktree matches '{target}'. Known worktrees:");
    for wt in worktrees {
        let branch = wt.branch.as_deref().unwrap_or("detached HEAD");
        message.push_str(&format!("\n  {} [{branch}]", wt.path.display()));
    }
    message
}

/// Nukes one target, resolving it against a freshly listed set of worktrees.
fn nuke_target(repo_root: &Path, target: &str, cwd: Option<&Path>) -> Result<(), NukeError> {
    let worktrees =
        get_worktrees(repo_root).map_err(|e| NukeError::new(exit_codes::GIT_COMMAND_ERROR, e))?;

    match resolve_target(&worktrees, target, cwd) {
        Resolution::Single(idx) => nuke(repo_root, &worktrees[idx]),
        Resolution::Multiple(indices) => {
            let mut message =
                format!("'{target}' matches more than one worktree; use a path instead:");
            for idx in indices {
                message.push_str(&format!("\n  {}", worktrees[idx].path.display()));
            }
            Err(NukeError::new(exit_codes::MULTIPLE_MATCHES, message))
        }
        Resolution::NotFound => Err(NukeError::new(
            exit_codes::WORKTREE_NOT_FOUND,
            not_found_message(&worktrees, target),
        )),
    }
}

fn main() {
    let cli = Cli::parse();

    let Some(repo_root) = find_git_repo() else {
        eprintln!("{} not in a git repository", "gitnuke:".red().bold());
        exit(exit_codes::NOT_IN_REPO);
    };

    let cwd = std::env::current_dir().ok();

    // The worktree list is re-read per target because nuking one invalidates it.
    let mut first_error: Option<i32> = None;
    for target in &cli.targets {
        if let Err(error) = nuke_target(&repo_root, target, cwd.as_deref()) {
            eprintln!("{} {}", "gitnuke:".red().bold(), error.message);
            first_error.get_or_insert(error.code);
        }
    }

    if let Some(code) = first_error {
        exit(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(path: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            path: PathBuf::from(path),
            branch: branch.map(str::to_string),
        }
    }

    #[test]
    fn parses_porcelain_worktree_list() {
        let output = "\
worktree /repo
HEAD abc123
branch refs/heads/main

worktree /repo-worktrees/feature
HEAD def456
branch refs/heads/feature/login

worktree /repo-worktrees/detached
HEAD 789abc
detached
";
        let worktrees = parse_worktree_list(output);

        assert_eq!(worktrees.len(), 3);
        assert_eq!(worktrees[0].path, PathBuf::from("/repo"));
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert_eq!(worktrees[1].branch.as_deref(), Some("feature/login"));
        assert_eq!(worktrees[2].branch, None);
    }

    #[test]
    fn parses_final_block_without_trailing_blank_line() {
        let worktrees = parse_worktree_list("worktree /repo\nHEAD abc\nbranch refs/heads/main");

        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn resolves_exact_directory_and_branch_names() {
        let worktrees = vec![
            wt("/repo", Some("main")),
            wt("/wt/absurd-rock", Some("issue-42")),
        ];

        assert_eq!(
            resolve_target(&worktrees, "absurd-rock", None),
            Resolution::Single(1)
        );
        assert_eq!(
            resolve_target(&worktrees, "issue-42", None),
            Resolution::Single(1)
        );
    }

    #[test]
    fn never_resolves_a_substring_of_a_branch_name() {
        // A near-miss must be a miss: gitnuke destroys what it resolves.
        let worktrees = vec![wt("/repo", Some("main")), wt("/wt/x", Some("issue-421"))];

        assert_eq!(
            resolve_target(&worktrees, "issue-42", None),
            Resolution::NotFound
        );
        assert_eq!(resolve_target(&worktrees, "", None), Resolution::NotFound);
    }

    #[test]
    fn reports_ambiguity_between_a_directory_name_and_a_branch_name() {
        let worktrees = vec![
            wt("/repo", Some("main")),
            wt("/wt/shared", Some("branch-a")),
            wt("/wt/other", Some("shared")),
        ];

        assert_eq!(
            resolve_target(&worktrees, "shared", None),
            Resolution::Multiple(vec![1, 2])
        );
    }

    #[test]
    fn resolves_multibyte_directory_and_branch_names() {
        let worktrees = vec![
            wt("/repo", Some("main")),
            wt("/wt/日本語テスト", Some("機能/ログイン")),
            wt("/wt/café", Some("🎉-party")),
        ];

        assert_eq!(
            resolve_target(&worktrees, "日本語テスト", None),
            Resolution::Single(1)
        );
        assert_eq!(
            resolve_target(&worktrees, "機能/ログイン", None),
            Resolution::Single(1)
        );
        assert_eq!(
            resolve_target(&worktrees, "café", None),
            Resolution::Single(2)
        );
        assert_eq!(
            resolve_target(&worktrees, "🎉-party", None),
            Resolution::Single(2)
        );
    }

    #[test]
    fn not_found_message_lists_known_worktrees() {
        let worktrees = vec![wt("/repo", Some("main")), wt("/wt/x", None)];

        let message = not_found_message(&worktrees, "nope");

        assert!(message.contains("no worktree matches 'nope'"));
        assert!(message.contains("/repo [main]"));
        assert!(message.contains("/wt/x [detached HEAD]"));
    }
}
