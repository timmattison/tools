use std::env;
use std::path::Path;
use std::process::{Command, exit};
use buildinfo::version_string;
use clap::Parser;
use repowalker::{find_git_repo, RepoWalker};

#[derive(Parser)]
#[command(name = "nodeup")]
#[command(version = version_string!())]
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
    
    let walker = RepoWalker::new(start_dir.clone())
        .respect_gitignore(false)  // Don't respect gitignore - find ALL Node.js projects
        .include_hidden(true);     // Include hidden directories
    
    for entry in walker.walk_with_ignore() {
        // Check for directories with package.json
        if entry.file_type().map_or(false, |ft| ft.is_dir()) {
            let dir_path = entry.path();
            
            // Determine package manager to use
            let detected_pm = detect_package_manager(dir_path);
            if detected_pm.is_none() {
                continue; // No package.json in this directory
            }
            
            let pm = if cli.npm {
                "npm"
            } else if cli.pnpm {
                "pnpm"
            } else if root_has_pnpm_lock {
                // Root has pnpm-lock.yaml, prefer pnpm
                "pnpm"
            } else {
                // Use the detected package manager
                detected_pm.unwrap()
            };
            
            let cmd_args = match pm {
                "pnpm" => {
                    if cli.latest {
                        vec!["pnpm", "up", "-L"]
                    } else {
                        vec!["pnpm", "up"]
                    }
                }
                "yarn" => {
                    if cli.latest {
                        vec!["yarn", "upgrade", "--latest"]
                    } else {
                        vec!["yarn", "upgrade"]
                    }
                }
                _ => { // npm or default
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