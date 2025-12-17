use std::fs;
use std::process::{exit, Command};

use clap::Parser;
use names::Generator;
use repowalker::find_git_repo;

/// Create a new git worktree with a Docker-style random name.
#[derive(Parser)]
#[command(name = "nwt")]
#[command(about = "Create a new git worktree with a Docker-style random name")]
struct Cli {
    /// Specify branch name instead of generating a random one.
    #[arg(short, long, conflicts_with = "checkout")]
    branch: Option<String>,

    /// Checkout a specific ref/commit instead of creating a new branch.
    #[arg(short, long, conflicts_with = "branch")]
    checkout: Option<String>,
}

/// Generates a Docker-style random name in the format "adjective-noun".
fn generate_docker_name() -> String {
    Generator::default()
        .next()
        .expect("Name generator should always produce a name")
}

fn main() {
    let cli = Cli::parse();

    // Find git repo root
    let repo_root = match find_git_repo() {
        Some(root) => root,
        None => {
            eprintln!("Error: Not in a git repository");
            eprintln!("Please run this command from within a git repository.");
            exit(1);
        }
    };

    // Get repo name from path with sanitization
    let repo_name = match repo_root.file_name() {
        Some(name) => {
            let name_str = name.to_string_lossy();
            // Sanitize the name to prevent path traversal
            let sanitized: String = name_str
                .chars()
                .map(|c| if c == '/' || c == '\\' { '_' } else { c })
                .collect();
            if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
                eprintln!("Error: Invalid repository name");
                exit(1);
            }
            sanitized
        }
        None => {
            eprintln!("Error: Could not determine repository name");
            exit(1);
        }
    };

    // Build worktrees directory path
    let parent = match repo_root.parent() {
        Some(p) => p,
        None => {
            eprintln!("Error: Repository has no parent directory");
            exit(1);
        }
    };
    let worktrees_dir = parent.join(format!("{}-worktrees", repo_name));

    // Create worktrees directory if needed
    if let Err(e) = fs::create_dir_all(&worktrees_dir) {
        eprintln!(
            "Error: Could not create worktrees directory '{}': {}",
            worktrees_dir.display(),
            e
        );
        exit(1);
    }

    // Generate random Docker-style name for the directory with collision detection
    const MAX_ATTEMPTS: u32 = 10;
    let (dir_name, worktree_path) = {
        let mut attempts = 0;
        loop {
            let name = generate_docker_name();
            let path = worktrees_dir.join(&name);

            if !path.exists() {
                break (name, path);
            }

            attempts += 1;
            if attempts >= MAX_ATTEMPTS {
                eprintln!(
                    "Error: Could not find an available directory name after {} attempts",
                    MAX_ATTEMPTS
                );
                eprintln!("Please try again or clean up unused worktrees.");
                exit(1);
            }
        }
    };

    // Build git worktree command arguments
    let worktree_path_str = worktree_path.to_string_lossy().to_string();

    let output = if let Some(ref checkout_ref) = cli.checkout {
        // Checkout existing ref: git worktree add <path> <ref>
        Command::new("git")
            .args(["worktree", "add", &worktree_path_str, checkout_ref])
            .current_dir(&repo_root)
            .output()
    } else {
        // Create new branch
        let branch_name = cli.branch.as_deref().unwrap_or(&dir_name);
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
                let branch_name = cli.branch.as_deref().unwrap_or(&dir_name);

                // Check for common errors and provide helpful messages
                if stderr.contains("already exists") {
                    if stderr.contains("is already used by worktree") {
                        eprintln!("Error: The ref '{}' is already checked out in another worktree.",
                            cli.checkout.as_deref().unwrap_or(branch_name));
                    } else {
                        eprintln!("Error: Branch '{}' already exists.", branch_name);
                        eprintln!("Use --checkout to check out an existing branch instead.");
                    }
                } else {
                    eprintln!("Failed to create worktree: {}", stderr);
                }
                exit(1);
            }
        }
        Err(e) => {
            eprintln!("Error running git command: {}", e);
            exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_docker_name_format() {
        let name = generate_docker_name();
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
    fn test_generate_docker_name_randomness() {
        let name1 = generate_docker_name();
        let mut found_different = false;

        // Generate several names to check randomness (very unlikely all same)
        for _ in 0..10 {
            let name2 = generate_docker_name();
            if name1 != name2 {
                found_different = true;
                break;
            }
        }

        assert!(found_different, "Names should be randomly generated");
    }
}
