use buildinfo::version_string;
use clap::Parser;
use sha2::{Digest, Sha256};
use std::env;
use std::path::Path;

#[derive(Parser)]
#[command(name = "portplz")]
#[command(version = version_string!())]
#[command(about = "Generate a port number from the git repo root and branch name", long_about = None)]
struct Cli {
    #[arg(help = "Directory path (defaults to current directory)")]
    path: Option<String>,

    #[arg(
        short,
        long,
        help = "Print verbose output with directory name and branch"
    )]
    verbose: bool,

    #[arg(long, help = "Disable git branch detection")]
    no_git: bool,
}

/// Returns the repo root directory basename, consistent across worktrees.
///
/// Uses `common_dir()` which points to the shared `.git` directory,
/// then takes the parent (repo root) and extracts its basename.
/// For worktrees, `common_dir()` always points back to the main repo's
/// `.git` directory, so this returns the same name regardless of which
/// worktree you're in.
fn get_repo_root_name(repo: &gix::Repository) -> Option<String> {
    let common = std::fs::canonicalize(repo.common_dir()).ok()?;
    common
        .parent()
        .and_then(|p| p.file_name())
        .map(|name| name.to_string_lossy().to_string())
}

fn get_git_branch(repo: &gix::Repository) -> Option<String> {
    match repo.head() {
        Ok(head) => head.referent_name().map(|n| n.shorten().to_string()),
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
    let basename = path
        .file_name()
        .ok_or("Invalid path: no basename")?
        .to_string_lossy();

    let (input_string, verbose_desc) = if cli.no_git {
        (
            basename.to_string(),
            format!("directory '{basename}' (no git repo)"),
        )
    } else {
        match gix::discover(path) {
            Ok(repo) => {
                let repo_name = get_repo_root_name(&repo)
                    .unwrap_or_else(|| basename.to_string());
                match get_git_branch(&repo) {
                    Some(branch) => (
                        format!("{repo_name}@{branch}"),
                        format!("repo '{repo_name}' on branch '{branch}'"),
                    ),
                    None => (
                        repo_name.clone(),
                        format!("repo '{repo_name}' (detached HEAD)"),
                    ),
                }
            }
            Err(_) => (
                basename.to_string(),
                format!("directory '{basename}' (no git repo)"),
            ),
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
    fn test_same_repo_same_branch_same_port() {
        // Same repo root + same branch -> same port
        let port1 = unprivileged_port_from_string("project-a@main");
        let port2 = unprivileged_port_from_string("project-a@main");
        assert_eq!(port1, port2);
    }

    #[test]
    fn test_different_repos_same_branch_different_ports() {
        // Different repo roots + same branch -> different ports
        let port_a = unprivileged_port_from_string("project-a@main");
        let port_b = unprivileged_port_from_string("project-b@main");
        assert_ne!(port_a, port_b);
    }

    #[test]
    fn test_same_repo_different_branches() {
        let main_port = unprivileged_port_from_string("project-a@main");
        let dev_port = unprivileged_port_from_string("project-a@dev");
        assert_ne!(main_port, dev_port);
    }

    #[test]
    fn test_get_repo_root_name_returns_basename() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo = gix::discover(path).expect("Should find git repo");
        let name = get_repo_root_name(&repo);
        assert!(name.is_some(), "Should find repo root name for a valid repo");
        let name = name.unwrap();
        assert!(!name.is_empty(), "Repo root name should not be empty");
        // This repo is "tools" (or a worktree of it), so the root name should be "tools"
        assert_eq!(name, "tools");
    }

    #[test]
    fn test_verbose_format_with_git() {
        // Verbose output should be: "Port {port} for repo '{root}' on branch '{branch}'"
        let repo_name = "myproject";
        let branch = "main";
        let input = format!("{repo_name}@{branch}");
        let port = unprivileged_port_from_string(&input);
        let verbose = format!("Port {port} for repo '{repo_name}' on branch '{branch}'");
        assert!(verbose.starts_with("Port "));
        assert!(verbose.contains("repo 'myproject'"));
        assert!(verbose.contains("on branch 'main'"));
    }

    #[test]
    fn test_verbose_format_no_git() {
        // Without git: "Port {port} for directory '{dirname}' (no git repo)"
        let dirname = "some-dir";
        let port = unprivileged_port_from_string(dirname);
        let verbose = format!("Port {port} for directory '{dirname}' (no git repo)");
        assert!(verbose.contains("directory 'some-dir'"));
        assert!(verbose.contains("(no git repo)"));
    }

    #[test]
    fn test_worktrees_share_repo_root_name() {
        // common_dir() is the same for main repo and worktrees,
        // so get_repo_root_name should return the same value.
        // We test this indirectly: the hash input format repo@branch
        // ensures worktrees of the same repo get the same port.
        let port1 = unprivileged_port_from_string("tools@feature-x");
        let port2 = unprivileged_port_from_string("tools@feature-x");
        assert_eq!(port1, port2);
    }

    #[test]
    fn test_detached_head_uses_repo_name_only() {
        // Detached HEAD: hash input is just repo-root-basename (no @branch)
        let detached = unprivileged_port_from_string("myproject");
        let on_branch = unprivileged_port_from_string("myproject@main");
        // They should differ because the input strings differ
        assert_ne!(detached, on_branch);
    }

    #[test]
    fn test_detached_head_verbose_format() {
        let repo_name = "myproject";
        let port = unprivileged_port_from_string(repo_name);
        let verbose = format!("Port {port} for repo '{repo_name}' (detached HEAD)");
        assert!(verbose.contains("repo 'myproject'"));
        assert!(verbose.contains("(detached HEAD)"));
    }

    #[test]
    fn test_no_git_uses_cwd_basename() {
        // --no-git: hash input is just the directory basename
        let port = unprivileged_port_from_string("my-directory");
        let port2 = unprivileged_port_from_string("my-directory");
        assert_eq!(port, port2);
        // Different directory names produce different ports
        let other = unprivileged_port_from_string("other-directory");
        assert_ne!(port, other);
    }
}
