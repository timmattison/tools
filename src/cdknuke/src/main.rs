use std::env;
use std::fs;
use std::process::exit;
use clap::Parser;
use repowalker::{find_git_repo, RepoWalker};

#[derive(Parser)]
#[command(name = "cdknuke")]
#[command(about = "Remove cdk.out directories from AWS CDK projects")]
struct Cli {
    #[arg(long, help = "Don't go to the git repository root before running")]
    no_root: bool,
    #[arg(long, help = "Include hidden directories in the search")]
    hidden: bool,
}

fn main() {
    let cli = Cli::parse();
    
    let target_dirs = vec!["cdk.out"];
    
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
    
    // Find and remove cdk.out directories without respecting gitignore
    // This ensures we always find and delete cdk.out even if it's gitignored
    let dir_walker = RepoWalker::new(start_dir.clone())
        .respect_gitignore(false)  // Don't respect gitignore for target directories
        .skip_node_modules(true)   // Skip node_modules to avoid unnecessary traversal
        .skip_worktrees(true)
        .include_hidden(cli.hidden);  // Only traverse hidden dirs if --hidden flag is set
    
    let mut found_any = false;
    
    for entry in dir_walker.walk_with_ignore() {
        let entry_name = entry.file_name().to_string_lossy();
        
        // Check for target directories
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            if target_dirs.contains(&entry_name.as_ref()) {
                found_any = true;
                println!("Removing directory: {}", entry.path().display());
                if let Err(e) = fs::remove_dir_all(entry.path()) {
                    eprintln!("Error removing {}: {}", entry.path().display(), e);
                }
            }
        }
    }
    
    // Also check for cdk.out at the top level even without --hidden
    // (in case it's a hidden directory for some reason)
    if !cli.hidden {
        for target_dir in &target_dirs {
            if target_dir.starts_with('.') {
                let target_path = start_dir.join(target_dir);
                if target_path.is_dir() {
                    found_any = true;
                    println!("Removing directory: {}", target_path.display());
                    if let Err(e) = fs::remove_dir_all(&target_path) {
                        eprintln!("Error removing {}: {}", target_path.display(), e);
                    }
                }
            }
        }
    }
    
    if found_any {
        println!("Cleanup complete!");
    } else {
        println!("No cdk.out directories found.");
    }
}