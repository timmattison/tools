use std::env;
use std::fs;
use std::path::Path;
use std::process::exit;
use walkdir::WalkDir;
use clap::Parser;

#[derive(Parser)]
#[command(name = "nodenuke")]
#[command(about = "Remove node_modules directories and lock files")]
struct Cli {
    #[arg(long, help = "Don't go to the git repository root before running")]
    no_root: bool,
}

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

fn main() {
    let cli = Cli::parse();
    
    let target_dirs = vec!["node_modules", ".next"];
    let target_files = vec!["pnpm-lock.yaml", "package-lock.json"];
    
    let start_dir = if cli.no_root {
        env::current_dir().unwrap_or_else(|e| {
            eprintln!("Error getting current directory: {}", e);
            exit(1);
        })
    } else {
        match find_git_repo() {
            Some(repo_root) => {
                println!("Found git repository, changing to root: {}", repo_root);
                Path::new(&repo_root).to_path_buf()
            }
            None => {
                env::current_dir().unwrap_or_else(|e| {
                    eprintln!("Error getting current directory: {}", e);
                    exit(1);
                })
            }
        }
    };
    
    println!("Starting to scan from: {}", start_dir.display());
    println!("Will delete directories: {:?}", target_dirs);
    println!("Will delete files: {:?}", target_files);
    
    for entry in WalkDir::new(&start_dir) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                eprintln!("Error accessing {}: {}", e.path().unwrap_or(Path::new("unknown")).display(), e);
                continue;
            }
        };
        
        let entry_name = entry.file_name().to_string_lossy();
        
        // Check for target directories
        if entry.file_type().is_dir() {
            if target_dirs.contains(&entry_name.as_ref()) {
                println!("Removing directory: {}", entry.path().display());
                if let Err(e) = fs::remove_dir_all(entry.path()) {
                    eprintln!("Error removing {}: {}", entry.path().display(), e);
                }
            }
        }
        
        // Check for target files
        if entry.file_type().is_file() {
            if target_files.contains(&entry_name.as_ref()) {
                println!("Removing file: {}", entry.path().display());
                if let Err(e) = fs::remove_file(entry.path()) {
                    eprintln!("Error removing {}: {}", entry.path().display(), e);
                }
            }
        }
    }
    
    println!("Cleanup complete!");
}