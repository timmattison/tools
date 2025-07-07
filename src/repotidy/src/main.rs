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
        eprintln!("Error running {} in {}: {}", 
                 command.join(" "), 
                 dir.display(), 
                 String::from_utf8_lossy(&output.stderr));
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "Command failed"));
    }
    
    println!("Ran {} in {}", command.join(" "), dir.display());
    Ok(())
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
    
    for entry in WalkDir::new(repo_path) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                eprintln!("Warning: Error accessing path: {}", e);
                continue;
            }
        };
        
        if entry.file_type().is_dir() {
            let go_mod_path = entry.path().join("go.mod");
            if go_mod_path.exists() {
                if let Err(e) = run_command_in_directory(entry.path(), &["go", "mod", "tidy"]) {
                    eprintln!("Error running go mod tidy in {}: {}", entry.path().display(), e);
                }
            }
        }
    }
}