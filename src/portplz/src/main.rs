use buildinfo::version_string;
use clap::Parser;
use sha2::{Digest, Sha256};
use std::env;
use std::path::Path;

#[derive(Parser)]
#[command(name = "portplz")]
#[command(version = version_string!())]
#[command(about = "Generate a port number from the current git branch name", long_about = None)]
struct Cli {
    #[arg(help = "Directory path (defaults to current directory)")]
    path: Option<String>,

    #[arg(short, long, help = "Print verbose output with directory name and branch")]
    verbose: bool,

    #[arg(long, help = "Disable git branch detection")]
    no_git: bool,

    #[arg(
        long,
        help = "Include directory name in the hash (dirname@branch)",
        conflicts_with = "no_git"
    )]
    with_dir: bool,
}

fn get_git_branch(path: &Path) -> Option<String> {
    match gix::discover(path) {
        Ok(repo) => match repo.head() {
            Ok(head) => head.referent_name().map(|n| n.shorten().to_string()),
            Err(_) => None,
        },
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

    let path_str = match cli.path {
        Some(p) => p,
        None => env::current_dir()?.to_string_lossy().into_owned(),
    };

    let path = Path::new(&path_str);
    let basename = path.file_name()
        .ok_or("Invalid path: no basename")?
        .to_string_lossy();

    let (input_string, verbose_desc) = if cli.no_git {
        (basename.to_string(), format!("directory '{basename}'"))
    } else {
        match get_git_branch(path) {
            Some(branch) => {
                if cli.with_dir {
                    (
                        format!("{basename}@{branch}"),
                        format!("directory '{basename}' on branch '{branch}'"),
                    )
                } else {
                    let desc = format!("branch '{branch}'");
                    (branch, desc)
                }
            }
            None => (basename.to_string(), format!("directory '{basename}' (no git repo)")),
        }
    };

    let port = unprivileged_port_from_string(&input_string);

    if cli.verbose {
        println!("Port {port} for {verbose_desc}");
    } else {
        println!("{port}");
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
        let port1 = unprivileged_port_from_string("branch-a");
        let port2 = unprivileged_port_from_string("branch-b");
        assert_ne!(port1, port2);
    }

    #[test]
    fn test_branch_only_default() {
        // Default: hash is just the branch name
        let port1 = unprivileged_port_from_string("main");
        let port2 = unprivileged_port_from_string("main");
        assert_eq!(port1, port2);
    }

    #[test]
    fn test_with_dir_format() {
        // --with-dir uses dirname@branch
        let branch_only = unprivileged_port_from_string("main");
        let with_dir = unprivileged_port_from_string("myproject@main");
        assert_ne!(branch_only, with_dir);
    }

    #[test]
    fn test_same_branch_different_dirs() {
        // Same branch but different directory names produce different ports with --with-dir
        let dir_a = unprivileged_port_from_string("repo-a@main");
        let dir_b = unprivileged_port_from_string("repo-b@main");
        assert_ne!(dir_a, dir_b);
    }

    #[test]
    fn test_different_branches() {
        let main_port = unprivileged_port_from_string("main");
        let dev_port = unprivileged_port_from_string("dev");
        assert_ne!(main_port, dev_port);
    }
}
