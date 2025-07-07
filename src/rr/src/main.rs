use std::env;
use std::path::Path;
use std::process::{Command, exit};
use walkdir::WalkDir;
use clap::Parser;

#[derive(Parser)]
#[command(name = "rr")]
#[command(about = "Rust remover - runs cargo clean in all Rust projects")]
struct Cli {
    #[arg(long, help = "Don't go to the git repository root before running")]
    no_root: bool,
    
    #[arg(long, help = "Dry run - show what would be cleaned without actually cleaning")]
    dry_run: bool,
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

fn run_cargo_clean(dir: &Path, dry_run: bool) -> Result<(), std::io::Error> {
    if dry_run {
        println!("Would clean: {}", dir.display());
        return Ok(());
    }
    
    let output = Command::new("cargo")
        .arg("clean")
        .current_dir(dir)
        .output()?;
    
    if !output.status.success() {
        eprintln!("Warning: cargo clean failed in {} - {}", 
                 dir.display(), 
                 String::from_utf8_lossy(&output.stderr).trim());
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "cargo clean failed"));
    }
    
    println!("Cleaned: {}", dir.display());
    Ok(())
}

fn calculate_target_size(dir: &Path) -> u64 {
    let target_dir = dir.join("target");
    if !target_dir.exists() {
        return 0;
    }
    
    let mut total_size = 0u64;
    
    for entry in WalkDir::new(&target_dir) {
        if let Ok(entry) = entry {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_file() {
                    total_size += metadata.len();
                }
            }
        }
    }
    
    total_size
}

fn format_size(size: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = size as f64;
    let mut unit_index = 0;
    
    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }
    
    format!("{:.2} {}", size, UNITS[unit_index])
}

fn main() {
    let cli = Cli::parse();
    
    let start_dir = if cli.no_root {
        env::current_dir().unwrap_or_else(|e| {
            eprintln!("Error getting current directory: {}", e);
            exit(1);
        })
    } else {
        match find_git_repo() {
            Some(repo_root) => {
                println!("Found git repository, starting from root: {}", repo_root);
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
    
    println!("Scanning for Rust projects from: {}", start_dir.display());
    if cli.dry_run {
        println!("DRY RUN MODE - no files will be deleted");
    }
    println!();
    
    let mut total_cleaned = 0;
    let mut total_size_freed = 0u64;
    let mut projects_found = 0;
    let mut total_failed = 0;
    
    for entry in WalkDir::new(&start_dir) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                eprintln!("Warning: Error accessing path: {}", e);
                continue;
            }
        };
        
        if entry.file_type().is_dir() {
            let cargo_toml_path = entry.path().join("Cargo.toml");
            if cargo_toml_path.exists() {
                projects_found += 1;
                let target_size = calculate_target_size(entry.path());
                
                if target_size > 0 {
                    println!("Found Rust project: {} (target size: {})", 
                            entry.path().display(), 
                            format_size(target_size));
                    
                    match run_cargo_clean(entry.path(), cli.dry_run) {
                        Ok(_) => {
                            total_cleaned += 1;
                            total_size_freed += target_size;
                        }
                        Err(_) => {
                            total_failed += 1;
                            eprintln!("  Skipping this project, continuing with others...");
                        }
                    }
                }
            }
        }
    }
    
    println!("\n=== Summary ===");
    println!("Rust projects found: {}", projects_found);
    println!("Projects cleaned: {}", total_cleaned);
    if total_failed > 0 {
        println!("Projects failed: {} (see warnings above)", total_failed);
    }
    println!("Space freed: {}", format_size(total_size_freed));
    
    if cli.dry_run {
        println!("\nThis was a dry run. Use without --dry-run to actually clean projects.");
    }
}