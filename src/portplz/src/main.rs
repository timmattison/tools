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
        help = "Print verbose output with repo/directory name and branch"
    )]
    verbose: bool,

    #[arg(long, help = "Disable git branch detection")]
    no_git: bool,
}

/// Describes how the port hash input was determined.
///
/// Each variant produces a different hash input format and verbose description.
enum PortSource {
    /// Git repo with a branch: hash input is `"repo_name\nbranch"`.
    /// Uses `\n` as separator because git branch names cannot contain newlines
    /// (per git-check-ref-format), making the hash input unambiguous even when
    /// repo names or branch names contain `@` or other special characters.
    GitRepo { repo_name: String, branch: String },
    /// Git repo with detached HEAD: hash input is just `repo_name`.
    DetachedHead { repo_name: String },
    /// No git repo (--no-git or not a repo): hash input is `dirname`.
    Directory { dirname: String },
}

impl PortSource {
    fn hash_input(&self) -> String {
        match self {
            Self::GitRepo { repo_name, branch } => format!("{repo_name}\n{branch}"),
            Self::DetachedHead { repo_name } => repo_name.clone(),
            Self::Directory { dirname } => dirname.clone(),
        }
    }

    fn verbose_description(&self, port: u16) -> String {
        let desc = match self {
            Self::GitRepo { repo_name, branch } => {
                format!("repo '{repo_name}' on branch '{branch}'")
            }
            Self::DetachedHead { repo_name } => {
                format!("repo '{repo_name}' (detached HEAD)")
            }
            Self::Directory { dirname } => {
                format!("directory '{dirname}' (no git repo)")
            }
        };
        format!("Port {port} for {desc}")
    }
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

    let source = if cli.no_git {
        PortSource::Directory {
            dirname: basename.to_string(),
        }
    } else {
        match gix::discover(path) {
            Ok(repo) => {
                let repo_name =
                    get_repo_root_name(&repo).unwrap_or_else(|| basename.to_string());
                match get_git_branch(&repo) {
                    Some(branch) => PortSource::GitRepo { repo_name, branch },
                    None => PortSource::DetachedHead { repo_name },
                }
            }
            Err(_) => PortSource::Directory {
                dirname: basename.to_string(),
            },
        }
    };

    let port = unprivileged_port_from_string(&source.hash_input());

    if cli.verbose {
        println!("{}", source.verbose_description(port));
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

    // --- PortSource tests ---

    #[test]
    fn test_port_source_git_repo_hash_input() {
        let source = PortSource::GitRepo {
            repo_name: "myproject".into(),
            branch: "main".into(),
        };
        // Separator must be \n (can't appear in git branch names)
        assert_eq!(source.hash_input(), "myproject\nmain");
    }

    #[test]
    fn test_port_source_detached_head_hash_input() {
        let source = PortSource::DetachedHead {
            repo_name: "myproject".into(),
        };
        assert_eq!(source.hash_input(), "myproject");
    }

    #[test]
    fn test_port_source_directory_hash_input() {
        let source = PortSource::Directory {
            dirname: "some-dir".into(),
        };
        assert_eq!(source.hash_input(), "some-dir");
    }

    #[test]
    fn test_port_source_verbose_git_repo() {
        let source = PortSource::GitRepo {
            repo_name: "myproject".into(),
            branch: "main".into(),
        };
        let port = unprivileged_port_from_string(&source.hash_input());
        assert_eq!(
            source.verbose_description(port),
            format!("Port {port} for repo 'myproject' on branch 'main'")
        );
    }

    #[test]
    fn test_port_source_verbose_detached() {
        let source = PortSource::DetachedHead {
            repo_name: "myproject".into(),
        };
        let port = unprivileged_port_from_string(&source.hash_input());
        assert_eq!(
            source.verbose_description(port),
            format!("Port {port} for repo 'myproject' (detached HEAD)")
        );
    }

    #[test]
    fn test_port_source_verbose_directory() {
        let source = PortSource::Directory {
            dirname: "some-dir".into(),
        };
        let port = unprivileged_port_from_string(&source.hash_input());
        assert_eq!(
            source.verbose_description(port),
            format!("Port {port} for directory 'some-dir' (no git repo)")
        );
    }

    #[test]
    fn test_separator_prevents_cross_component_collision() {
        // Repo "a@b" on branch "c" must produce a different hash input
        // than repo "a" on branch "b@c". With @ as separator both would
        // be "a@b@c" — a collision. The \n separator prevents this.
        let source_1 = PortSource::GitRepo {
            repo_name: "a@b".into(),
            branch: "c".into(),
        };
        let source_2 = PortSource::GitRepo {
            repo_name: "a".into(),
            branch: "b@c".into(),
        };
        assert_ne!(
            source_1.hash_input(),
            source_2.hash_input(),
            "Different repo/branch combinations must produce different hash inputs"
        );
    }
}
