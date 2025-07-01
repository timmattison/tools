use std::env;
use std::path::Path;
use std::process::{Command, exit};
use walkdir::WalkDir;
use clap::Parser;

#[derive(Parser)]
#[command(name = "nodeup")]
#[command(about = "Update npm/pnpm packages in directories with package.json")]
struct Cli {
    #[arg(long, help = "Use --latest flag with npm or -L with pnpm")]
    latest: bool,
    
    #[arg(long, help = "Force using npm for all directories")]
    npm: bool,
    
    #[arg(long, help = "Force using pnpm for all directories")]
    pnpm: bool,
    
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

fn format_command(args: &[&str]) -> String {
    args.join(" ")
}

fn main() {
    let cli = Cli::parse();
    
    // Check for conflicting flags
    if cli.npm && cli.pnpm {
        eprintln!("Error: Cannot specify both --npm and --pnpm flags");
        exit(1);
    }
    
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
    
    // Check if there's a pnpm-lock.yaml in the root directory
    let root_has_pnpm_lock = start_dir.join("pnpm-lock.yaml").exists();
    
    println!("Starting to scan from: {}", start_dir.display());
    println!("Will update npm/pnpm packages in directories with package.json");
    
    if cli.npm {
        println!("Forcing npm for all directories");
    } else if cli.pnpm {
        println!("Forcing pnpm for all directories");
    } else if root_has_pnpm_lock {
        println!("Found pnpm-lock.yaml in root directory, preferring pnpm");
    }
    
    if cli.latest {
        println!("Using --latest flag to update to latest versions");
    }
    
    for entry in WalkDir::new(&start_dir) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                eprintln!("Error accessing {}: {}", e.path().unwrap_or(Path::new("unknown")).display(), e);
                continue;
            }
        };
        
        // Skip node_modules directories
        if entry.file_type().is_dir() && entry.file_name() == "node_modules" {
            continue;
        }
        
        // Check for package.json
        if entry.file_type().is_file() && entry.file_name() == "package.json" {
            let dir_path = entry.path().parent().unwrap();
            
            let cmd_args = if cli.npm {
                // Force npm
                if cli.latest {
                    vec!["npm", "update", "--latest"]
                } else {
                    vec!["npm", "update"]
                }
            } else if cli.pnpm {
                // Force pnpm
                if cli.latest {
                    vec!["pnpm", "up", "-L"]
                } else {
                    vec!["pnpm", "up"]
                }
            } else if root_has_pnpm_lock {
                // Root has pnpm-lock.yaml, prefer pnpm
                if cli.latest {
                    vec!["pnpm", "up", "-L"]
                } else {
                    vec!["pnpm", "up"]
                }
            } else {
                // Check for lock files to determine which package manager to use
                let pnpm_lock_path = dir_path.join("pnpm-lock.yaml");
                let npm_lock_path = dir_path.join("package-lock.json");
                
                if pnpm_lock_path.exists() {
                    if cli.latest {
                        vec!["pnpm", "up", "-L"]
                    } else {
                        vec!["pnpm", "up"]
                    }
                } else if npm_lock_path.exists() {
                    if cli.latest {
                        vec!["npm", "update", "--latest"]
                    } else {
                        vec!["npm", "update"]
                    }
                } else {
                    // Default to npm if no lock file is found
                    if cli.latest {
                        vec!["npm", "update", "--latest"]
                    } else {
                        vec!["npm", "update"]
                    }
                }
            };
            
            println!("Running '{}' in {}", format_command(&cmd_args), dir_path.display());
            
            let output = Command::new(cmd_args[0])
                .args(&cmd_args[1..])
                .current_dir(dir_path)
                .output();
            
            match output {
                Ok(output) => {
                    if !output.status.success() {
                        eprintln!("Error executing command in {}: {}", 
                                dir_path.display(), 
                                String::from_utf8_lossy(&output.stderr));
                    }
                }
                Err(e) => {
                    eprintln!("Error executing command in {}: {}", dir_path.display(), e);
                }
            }
        }
    }
    
    println!("Update complete!");
}