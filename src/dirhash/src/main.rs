use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use ignore::WalkBuilder;
use rayon::prelude::*;
use sha2::{Digest, Sha256, Sha512};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

#[derive(Parser)]
#[command(name = "dirhash")]
#[command(version = version_string!())]
#[command(about = "Calculate a hash of all files in a directory")]
#[command(
    long_about = "Calculates SHA-512 hash for each file, then creates a final SHA-256 hash from sorted file hashes. Respects .gitignore and other ignore files."
)]
struct Cli {
    #[arg(help = "Directory to hash")]
    directory: String,

    #[arg(long, help = "Don't respect ignore files (.gitignore, .ignore, etc.)")]
    no_ignore: bool,

    #[arg(long, help = "Don't respect .gitignore files")]
    no_ignore_vcs: bool,

    #[arg(long, help = "Include hidden files and directories")]
    hidden: bool,
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("Failed to open file: {}", path.display()))?;

    let mut hasher = Sha512::new();
    let mut buffer = [0; 8192];

    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_string(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Track if we're ignoring files
    let ignoring_files = !cli.no_ignore || !cli.no_ignore_vcs;

    // Build the walker with ignore settings
    let mut walker = WalkBuilder::new(&cli.directory);
    walker
        .ignore(!cli.no_ignore)
        .git_ignore(!cli.no_ignore_vcs)
        .git_global(!cli.no_ignore_vcs)
        .git_exclude(!cli.no_ignore_vcs)
        .hidden(!cli.hidden);

    // Also build a walker that doesn't respect ignore files to count ignored files
    let mut all_files_walker = WalkBuilder::new(&cli.directory);
    all_files_walker
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .hidden(!cli.hidden);

    // Collect all file paths and their hashes
    let entries: Vec<_> = walker
        .build()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
        .collect();

    // Count total files if we're ignoring some
    let total_files = if ignoring_files {
        all_files_walker
            .build()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
            .count()
    } else {
        entries.len()
    };

    let processed_files = entries.len();
    let ignored_count = total_files - processed_files;

    let mut file_hashes: Vec<(String, String)> = entries
        .into_par_iter()
        .map(|entry| {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            match hash_file(path) {
                Ok(hash) => Some((hash, name)),
                Err(e) => {
                    eprintln!("Error hashing {}: {}", path.display(), e);
                    None
                }
            }
        })
        .flatten()
        .collect();

    // Sort hashes
    file_hashes.sort_by(|a, b| a.0.cmp(&b.0));

    // Concatenate sorted hashes
    let concatenated: String = file_hashes.into_iter().map(|(hash, _)| hash).collect();

    // Calculate final hash
    let final_hash = hash_string(&concatenated);

    // Print message about ignored files to stderr if any
    if ignoring_files && ignored_count > 0 {
        let mut stderr = io::stderr();
        writeln!(
            stderr,
            "Note: {ignored_count} file(s) ignored. Use --no-ignore to include all files, or --no-ignore-vcs to include files ignored by .gitignore"
        )?;
    }

    // Print only the final hash to stdout
    println!("{final_hash}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_string() {
        // Test with empty string
        let hash = hash_string("");
        assert_eq!(hash.len(), 64); // SHA-256 produces 64 hex characters

        // Test deterministic behavior
        let hash1 = hash_string("test");
        let hash2 = hash_string("test");
        assert_eq!(hash1, hash2);

        // Test different inputs produce different hashes
        let hash3 = hash_string("test2");
        assert_ne!(hash1, hash3);
    }
}
