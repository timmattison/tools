use std::env;
use std::path::Path;
use std::process::exit;
use walkdir::WalkDir;
use num_format::{Locale, ToFormattedString};

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

fn calculate_dir_size(dir_path: &Path) -> Result<u64, std::io::Error> {
    let mut total_size = 0u64;
    
    for entry in WalkDir::new(dir_path) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                eprintln!("Warning: Error accessing path: {}", e);
                continue;
            }
        };
        
        if entry.file_type().is_file() {
            // Check if it's a symlink to avoid double counting
            if let Ok(metadata) = entry.path().symlink_metadata() {
                if metadata.file_type().is_symlink() {
                    continue; // Skip symlinks
                }
                total_size += metadata.len();
            }
        }
    }
    
    Ok(total_size)
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
    
    match calculate_dir_size(repo_path) {
        Ok(total_size) => {
            let formatted_size = total_size.to_formatted_string(&Locale::en);
            println!("Git repo size: {} bytes, path: {}", formatted_size, repo_root);
        }
        Err(e) => {
            eprintln!("Error calculating repository size: {}", e);
            exit(1);
        }
    }
}