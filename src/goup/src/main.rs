use std::path::{Path, PathBuf};
use std::process::{Command, exit};
use buildinfo::version_string;
use clap::Parser;
use repowalker::{find_git_repo, RepoWalker};

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

#[derive(Parser, Debug)]
#[command(author, version = version_string!(), about = "Update Go dependencies in a repository", long_about = None)]
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
    
    println!("Updating Go dependencies in repository: {}", repo_root.display());
    println!();
    
    // Collect all Go project directories
    let mut go_dirs: Vec<PathBuf> = Vec::new();
    
    // Walk through all directories and find Go projects
    let walker = RepoWalker::new(repo_root.clone())
        .respect_gitignore(false)  // Don't respect gitignore - find ALL Go projects
        .include_hidden(true);     // Include hidden directories
    
    for entry in walker.walk_with_ignore() {
        if entry.file_type().map_or(false, |ft| ft.is_dir()) {
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