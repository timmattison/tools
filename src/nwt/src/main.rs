use std::fs;
use std::process::{exit, Command};

use clap::Parser;
use names::Generator;
use repowalker::find_git_repo;

/// Exit codes for different failure modes
mod exit_codes {
    /// Not running inside a git repository
    pub const NOT_IN_REPO: i32 = 1;
    /// Invalid or missing repository name
    pub const INVALID_REPO_NAME: i32 = 2;
    /// Repository has no parent directory
    pub const NO_PARENT_DIR: i32 = 3;
    /// Failed to create worktrees directory
    pub const DIR_CREATE_FAILED: i32 = 4;
    /// Could not find available directory name after max attempts
    pub const NAME_COLLISION: i32 = 5;
    /// Git command failed to execute
    pub const GIT_COMMAND_ERROR: i32 = 6;
    /// Git worktree creation failed
    pub const WORKTREE_FAILED: i32 = 7;
    /// Path contains non-UTF8 characters
    pub const INVALID_PATH_ENCODING: i32 = 8;
}

/// Maximum attempts to find an available directory name before giving up.
/// The `names` crate has ~100 adjectives and ~200 nouns, giving ~20,000 combinations.
/// With 10 attempts, the probability of failure when <2000 worktrees exist is negligible.
const MAX_ATTEMPTS: u32 = 10;

/// Create a new git worktree with a Docker-style random name.
///
/// This tool simplifies creating git worktrees by automatically generating
/// unique directory names and managing the worktree directory structure.
///
/// # Examples
///
/// Create a worktree with a random name and branch:
///     nwt
///
/// Create a worktree with a specific branch name:
///     nwt --branch feature/my-feature
///
/// Checkout an existing branch in a new worktree:
///     nwt --checkout main
#[derive(Parser)]
#[command(name = "nwt")]
#[command(about = "Create a new git worktree with a Docker-style random name")]
#[command(long_about = "Creates a git worktree in a '{repo-name}-worktrees' directory alongside \
the repository. Generates Docker-style random names (adjective-noun) for both the directory \
and branch unless overridden.

EXAMPLES:
    nwt                              # Random name for both directory and branch
    nwt -b feature/login             # Custom branch name, random directory
    nwt -c main                      # Checkout existing 'main' branch
    nwt -c v1.0.0                    # Checkout a tag

EXIT CODES:
    0  Success
    1  Not in a git repository
    2  Invalid repository name
    3  Repository has no parent directory
    4  Failed to create worktrees directory
    5  Could not find available directory name
    6  Git command failed to execute
    7  Git worktree creation failed
    8  Path contains non-UTF8 characters")]
struct Cli {
    /// Specify branch name instead of generating a random one.
    #[arg(short, long, conflicts_with = "checkout")]
    branch: Option<String>,

    /// Checkout a specific ref/commit instead of creating a new branch.
    #[arg(short, long, conflicts_with = "branch")]
    checkout: Option<String>,

    /// Suppress error messages (only output worktree path on success).
    #[arg(short, long)]
    quiet: bool,
}

/// Prints an error message to stderr unless quiet mode is enabled.
macro_rules! error {
    ($quiet:expr, $($arg:tt)*) => {
        if !$quiet {
            eprintln!($($arg)*);
        }
    };
}

/// Generates a Docker-style random name in the format "adjective-noun".
///
/// Returns `None` if the generator fails to produce a name (should not happen
/// with the default generator configuration, but handled gracefully).
fn generate_docker_name(generator: &mut Generator) -> Option<String> {
    generator.next()
}

/// Sanitizes a repository name to only allow safe characters.
/// Uses an allowlist approach: only alphanumeric, hyphen, underscore, and dot are permitted.
/// All other characters are replaced with underscores.
fn sanitize_repo_name(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }

    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();

    // Reject names that are just dots (path traversal)
    if sanitized.chars().all(|c| c == '.') {
        return None;
    }

    // Reject names that start with a dot followed by only dots and underscores
    // (could be attempts to create hidden or traversal paths)
    if sanitized.starts_with('.') && sanitized[1..].chars().all(|c| c == '.' || c == '_') {
        return None;
    }

    Some(sanitized)
}

/// Determines the branch name to use based on CLI options and generated directory name.
fn get_branch_name<'a>(cli: &'a Cli, dir_name: &'a str) -> &'a str {
    cli.branch.as_deref().unwrap_or(dir_name)
}

fn main() {
    let cli = Cli::parse();

    // Find git repo root
    let repo_root = match find_git_repo() {
        Some(root) => root,
        None => {
            error!(cli.quiet, "Error: Not in a git repository");
            error!(cli.quiet, "Please run this command from within a git repository.");
            exit(exit_codes::NOT_IN_REPO);
        }
    };

    // Get repo name from path with sanitization (fail-fast on non-UTF8)
    let repo_name = match repo_root.file_name() {
        Some(name) => {
            let name_str = match name.to_str() {
                Some(s) => s,
                None => {
                    error!(
                        cli.quiet,
                        "Error: Repository name contains invalid UTF-8 characters"
                    );
                    exit(exit_codes::INVALID_PATH_ENCODING);
                }
            };
            match sanitize_repo_name(name_str) {
                Some(sanitized) => sanitized,
                None => {
                    error!(cli.quiet, "Error: Invalid repository name");
                    exit(exit_codes::INVALID_REPO_NAME);
                }
            }
        }
        None => {
            error!(cli.quiet, "Error: Could not determine repository name");
            exit(exit_codes::INVALID_REPO_NAME);
        }
    };

    // Build worktrees directory path
    let parent = match repo_root.parent() {
        Some(p) => p,
        None => {
            error!(cli.quiet, "Error: Repository has no parent directory");
            exit(exit_codes::NO_PARENT_DIR);
        }
    };
    let worktrees_dir = parent.join(format!("{}-worktrees", repo_name));

    // Create worktrees directory if needed
    if let Err(e) = fs::create_dir_all(&worktrees_dir) {
        error!(
            cli.quiet,
            "Error: Could not create worktrees directory '{}': {}",
            worktrees_dir.display(),
            e
        );
        exit(exit_codes::DIR_CREATE_FAILED);
    }

    // Generate random Docker-style name for the directory with collision detection.
    let mut generator = Generator::default();
    let (dir_name, worktree_path) = {
        let mut attempts = 0;
        loop {
            let name = match generate_docker_name(&mut generator) {
                Some(n) => n,
                None => {
                    error!(
                        cli.quiet,
                        "Error: Name generator failed to produce a name"
                    );
                    exit(exit_codes::NAME_COLLISION);
                }
            };
            let path = worktrees_dir.join(&name);

            if !path.exists() {
                break (name, path);
            }

            attempts += 1;
            if attempts >= MAX_ATTEMPTS {
                error!(
                    cli.quiet,
                    "Error: Could not find an available directory name after {} attempts",
                    MAX_ATTEMPTS
                );
                error!(cli.quiet, "Please try again or clean up unused worktrees.");
                exit(exit_codes::NAME_COLLISION);
            }
        }
    };

    // Convert path to string, checking for valid UTF-8
    let worktree_path_str = match worktree_path.to_str() {
        Some(s) => s.to_string(),
        None => {
            error!(
                cli.quiet,
                "Error: Worktree path contains invalid UTF-8 characters: {}",
                worktree_path.display()
            );
            error!(
                cli.quiet,
                "Please ensure the repository path contains only valid UTF-8 characters."
            );
            exit(exit_codes::INVALID_PATH_ENCODING);
        }
    };

    // Determine the branch name once (used for both command and error messages)
    let branch_name = get_branch_name(&cli, &dir_name);

    // Build and execute git worktree command
    let output = if let Some(ref checkout_ref) = cli.checkout {
        // Checkout existing ref: git worktree add <path> <ref>
        Command::new("git")
            .args(["worktree", "add", &worktree_path_str, checkout_ref])
            .current_dir(&repo_root)
            .output()
    } else {
        // Create new branch
        Command::new("git")
            .args(["worktree", "add", &worktree_path_str, "-b", branch_name])
            .current_dir(&repo_root)
            .output()
    };

    match output {
        Ok(result) => {
            if result.status.success() {
                println!("{}", worktree_path.display());
            } else {
                let stderr = String::from_utf8_lossy(&result.stderr);

                // Check for common errors and provide helpful messages
                if stderr.contains("already exists") {
                    if stderr.contains("is already used by worktree") {
                        error!(
                            cli.quiet,
                            "Error: The ref '{}' is already checked out in another worktree.",
                            cli.checkout.as_deref().unwrap_or(branch_name)
                        );
                    } else {
                        error!(cli.quiet, "Error: Branch '{}' already exists.", branch_name);
                        error!(
                            cli.quiet,
                            "Use --checkout to check out an existing branch instead."
                        );
                    }
                } else {
                    error!(cli.quiet, "Failed to create worktree: {}", stderr);
                }
                exit(exit_codes::WORKTREE_FAILED);
            }
        }
        Err(e) => {
            error!(cli.quiet, "Error running git command: {}", e);
            exit(exit_codes::GIT_COMMAND_ERROR);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_docker_name_format() {
        let mut generator = Generator::default();
        let name = generate_docker_name(&mut generator).expect("Generator should produce a name");
        assert!(name.contains('-'), "Name should contain a hyphen");

        let parts: Vec<&str> = name.split('-').collect();
        assert_eq!(parts.len(), 2, "Name should have exactly two parts");

        // Verify both parts are non-empty lowercase strings
        assert!(!parts[0].is_empty(), "Adjective should not be empty");
        assert!(!parts[1].is_empty(), "Noun should not be empty");
        assert!(
            parts[0].chars().all(|c| c.is_ascii_lowercase()),
            "Adjective should be lowercase"
        );
        assert!(
            parts[1].chars().all(|c| c.is_ascii_lowercase()),
            "Noun should be lowercase"
        );
    }

    #[test]
    fn test_generate_docker_name_returns_some() {
        let mut generator = Generator::default();
        let result = generate_docker_name(&mut generator);
        assert!(result.is_some(), "Generator should return Some");
    }

    #[test]
    fn test_generate_docker_name_randomness() {
        let mut generator = Generator::default();
        let name1 = generate_docker_name(&mut generator).expect("Generator should produce a name");
        let mut found_different = false;

        // Generate several names to check randomness (very unlikely all same)
        for _ in 0..10 {
            let name2 =
                generate_docker_name(&mut generator).expect("Generator should produce a name");
            if name1 != name2 {
                found_different = true;
                break;
            }
        }

        assert!(found_different, "Names should be randomly generated");
    }

    #[test]
    fn test_generator_reuse_produces_different_names() {
        let mut generator = Generator::default();
        let mut names = Vec::new();

        for _ in 0..5 {
            names.push(
                generate_docker_name(&mut generator).expect("Generator should produce a name"),
            );
        }

        // Check that we got at least some different names
        let unique_count = {
            let mut sorted = names.clone();
            sorted.sort();
            sorted.dedup();
            sorted.len()
        };

        assert!(
            unique_count > 1,
            "Reusing generator should produce different names"
        );
    }

    #[test]
    fn test_sanitize_repo_name_valid() {
        assert_eq!(
            sanitize_repo_name("my-repo"),
            Some("my-repo".to_string())
        );
        assert_eq!(
            sanitize_repo_name("my_repo"),
            Some("my_repo".to_string())
        );
        assert_eq!(
            sanitize_repo_name("MyRepo123"),
            Some("MyRepo123".to_string())
        );
        assert_eq!(
            sanitize_repo_name("repo.name"),
            Some("repo.name".to_string())
        );
    }

    #[test]
    fn test_sanitize_repo_name_replaces_invalid_chars() {
        assert_eq!(
            sanitize_repo_name("my/repo"),
            Some("my_repo".to_string())
        );
        assert_eq!(
            sanitize_repo_name("my\\repo"),
            Some("my_repo".to_string())
        );
        assert_eq!(
            sanitize_repo_name("my repo"),
            Some("my_repo".to_string())
        );
        assert_eq!(
            sanitize_repo_name("my:repo"),
            Some("my_repo".to_string())
        );
    }

    #[test]
    fn test_sanitize_repo_name_rejects_invalid() {
        assert_eq!(sanitize_repo_name(""), None);
        assert_eq!(sanitize_repo_name("."), None);
        assert_eq!(sanitize_repo_name(".."), None);
        assert_eq!(sanitize_repo_name("..."), None);
        assert_eq!(sanitize_repo_name("._"), None);
        assert_eq!(sanitize_repo_name(".._"), None);
    }

    #[test]
    fn test_sanitize_repo_name_allows_dotfiles() {
        // .gitignore style names should be allowed
        assert_eq!(
            sanitize_repo_name(".gitignore"),
            Some(".gitignore".to_string())
        );
        assert_eq!(
            sanitize_repo_name(".hidden-repo"),
            Some(".hidden-repo".to_string())
        );
    }

    #[test]
    fn test_get_branch_name_with_explicit_branch() {
        let cli = Cli {
            branch: Some("feature/test".to_string()),
            checkout: None,
            quiet: false,
        };
        assert_eq!(get_branch_name(&cli, "random-name"), "feature/test");
    }

    #[test]
    fn test_get_branch_name_with_generated_name() {
        let cli = Cli {
            branch: None,
            checkout: None,
            quiet: false,
        };
        assert_eq!(get_branch_name(&cli, "random-name"), "random-name");
    }

    #[test]
    fn test_cli_branch_and_checkout_conflict() {
        // This tests that clap correctly rejects conflicting options
        use clap::CommandFactory;
        let cmd = Cli::command();

        // Try to parse with both --branch and --checkout - should fail
        let result = cmd.try_get_matches_from(["nwt", "--branch", "foo", "--checkout", "bar"]);

        assert!(
            result.is_err(),
            "Should fail when both --branch and --checkout are provided"
        );

        let err = result.unwrap_err();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::ArgumentConflict,
            "Error should be an argument conflict"
        );
    }

    #[test]
    fn test_exit_codes_are_unique() {
        let codes = [
            exit_codes::NOT_IN_REPO,
            exit_codes::INVALID_REPO_NAME,
            exit_codes::NO_PARENT_DIR,
            exit_codes::DIR_CREATE_FAILED,
            exit_codes::NAME_COLLISION,
            exit_codes::GIT_COMMAND_ERROR,
            exit_codes::WORKTREE_FAILED,
            exit_codes::INVALID_PATH_ENCODING,
        ];

        let mut sorted = codes.to_vec();
        sorted.sort();
        sorted.dedup();

        assert_eq!(
            sorted.len(),
            codes.len(),
            "All exit codes should be unique"
        );
    }
}
