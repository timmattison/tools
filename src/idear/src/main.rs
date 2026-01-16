use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use walkdir::WalkDir;

#[derive(Parser)]
#[command(name = "idear")]
#[command(version = version_string!())]
#[command(about = "IDEA Reaper - Find directories containing only .idea subdirectories")]
#[command(long_about = "Recursively searches for directories that contain exactly one entry: a .idea directory. This is useful for finding JetBrains IDE project directories that may have been orphaned when you delete a project directory before closing the IDE.")]
struct Cli {
    #[arg(help = "Path to search from (defaults to current directory)")]
    path: Option<String>,
    
    #[arg(short, long, help = "Maximum depth to search")]
    max_depth: Option<usize>,
    
    #[arg(short, long, help = "Delete the directories containing only .idea")]
    delete: bool,
    
    #[arg(long, help = "Dry run - show what would be deleted without actually deleting")]
    dry_run: bool,
    
    #[arg(short, long, help = "Force deletion without confirmation prompt")]
    force: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    
    let search_path = cli.path.as_deref().unwrap_or(".");
    
    let walker = if let Some(depth) = cli.max_depth {
        WalkDir::new(search_path).max_depth(depth)
    } else {
        WalkDir::new(search_path)
    };
    
    let mut found_dirs = Vec::new();
    let mut total_size = 0u64;
    
    for entry in walker {
        let entry = entry?;
        
        if entry.file_type().is_dir() {
            if is_idea_only_directory(entry.path())? {
                if cli.delete || cli.dry_run {
                    let size = calculate_dir_size(entry.path())?;
                    total_size += size;
                    found_dirs.push((entry.path().to_path_buf(), size));
                } else {
                    println!("{}", entry.path().display());
                }
            }
        }
    }
    
    if cli.delete || cli.dry_run {
        if found_dirs.is_empty() {
            println!("No directories found containing only .idea");
            return Ok(());
        }
        
        println!("Found {} directories containing only .idea:", found_dirs.len());
        println!("Total size to be freed: {}", format_size(total_size));
        println!();
        
        for (dir, size) in &found_dirs {
            if cli.dry_run {
                println!("Would delete: {} ({})", dir.display(), format_size(*size));
            } else {
                println!("Will delete: {} ({})", dir.display(), format_size(*size));
            }
        }
        
        if !cli.dry_run && cli.delete {
            if !cli.force && !confirm_deletion()? {
                println!("Deletion cancelled.");
                return Ok(());
            }
            
            println!();
            for (dir, _) in &found_dirs {
                match fs::remove_dir_all(dir) {
                    Ok(_) => println!("Deleted: {}", dir.display()),
                    Err(e) => eprintln!("Error deleting {}: {}", dir.display(), e),
                }
            }
            println!("\nDeletion complete! Freed {}", format_size(total_size));
        }
    }
    
    Ok(())
}

fn is_idea_only_directory(dir_path: &Path) -> Result<bool> {
    let entries: Vec<_> = fs::read_dir(dir_path)?
        .collect::<Result<Vec<_>, _>>()?;
    
    if entries.len() != 1 {
        return Ok(false);
    }
    
    let entry = &entries[0];
    let file_name = entry.file_name();
    let file_type = entry.file_type()?;
    
    Ok(file_name == ".idea" && file_type.is_dir())
}

fn calculate_dir_size(dir_path: &Path) -> Result<u64> {
    let mut total_size = 0u64;
    
    for entry in WalkDir::new(dir_path) {
        if let Ok(entry) = entry {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_file() {
                    total_size += metadata.len();
                }
            }
        }
    }
    
    Ok(total_size)
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

fn confirm_deletion() -> Result<bool> {
    print!("\nAre you sure you want to delete these directories? [y/N] ");
    io::stdout().flush()?;
    
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    
    Ok(input.trim().eq_ignore_ascii_case("y") || input.trim().eq_ignore_ascii_case("yes"))
}