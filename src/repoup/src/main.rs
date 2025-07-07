use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, exit};
use walkdir::{DirEntry, WalkDir};

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

fn detect_package_manager(dir: &Path) -> Option<&'static str> {
    if dir.join("pnpm-lock.yaml").exists() {
        Some("pnpm")
    } else if dir.join("package-lock.json").exists() {
        Some("npm")
    } else if dir.join("yarn.lock").exists() {
        Some("yarn")
    } else if dir.join("package.json").exists() {
        // Default to pnpm if no lock file found
        Some("pnpm")
    } else {
        None
    }
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

fn should_skip_entry(entry: &DirEntry) -> bool {
    // Skip any path that has node_modules as a component
    if entry.file_name() == "node_modules" {
        return true;
    }
    
    // Skip git worktree directories
    if entry.file_type().is_dir() && is_git_worktree(entry.path()) {
        println!("Skipping git worktree directory: {}", entry.path().display());
        return true;
    }
    
    false
}

fn main() {
    let repo_root = match find_git_repo() {
        Some(root) => root,
        None => {
            eprintln!("Error: Could not find git repository");
            exit(1);
        }
    };
    
    let repo_path = Path::new(&repo_root);
    
    println!("Updating dependencies in repository: {}", repo_root);
    println!("This will update Rust, Node.js, and Go projects...\n");
    
    // Collect all project directories by type
    let mut rust_dirs: Vec<PathBuf> = Vec::new();
    let mut node_dirs: Vec<(PathBuf, &str)> = Vec::new();
    let mut go_dirs: Vec<PathBuf> = Vec::new();
    
    // Collection phase - walk through all directories and categorize
    for entry in WalkDir::new(repo_path)
        .into_iter()
        .filter_entry(|e| !should_skip_entry(e))
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
            
            // Categorize directories by project type
            if dir_path.join("Cargo.toml").exists() {
                rust_dirs.push(dir_path.to_path_buf());
            }
            
            if let Some(pm) = detect_package_manager(dir_path) {
                node_dirs.push((dir_path.to_path_buf(), pm));
            }
            
            if dir_path.join("go.mod").exists() {
                go_dirs.push(dir_path.to_path_buf());
            }
        }
    }
    
    // Processing phase - handle each language type globally
    
    // Process all Rust projects first
    for dir_path in rust_dirs {
        println!("\n[Rust] Found Cargo.toml in {}", dir_path.display());
        if let Err(e) = run_command_in_directory(&dir_path, &["cargo", "update"]) {
            eprintln!("Warning: {}", e);
        }
    }
    
    // Process all Node.js projects second
    for (dir_path, pm) in node_dirs {
        println!("\n[Node] Found package.json in {} (using {})", dir_path.display(), pm);
        let cmd = match pm {
            "pnpm" => vec!["pnpm", "update"],
            "yarn" => vec!["yarn", "upgrade"],
            _ => vec!["npm", "update"],
        };
        if let Err(e) = run_command_in_directory(&dir_path, &cmd) {
            eprintln!("Warning: {}", e);
        }
    }
    
    // Process all Go projects last
    for dir_path in go_dirs {
        println!("\n[Go] Found go.mod in {}", dir_path.display());
        if let Err(e) = run_command_in_directory(&dir_path, &["go", "get", "-u", "all"]) {
            eprintln!("Warning: {}", e);
        }
    }
    
    println!("\n✓ Dependency update complete!");
}