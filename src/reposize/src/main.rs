use std::process::exit;
use num_format::{Locale, ToFormattedString};
use repowalker::{find_git_repo, RepoWalker};

fn calculate_dir_size(repo_root: std::path::PathBuf) -> Result<u64, std::io::Error> {
    let mut total_size = 0u64;
    
    let walker = RepoWalker::new(repo_root.clone())
        .respect_gitignore(true)
        .skip_node_modules(true)
        .skip_worktrees(true);
    
    for entry in walker.walk_with_ignore() {
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
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
    
    match calculate_dir_size(repo_root.clone()) {
        Ok(total_size) => {
            let formatted_size = total_size.to_formatted_string(&Locale::en);
            println!("Git repo size: {} bytes, path: {}", formatted_size, repo_root.display());
        }
        Err(e) => {
            eprintln!("Error calculating repository size: {}", e);
            exit(1);
        }
    }
}