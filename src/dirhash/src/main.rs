use anyhow::{Context, Result};
use clap::Parser;
use rayon::prelude::*;
use sha2::{Sha256, Sha512, Digest};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use walkdir::WalkDir;

#[derive(Parser)]
#[command(name = "dirhash")]
#[command(about = "Calculate a hash of all files in a directory")]
#[command(long_about = "Calculates SHA-512 hash for each file, then creates a final SHA-256 hash from sorted file hashes")]
struct Cli {
    #[arg(help = "Directory to hash")]
    directory: String,
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("Failed to open file: {}", path.display()))?;
    
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
    
    // Collect all file paths and their hashes
    let mut file_hashes: Vec<(String, String)> = WalkDir::new(&cli.directory)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| !entry.file_type().is_dir())
        .par_bridge()  // Parallel processing
        .map(|entry| {
            let path = entry.path();
            let name = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            
            match hash_file(path) {
                Ok(hash) => {
                    println!("{}  {}", hash, name);
                    Some((hash, name))
                }
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
    let concatenated: String = file_hashes
        .into_iter()
        .map(|(hash, _)| hash)
        .collect();
    
    // Calculate final hash
    let final_hash = hash_string(&concatenated);
    println!("{}", final_hash);
    
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