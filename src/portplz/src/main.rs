use clap::Parser;
use sha2::{Sha256, Digest};
use std::env;
use std::path::Path;

#[derive(Parser)]
#[command(name = "portplz")]
#[command(about = "Generate a port number from the current directory name", long_about = None)]
struct Cli {
    #[arg(help = "Directory path (defaults to current directory)")]
    path: Option<String>,
}

fn unprivileged_port_from_string(input: &str) -> u16 {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    
    let hash_bytes = &result[..2];
    let mut port = u16::from_be_bytes([hash_bytes[0], hash_bytes[1]]);
    
    while port < 1024 {
        port += 1024;
        port %= 65535;
    }
    
    port
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    
    let path = match cli.path {
        Some(p) => p,
        None => env::current_dir()?.to_string_lossy().into_owned(),
    };
    
    let path = Path::new(&path);
    let basename = path.file_name()
        .ok_or("Invalid path: no basename")?
        .to_string_lossy();
    
    let port = unprivileged_port_from_string(&basename);
    
    println!("Port {} for directory '{}'", port, basename);
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_generation() {
        let port = unprivileged_port_from_string("test");
        assert!(port >= 1024);
        assert!(port < 65535);
    }
    
    #[test]
    fn test_consistent_port() {
        let port1 = unprivileged_port_from_string("example");
        let port2 = unprivileged_port_from_string("example");
        assert_eq!(port1, port2);
    }
    
    #[test]
    fn test_different_inputs() {
        let port1 = unprivileged_port_from_string("dir1");
        let port2 = unprivileged_port_from_string("dir2");
        assert_ne!(port1, port2);
    }
}