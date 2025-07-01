use std::env;
use std::path::Path;
use std::process::{Command, exit};
use walkdir::WalkDir;

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
    println!("This will update Go, Node.js, and Rust projects...\n");
    
    // Single pass through all directories
    for entry in WalkDir::new(repo_path) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                eprintln!("Warning: Error accessing path: {}", e);
                continue;
            }
        };
        
        if entry.file_type().is_dir() {
            let dir_path = entry.path();
            
            // Skip any path that has node_modules as a component
            if dir_path.components().any(|c| c.as_os_str() == "node_modules") {
                continue;
            }
            
            // Check for Go projects
            if dir_path.join("go.mod").exists() {
                println!("\n[Go] Found go.mod in {}", dir_path.display());
                if let Err(e) = run_command_in_directory(dir_path, &["go", "get", "-u", "all"]) {
                    eprintln!("Warning: {}", e);
                }
            }
            
            // Check for Rust projects
            if dir_path.join("Cargo.toml").exists() {
                println!("\n[Rust] Found Cargo.toml in {}", dir_path.display());
                if let Err(e) = run_command_in_directory(dir_path, &["cargo", "update"]) {
                    eprintln!("Warning: {}", e);
                }
            }
            
            // Check for Node.js projects
            if let Some(pm) = detect_package_manager(dir_path) {
                println!("\n[Node] Found package.json in {} (using {})", dir_path.display(), pm);
                let cmd = match pm {
                    "pnpm" => vec!["pnpm", "update"],
                    "yarn" => vec!["yarn", "upgrade"],
                    _ => vec!["npm", "update"],
                };
                if let Err(e) = run_command_in_directory(dir_path, &cmd) {
                    eprintln!("Warning: {}", e);
                }
            }
        }
    }
    
    println!("\n✓ Dependency update complete!");
}