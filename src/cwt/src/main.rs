use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

use clap::Parser;
use colored::Colorize;
use repowalker::find_git_repo;

/// Exit codes for different error conditions.
mod exit_codes {
    /// Not in a git repository.
    pub const NOT_IN_REPO: i32 = 1;
    /// Git command failed to execute or returned an error.
    pub const GIT_COMMAND_ERROR: i32 = 2;
    /// The specified worktree was not found.
    pub const WORKTREE_NOT_FOUND: i32 = 3;
    /// Could not determine current worktree.
    pub const CURRENT_UNKNOWN: i32 = 4;
    /// Shell setup failed.
    pub const SHELL_SETUP_ERROR: i32 = 5;
}

/// Length of short commit hash for display (git uses 7 by default).
const SHORT_COMMIT_HASH_LENGTH: usize = 7;

/// Macro for printing error messages that respects quiet mode.
macro_rules! error {
    ($quiet:expr, $($arg:tt)*) => {
        if !$quiet {
            eprintln!($($arg)*);
        }
    };
}

/// Change to a different git worktree.
///
/// Lists all worktrees in the current repository or navigates to a specific one.
///
/// # Usage
///
/// ```sh
/// cwt           # Show list of worktrees with current highlighted
/// cwt -f        # Go to next worktree (wraps around)
/// cwt -p        # Go to previous worktree (wraps around)
/// cwt NAME      # Go to worktree by directory name or branch name
/// ```
///
/// # Shell Integration
///
/// Add this to your ~/.bashrc or ~/.zshrc:
///
/// ```sh
/// function wt() {
///     if [ $# -eq 0 ]; then
///         cwt
///     else
///         local target=$(cwt "$@")
///         if [ $? -eq 0 ] && [ -n "$target" ]; then
///             cd "$target"
///         fi
///     fi
/// }
///
/// alias wtf='wt -f'  # Next worktree
/// alias wtb='wt -p'  # Previous worktree (back)
/// ```
///
/// # Exit Codes
///
/// - 0: Success
/// - 1: Not in a git repository
/// - 2: Git command error
/// - 3: Worktree not found
/// - 4: Could not determine current worktree (for -f/-p)
/// - 5: Shell setup failed
#[derive(Parser)]
#[command(name = "cwt")]
#[command(about = "Change to a different git worktree")]
#[command(version)]
#[allow(clippy::struct_excessive_bools)] // CLI flags are naturally bool-heavy
struct Cli {
    /// Go to the next worktree (wraps around).
    #[arg(short = 'f', long, conflicts_with_all = ["prev", "target", "shell_setup"])]
    forward: bool,

    /// Go to the previous worktree (wraps around).
    #[arg(short = 'p', long, conflicts_with_all = ["forward", "target", "shell_setup"])]
    prev: bool,

    /// Worktree to switch to (directory name or branch name).
    #[arg(conflicts_with_all = ["forward", "prev", "shell_setup"])]
    target: Option<String>,

    /// Add shell integration (wt function and aliases) to your shell config.
    #[arg(long, conflicts_with_all = ["forward", "prev", "target"])]
    shell_setup: bool,

    /// Suppress error messages.
    #[arg(short, long)]
    quiet: bool,
}

/// Represents a single git worktree.
#[derive(Debug, Clone)]
struct Worktree {
    /// The filesystem path to this worktree.
    path: PathBuf,
    /// The HEAD commit hash.
    head: String,
    /// The branch name (without refs/heads/ prefix), or None for detached HEAD.
    branch: Option<String>,
}

impl Worktree {
    /// Get the final directory name (e.g., "absurd-rock" from full path).
    fn dir_name(&self) -> Option<&str> {
        self.path.file_name()?.to_str()
    }

    /// Get the branch name for display, or short commit hash for detached HEAD.
    fn display_branch(&self) -> String {
        if let Some(branch) = &self.branch { branch.clone() } else {
            // Show short commit hash for detached HEAD
            let short_hash = if self.head.len() >= SHORT_COMMIT_HASH_LENGTH {
                &self.head[..SHORT_COMMIT_HASH_LENGTH]
            } else {
                &self.head
            };
            format!("HEAD@{short_hash}")
        }
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
/// branch refs/heads/feature
/// ```
///
/// For detached HEAD, the branch line is absent.
fn parse_worktree_list(output: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_head: Option<String> = None;
    let mut current_branch: Option<String> = None;

    for line in output.lines() {
        if line.is_empty() {
            // End of a worktree block, save if we have the required fields.
            // Note: .take() already leaves the Option as None, so no need to reassign.
            if let (Some(path), Some(head)) = (current_path.take(), current_head.take()) {
                worktrees.push(Worktree {
                    path,
                    head,
                    branch: current_branch.take(),
                });
            }
        } else if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path));
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            current_head = Some(head.to_string());
        } else if let Some(branch) = line.strip_prefix("branch ") {
            // Strip the refs/heads/ prefix
            let branch_name = branch
                .strip_prefix("refs/heads/")
                .unwrap_or(branch);
            current_branch = Some(branch_name.to_string());
        }
        // Ignore other lines (like "bare" or "detached")
    }

    // Handle last block if output doesn't end with blank line
    if let (Some(path), Some(head)) = (current_path, current_head) {
        worktrees.push(Worktree {
            path,
            head,
            branch: current_branch,
        });
    }

    // Sort by path for consistent ordering
    worktrees.sort_by(|a, b| a.path.cmp(&b.path));

    worktrees
}

/// Gets all worktrees for the repository at the given root.
fn get_worktrees(repo_root: &Path) -> Result<Vec<Worktree>, String> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("Failed to execute git: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree list failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_worktree_list(&stdout))
}

/// Finds the index of the current worktree in the sorted list.
///
/// # Arguments
///
/// * `worktrees` - The list of worktrees to search
/// * `repo_root` - The root of the current repository (avoids redundant `find_git_repo()` call)
fn find_current_worktree(worktrees: &[Worktree], repo_root: &Path) -> Option<usize> {
    let canonical = std::fs::canonicalize(repo_root).ok()?;

    worktrees.iter().position(|wt| {
        std::fs::canonicalize(&wt.path)
            .map(|p| paths_equal(&p, &canonical))
            .unwrap_or(false)
    })
}

/// Compares two paths, handling case-insensitivity on macOS.
fn paths_equal(a: &Path, b: &Path) -> bool {
    // On macOS, the default filesystem is case-insensitive
    #[cfg(target_os = "macos")]
    {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    }

    #[cfg(not(target_os = "macos"))]
    {
        a == b
    }
}

/// Finds a worktree by name (directory name or branch name).
///
/// Rejects names containing path separators to prevent path traversal.
fn find_worktree_by_name(worktrees: &[Worktree], name: &str) -> Option<usize> {
    // Reject path traversal attempts
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return None;
    }

    // First: try exact directory name match
    if let Some(idx) = worktrees.iter().position(|wt| wt.dir_name() == Some(name)) {
        return Some(idx);
    }

    // Second: try exact branch name match
    worktrees
        .iter()
        .position(|wt| wt.branch.as_deref() == Some(name))
}

/// Displays the list of worktrees with the current one highlighted.
fn display_worktree_list(worktrees: &[Worktree], current_idx: Option<usize>) {
    for (idx, wt) in worktrees.iter().enumerate() {
        let is_current = current_idx == Some(idx);
        let marker = if is_current { ">" } else { " " };
        let path = wt.path.display().to_string();
        let branch = wt.display_branch();

        if is_current {
            println!(
                "{} {} [{}]",
                marker.green().bold(),
                path.green().bold(),
                branch.green()
            );
        } else {
            println!("{} {} [{}]", marker, path, branch.dimmed());
        }
    }
}

/// The shell integration code to add to shell config files.
const SHELL_INTEGRATION: &str = r#"
# cwt - Change Worktree shell integration
# Added by: cwt --shell-setup
function wt() {
    if [ $# -eq 0 ]; then
        # No args: show list interactively
        cwt
    else
        local target=$(cwt "$@")
        if [ $? -eq 0 ] && [ -n "$target" ]; then
            cd "$target"
        fi
    fi
}

# Quick navigation aliases (reuse wt function for proper error handling)
alias wtf='wt -f'  # Next worktree
alias wtb='wt -p'  # Previous worktree (back)
"#;

/// Marker to detect if shell integration is already installed.
const SHELL_INTEGRATION_MARKER: &str = "cwt - Change Worktree shell integration";

/// Sets up shell integration by adding the wt function to the user's shell config.
fn setup_shell_integration() -> Result<(), String> {
    // Get home directory
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;

    // Detect shell from SHELL environment variable
    let shell = std::env::var("SHELL").unwrap_or_default();
    let shell_name = Path::new(&shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    // Determine which config file to use
    let config_file = match shell_name {
        "zsh" => home.join(".zshrc"),
        "bash" => {
            // Prefer .bashrc, but use .bash_profile on macOS if .bashrc doesn't exist
            let bashrc = home.join(".bashrc");
            let bash_profile = home.join(".bash_profile");
            if bashrc.exists() {
                bashrc
            } else if bash_profile.exists() {
                bash_profile
            } else {
                bashrc // Create .bashrc if neither exists
            }
        }
        _ => {
            return Err(format!(
                "Unsupported shell: {shell_name}. Please manually add the shell integration to your config.\n\
                 See the README for shell integration examples: https://github.com/timmattison/tools#cwt-change-worktree"
            ));
        }
    };

    // Check if already installed
    if config_file.exists() {
        let contents = fs::read_to_string(&config_file)
            .map_err(|e| format!("Could not read {}: {}", config_file.display(), e))?;

        if contents.contains(SHELL_INTEGRATION_MARKER) {
            println!(
                "{} Shell integration already installed in {}",
                "✓".green(),
                config_file.display()
            );
            return Ok(());
        }
    }

    // Append shell integration to config file
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config_file)
        .map_err(|e| format!("Could not open {}: {}", config_file.display(), e))?;

    file.write_all(SHELL_INTEGRATION.as_bytes())
        .map_err(|e| format!("Could not write to {}: {}", config_file.display(), e))?;

    println!(
        "{} Shell integration added to {}",
        "✓".green(),
        config_file.display()
    );
    println!();
    println!("To activate, run:");
    println!("  {} {}", "source".cyan(), config_file.display());
    println!();
    println!("Or open a new terminal window.");
    println!();
    println!("Available commands:");
    println!("  {}  - List worktrees or change to one", "wt".yellow());
    println!("  {} - Next worktree", "wtf".yellow());
    println!("  {} - Previous worktree (back)", "wtb".yellow());

    Ok(())
}

fn main() {
    let cli = Cli::parse();

    // Handle shell setup (doesn't require being in a git repo)
    if cli.shell_setup {
        match setup_shell_integration() {
            Ok(()) => exit(0),
            Err(e) => {
                eprintln!("Error: {e}");
                exit(exit_codes::SHELL_SETUP_ERROR);
            }
        }
    }

    // Find git repo root
    let Some(repo_root) = find_git_repo() else {
        error!(cli.quiet, "Error: Not in a git repository");
        exit(exit_codes::NOT_IN_REPO);
    };

    // Get all worktrees
    let worktrees = match get_worktrees(&repo_root) {
        Ok(wts) => wts,
        Err(e) => {
            error!(cli.quiet, "Error getting worktrees: {}", e);
            exit(exit_codes::GIT_COMMAND_ERROR);
        }
    };

    if worktrees.is_empty() {
        error!(cli.quiet, "No worktrees found");
        exit(exit_codes::GIT_COMMAND_ERROR);
    }

    // Find current worktree (pass repo_root to avoid redundant find_git_repo() call)
    let current_idx = find_current_worktree(&worktrees, &repo_root);

    // Handle different modes
    if cli.forward {
        // Next worktree (wrap around)
        let target_idx = if let Some(i) = current_idx { (i + 1) % worktrees.len() } else {
            error!(cli.quiet, "Error: Could not determine current worktree");
            exit(exit_codes::CURRENT_UNKNOWN);
        };
        println!("{}", worktrees[target_idx].path.display());
    } else if cli.prev {
        // Previous worktree (wrap around)
        let target_idx = if let Some(i) = current_idx {
            if i == 0 {
                worktrees.len() - 1
            } else {
                i - 1
            }
        } else {
            error!(cli.quiet, "Error: Could not determine current worktree");
            exit(exit_codes::CURRENT_UNKNOWN);
        };
        println!("{}", worktrees[target_idx].path.display());
    } else if let Some(name) = &cli.target {
        // Find by name
        if let Some(idx) = find_worktree_by_name(&worktrees, name) {
            println!("{}", worktrees[idx].path.display());
        } else {
            error!(cli.quiet, "Error: Worktree '{}' not found", name);
            error!(cli.quiet, "Available worktrees:");
            for wt in &worktrees {
                let dir = wt.dir_name().unwrap_or("<unknown>");
                let branch = wt.display_branch();
                error!(cli.quiet, "  {} [{}]", dir, branch);
            }
            exit(exit_codes::WORKTREE_NOT_FOUND);
        }
    } else {
        // No args: display list
        display_worktree_list(&worktrees, current_idx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_worktree_list_single() {
        let output = "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main\n";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].path, PathBuf::from("/path/to/repo"));
        assert_eq!(worktrees[0].head, "abc123");
        assert_eq!(worktrees[0].branch, Some("main".to_string()));
    }

    #[test]
    fn test_parse_worktree_list_multiple() {
        let output = "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main\n\nworktree /path/to/wt\nHEAD def456\nbranch refs/heads/feature\n";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].path, PathBuf::from("/path/to/repo"));
        assert_eq!(worktrees[1].path, PathBuf::from("/path/to/wt"));
    }

    #[test]
    fn test_parse_worktree_list_detached_head() {
        let output = "worktree /path/to/repo\nHEAD abc123\ndetached\n";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch, None);
    }

    #[test]
    fn test_parse_worktree_list_sorted() {
        let output = "worktree /z/repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /a/repo\nHEAD def\nbranch refs/heads/feature\n";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 2);
        // Should be sorted by path
        assert_eq!(worktrees[0].path, PathBuf::from("/a/repo"));
        assert_eq!(worktrees[1].path, PathBuf::from("/z/repo"));
    }

    #[test]
    fn test_find_worktree_by_dir_name() {
        let worktrees = vec![
            Worktree {
                path: PathBuf::from("/repo"),
                head: "abc".to_string(),
                branch: Some("main".to_string()),
            },
            Worktree {
                path: PathBuf::from("/repo-wt/absurd-rock"),
                head: "def".to_string(),
                branch: Some("feature".to_string()),
            },
        ];
        assert_eq!(find_worktree_by_name(&worktrees, "absurd-rock"), Some(1));
    }

    #[test]
    fn test_find_worktree_by_branch() {
        let worktrees = vec![
            Worktree {
                path: PathBuf::from("/repo"),
                head: "abc".to_string(),
                branch: Some("main".to_string()),
            },
            Worktree {
                path: PathBuf::from("/repo-wt/absurd-rock"),
                head: "def".to_string(),
                branch: Some("feature".to_string()),
            },
        ];
        assert_eq!(find_worktree_by_name(&worktrees, "feature"), Some(1));
        assert_eq!(find_worktree_by_name(&worktrees, "main"), Some(0));
    }

    #[test]
    fn test_find_worktree_not_found() {
        let worktrees = vec![Worktree {
            path: PathBuf::from("/repo"),
            head: "abc".to_string(),
            branch: Some("main".to_string()),
        }];
        assert_eq!(find_worktree_by_name(&worktrees, "nonexistent"), None);
    }

    #[test]
    fn test_find_worktree_rejects_path_traversal() {
        let worktrees = vec![Worktree {
            path: PathBuf::from("/repo"),
            head: "abc".to_string(),
            branch: Some("main".to_string()),
        }];
        // Should reject path traversal attempts
        assert_eq!(find_worktree_by_name(&worktrees, "../etc/passwd"), None);
        assert_eq!(find_worktree_by_name(&worktrees, "foo/bar"), None);
        assert_eq!(find_worktree_by_name(&worktrees, "foo\\bar"), None);
        assert_eq!(find_worktree_by_name(&worktrees, ".."), None);
    }

    #[test]
    fn test_cycle_forward() {
        let current = 0;
        let count = 3;
        assert_eq!((current + 1) % count, 1);

        let current = 2;
        assert_eq!((current + 1) % count, 0); // Wraps
    }

    #[test]
    fn test_cycle_backward() {
        let count = 3;

        let current = 1;
        assert_eq!(if current == 0 { count - 1 } else { current - 1 }, 0);

        let current = 0;
        assert_eq!(if current == 0 { count - 1 } else { current - 1 }, 2); // Wraps
    }

    #[test]
    fn test_worktree_dir_name() {
        let wt = Worktree {
            path: PathBuf::from("/repo-worktrees/absurd-rock"),
            head: "abc".to_string(),
            branch: Some("feature".to_string()),
        };
        assert_eq!(wt.dir_name(), Some("absurd-rock"));
    }

    #[test]
    fn test_worktree_display_branch() {
        let with_branch = Worktree {
            path: PathBuf::from("/repo"),
            head: "abc".to_string(),
            branch: Some("main".to_string()),
        };
        assert_eq!(with_branch.display_branch(), "main");

        let detached = Worktree {
            path: PathBuf::from("/repo"),
            head: "abc1234567890".to_string(),
            branch: None,
        };
        assert_eq!(detached.display_branch(), "HEAD@abc1234");
    }

    #[test]
    fn test_parse_worktree_no_trailing_newline() {
        let output = "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch, Some("main".to_string()));
    }
}
