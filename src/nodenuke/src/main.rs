use std::env;
use std::fs;
use std::process::exit;
use clap::Parser;
use repowalker::{find_git_repo, RepoWalker};

#[derive(Parser)]
#[command(name = "nodenuke")]
#[command(about = "Remove node_modules directories and lock files")]
struct Cli {
    #[arg(long, help = "Don't go to the git repository root before running")]
    no_root: bool,
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
                println!("Found git repository, changing to root: {}", repo_root.display());
                repo_root
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
    
    // First pass: Find and remove target directories without respecting gitignore
    // This ensures we always find and delete node_modules/.next even if they're gitignored
    let dir_walker = RepoWalker::new(start_dir.clone())
        .respect_gitignore(false)  // Don't respect gitignore for target directories
        .skip_node_modules(false)  // We want to find and delete them
        .skip_worktrees(true)
        .include_hidden(true);     // Include hidden directories like .next
    
    for entry in dir_walker.walk_with_ignore() {
        let entry_name = entry.file_name().to_string_lossy();
        
        // Check for target directories
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            if target_dirs.contains(&entry_name.as_ref()) {
                println!("Removing directory: {}", entry.path().display());
                if let Err(e) = fs::remove_dir_all(entry.path()) {
                    eprintln!("Error removing {}: {}", entry.path().display(), e);
                }
            }
        }
    }
    
    // Second pass: Find and remove target files, respecting gitignore for other files
    // but still checking everywhere for lock files
    let file_walker = RepoWalker::new(start_dir.clone())
        .respect_gitignore(false)  // Don't respect gitignore to find lock files everywhere
        .skip_node_modules(true)   // Skip node_modules since we just deleted them
        .skip_worktrees(true)
        .include_hidden(true);     // Include hidden directories
    
    for entry in file_walker.walk_with_ignore() {
        let entry_name = entry.file_name().to_string_lossy();
        
        // Check for target files
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
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