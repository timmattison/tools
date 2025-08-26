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

#[derive(Parser, Debug)]
#[command(author, version, about = "Update Go dependencies in a repository", long_about = None)]
struct Args {
    /// Update to latest versions (use go get -u)
    #[arg(long, short = 'u')]
    update: bool,
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
    
    println!("Updating Go dependencies in repository: {}", repo_root);
    println!();
    
    // Collect all Go project directories
    let mut go_dirs: Vec<PathBuf> = Vec::new();
    
    // Walk through all directories and find Go projects
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
            
            if dir_path.join("go.mod").exists() {
                go_dirs.push(dir_path.to_path_buf());
            }
        }
    }
    
    // Process all Go projects
    if go_dirs.is_empty() {
        println!("No Go projects found in repository");
        return;
    }
    
    for dir_path in go_dirs {
        println!("[Go] Found go.mod in {}", dir_path.display());
        
        let cmd = if args.update {
            vec!["go", "get", "-u", "all"]
        } else {
            vec!["go", "mod", "tidy"]
        };
        
        if let Err(e) = run_command_in_directory(&dir_path, &cmd) {
            eprintln!("Warning: {}", e);
        }
    }
    
    println!("\n✓ Go dependency update complete!");
}