use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, exit};
use walkdir::{DirEntry, WalkDir};
use clap::Parser;

fn find_git_repo() -> Option<String> {
    let mut current_dir = env::current_dir().ok()?;
    
    loop {
        let git_dir = current_dir.join(".git");
        if git_dir.exists() {
            return Some(current_dir.to_string_lossy().to_string());
        }
        
        if !current_dir.pop() {
            break;
        }
    }
    
    None
}

fn run_command_in_directory(dir: &Path, command: &[&str]) -> Result<(), std::io::Error> {
    let output = Command::new(command[0])
        .args(&command[1..])
        .current_dir(dir)
        .output()?;
    
    if !output.status.success() {
        eprintln!("Warning: Error running {} in {}: {}", 
                 command.join(" "), 
                 dir.display(), 
                 String::from_utf8_lossy(&output.stderr));
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "Command failed"));
    }
    
    println!("Ran {} in {}", command.join(" "), dir.display());
    Ok(())
}

fn is_git_worktree(dir: &Path) -> bool {
    let git_path = dir.join(".git");
    
    // If .git is a file (not a directory), it's likely a worktree
    if git_path.is_file() {
        if let Ok(content) = fs::read_to_string(&git_path) {
            // Git worktrees have .git files that contain "gitdir: <path>"
            return content.trim().starts_with("gitdir:");
        }
    }
    
    false
}

fn should_skip_entry(entry: &DirEntry, repo_root: &Path) -> bool {
    // Skip any path that has node_modules as a component
    if entry.file_name() == "node_modules" {
        return true;
    }
    
    // Skip git worktree directories, but only if they're not the repo root we're running from
    if entry.file_type().is_dir() && is_git_worktree(entry.path()) {
        // Allow the root directory we're running from, even if it's a worktree
        if entry.path() != repo_root {
            println!("Skipping git worktree directory: {}", entry.path().display());
            return true;
        }
    }
    
    false
}

fn check_cargo_edit_installed() -> bool {
    Command::new("cargo")
        .args(&["upgrade", "--help"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Polish Rust dependencies - update crates in a repository", long_about = None)]
struct Args {
    /// Use latest versions for Rust crates (requires cargo-edit)
    #[arg(long, short = 'l')]
    latest: bool,
}

fn main() {
    let args = Args::parse();
    
    let repo_root = match find_git_repo() {
        Some(root) => root,
        None => {
            eprintln!("Error: Could not find git repository");
            exit(1);
        }
    };
    
    let repo_path = Path::new(&repo_root);
    
    println!("Polishing Rust dependencies in repository: {}", repo_root);
    println!();
    
    // Collect all Rust project directories
    let mut rust_dirs: Vec<PathBuf> = Vec::new();
    
    // Walk through all directories and find Rust projects
    for entry in WalkDir::new(repo_path)
        .into_iter()
        .filter_entry(|e| !should_skip_entry(e, repo_path))
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                eprintln!("Warning: Error accessing path: {}", e);
                continue;
            }
        };
        
        if entry.file_type().is_dir() {
            let dir_path = entry.path();
            
            if dir_path.join("Cargo.toml").exists() {
                rust_dirs.push(dir_path.to_path_buf());
            }
        }
    }
    
    // Process all Rust projects
    if rust_dirs.is_empty() {
        println!("No Rust projects found in repository");
        return;
    }
    
    // Check if cargo-edit is installed when --latest flag is used
    if args.latest {
        if check_cargo_edit_installed() {
            println!("✓ cargo-edit is installed, will use cargo upgrade for latest versions");
        } else {
            eprintln!("⚠️  Warning: --latest flag was specified but cargo-edit is not installed.");
            eprintln!("   To install it, run: cargo install cargo-edit");
            eprintln!("   Falling back to standard cargo update (respects version constraints)\n");
        }
    }
    
    for dir_path in rust_dirs {
        println!("[Rust] Found Cargo.toml in {}", dir_path.display());
        
        if args.latest && check_cargo_edit_installed() {
            // First run cargo upgrade to update Cargo.toml to latest versions
            if let Err(e) = run_command_in_directory(&dir_path, &["cargo", "upgrade"]) {
                eprintln!("Warning: Failed to run cargo upgrade: {}", e);
                eprintln!("         Falling back to cargo update");
                if let Err(e) = run_command_in_directory(&dir_path, &["cargo", "update"]) {
                    eprintln!("Warning: {}", e);
                }
            } else {
                // Then run cargo update to update Cargo.lock
                if let Err(e) = run_command_in_directory(&dir_path, &["cargo", "update"]) {
                    eprintln!("Warning: {}", e);
                }
            }
        } else {
            // Standard cargo update (respects version constraints)
            if let Err(e) = run_command_in_directory(&dir_path, &["cargo", "update"]) {
                eprintln!("Warning: {}", e);
            }
        }
    }
    
    println!("\n✓ Rust dependency polishing complete!");
}