use clap::Parser;
use sha2::{Sha256, Digest};
use std::env;
use std::path::Path;

#[derive(Parser)]
#[command(name = "portplz")]
#[command(about = "Generate a port number from the current directory name and git branch", long_about = None)]
struct Cli {
    #[arg(help = "Directory path (defaults to current directory)")]
    path: Option<String>,
    
    #[arg(short, long, help = "Print verbose output with directory name and branch")]
    verbose: bool,
    
    #[arg(long, help = "Disable git branch detection")]
    no_git: bool,
}

fn get_git_branch(path: &Path) -> Option<String> {
    match gix::discover(path) {
        Ok(repo) => {
            match repo.head() {
                Ok(head) => {
                    match head.referent_name() {
                        Some(name) => {
                            let branch_name = name.shorten();
                            Some(branch_name.to_string())
                        }
                        None => None,
                    }
                }
                Err(_) => None,
            }
        }
        Err(_) => None,
    }
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
    
    let input_string = if cli.no_git {
        basename.to_string()
    } else {
        match get_git_branch(&path) {
            Some(branch) => format!("{}@{}", basename, branch),
            None => basename.to_string(),
        }
    };
    
    let port = unprivileged_port_from_string(&input_string);
    
    if cli.verbose {
        if cli.no_git {
            println!("Port {} for directory '{}'", port, basename);
        } else {
            match get_git_branch(&path) {
                Some(branch) => println!("Port {} for directory '{}' on branch '{}'", port, basename, branch),
                None => println!("Port {} for directory '{}' (no git repo)", port, basename),
            }
        }
    } else {
        println!("{}", port);
    }
    
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
    
    #[test]
    fn test_combined_directory_and_branch() {
        let port1 = unprivileged_port_from_string("myproject@main");
        let port2 = unprivileged_port_from_string("myproject@feature");
        let port3 = unprivileged_port_from_string("myproject");
        
        assert_ne!(port1, port2);
        assert_ne!(port1, port3);
        assert_ne!(port2, port3);
    }
    
    #[test]
    fn test_branch_formatting() {
        let port1 = unprivileged_port_from_string("test@main");
        let port2 = unprivileged_port_from_string("test@main");
        assert_eq!(port1, port2);
    }
    
    #[test]
    fn test_same_directory_different_branches() {
        let dir_only = unprivileged_port_from_string("project");
        let with_main = unprivileged_port_from_string("project@main");
        let with_dev = unprivileged_port_from_string("project@dev");
        
        assert_ne!(dir_only, with_main);
        assert_ne!(dir_only, with_dev);
        assert_ne!(with_main, with_dev);
    }
}