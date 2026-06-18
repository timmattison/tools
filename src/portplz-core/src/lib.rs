//! Core port-derivation primitives shared by `portplz` and related tools.
//!
//! This crate provides the stable hashing primitive used to turn an arbitrary
//! string into a deterministic, guaranteed-unprivileged TCP port number.

use sha2::{Digest, Sha256};
use std::path::Path;

/// A TCP port guaranteed to be unprivileged (always `>= 1024`).
///
/// Construct one only via [`unprivileged_port_from_string`], which enforces the
/// unprivileged invariant. The inner value is private and there is intentionally
/// no `Display` implementation, so callers must go through [`DerivedPort::get`]
/// to obtain the raw `u16` — keeping the invariant impossible to bypass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DerivedPort(u16);

impl DerivedPort {
    /// Returns the underlying port number, which is always `>= 1024`.
    #[must_use]
    pub fn get(&self) -> u16 {
        self.0
    }
}

/// Derives a deterministic, unprivileged port from an arbitrary input string.
///
/// The same input always yields the same port, and the result is always
/// `>= 1024` (i.e. never a privileged port).
#[must_use]
pub fn unprivileged_port_from_string(input: &str) -> DerivedPort {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();

    // `result` is a fixed 32-byte SHA-256 digest, so indexing the first two
    // bytes directly is always in bounds (and avoids the `string_slice` lint).
    let mut port = u16::from_be_bytes([result[0], result[1]]);

    while port < 1024 {
        port += 1024;
        port %= 65535;
    }

    DerivedPort(port)
}

/// Describes how the port's hash input was determined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortSource {
    /// Git repo with a branch: hash input is `"repo_name\nbranch"`.
    /// `\n` separator: git branch names cannot contain newlines, so the hash
    /// input is unambiguous even when names contain `@` etc.
    GitRepo { repo_name: String, branch: String },
    /// Git repo with detached HEAD: hash input is just `repo_name`.
    DetachedHead { repo_name: String },
    /// No git repo (--no-git or not a repo): hash input is `dirname`.
    Directory { dirname: String },
}

impl PortSource {
    #[must_use]
    pub fn hash_input(&self) -> String {
        match self {
            Self::GitRepo { repo_name, branch } => format!("{repo_name}\n{branch}"),
            Self::DetachedHead { repo_name } => repo_name.clone(),
            Self::Directory { dirname } => dirname.clone(),
        }
    }

    /// Human-readable one-line description, e.g. `Port 1234 for repo 'x' on branch 'y'`.
    #[must_use]
    pub fn describe(&self, port: DerivedPort) -> String {
        let desc = match self {
            Self::GitRepo { repo_name, branch } => {
                format!("repo '{repo_name}' on branch '{branch}'")
            }
            Self::DetachedHead { repo_name } => format!("repo '{repo_name}' (detached HEAD)"),
            Self::Directory { dirname } => format!("directory '{dirname}' (no git repo)"),
        };
        format!("Port {} for {desc}", port.get())
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

/// The result of deriving a port: the port and how it was derived.
#[derive(Debug, Clone)]
pub struct Derivation {
    pub port: DerivedPort,
    pub source: PortSource,
}

/// Errors that can occur while deriving a port.
#[derive(Debug, thiserror::Error)]
pub enum DeriveError {
    #[error("invalid path: no basename")]
    NoBasename,
}

/// Derives the port for `path`.
///
/// When `no_git` is true, or `path` is not inside a git repo, the directory
/// basename is used; otherwise the repo-root name plus the current branch
/// (detached HEAD falls back to just the repo-root name).
///
/// # Errors
/// Returns [`DeriveError::NoBasename`] if `path` has no final path component.
pub fn derive(_path: &Path, _no_git: bool) -> Result<Derivation, DeriveError> {
    // STUB: ignores inputs, always returns a fixed Directory derivation.
    Ok(Derivation {
        port: unprivileged_port_from_string("STUB"),
        source: PortSource::Directory {
            dirname: "STUB".into(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_generation() {
        let port = unprivileged_port_from_string("test");
        assert!(port.get() >= 1024);
        assert!(port.get() < 65535);
    }

    #[test]
    fn test_consistent_port() {
        assert_eq!(
            unprivileged_port_from_string("example").get(),
            unprivileged_port_from_string("example").get()
        );
    }

    #[test]
    fn test_different_inputs() {
        assert_ne!(
            unprivileged_port_from_string("branch-a").get(),
            unprivileged_port_from_string("branch-b").get()
        );
    }

    #[test]
    fn test_different_repos_same_branch_different_ports() {
        let source_a = PortSource::GitRepo {
            repo_name: "project-a".into(),
            branch: "main".into(),
        };
        let source_b = PortSource::GitRepo {
            repo_name: "project-b".into(),
            branch: "main".into(),
        };
        assert_ne!(
            unprivileged_port_from_string(&source_a.hash_input()).get(),
            unprivileged_port_from_string(&source_b.hash_input()).get(),
        );
    }

    #[test]
    fn test_port_source_git_repo_hash_input() {
        let source = PortSource::GitRepo {
            repo_name: "myproject".into(),
            branch: "main".into(),
        };
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
    fn test_port_source_describe_git_repo() {
        let source = PortSource::GitRepo {
            repo_name: "myproject".into(),
            branch: "main".into(),
        };
        let port = unprivileged_port_from_string(&source.hash_input());
        assert_eq!(
            source.describe(port),
            format!("Port {} for repo 'myproject' on branch 'main'", port.get())
        );
    }

    #[test]
    fn test_port_source_describe_detached() {
        let source = PortSource::DetachedHead {
            repo_name: "myproject".into(),
        };
        let port = unprivileged_port_from_string(&source.hash_input());
        assert_eq!(
            source.describe(port),
            format!("Port {} for repo 'myproject' (detached HEAD)", port.get())
        );
    }

    #[test]
    fn test_port_source_describe_directory() {
        let source = PortSource::Directory {
            dirname: "some-dir".into(),
        };
        let port = unprivileged_port_from_string(&source.hash_input());
        assert_eq!(
            source.describe(port),
            format!("Port {} for directory 'some-dir' (no git repo)", port.get())
        );
    }

    #[test]
    fn test_separator_prevents_cross_component_collision() {
        let source_1 = PortSource::GitRepo {
            repo_name: "a@b".into(),
            branch: "c".into(),
        };
        let source_2 = PortSource::GitRepo {
            repo_name: "a".into(),
            branch: "b@c".into(),
        };
        assert_ne!(source_1.hash_input(), source_2.hash_input());
    }

    #[test]
    fn test_get_repo_root_name_returns_valid_basename() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo = gix::discover(path).expect("Should find git repo");
        let name = get_repo_root_name(&repo);
        assert!(
            name.is_some(),
            "Should find repo root name for a valid repo"
        );
        let name = name.unwrap();
        assert!(!name.is_empty(), "Repo root name should not be empty");
        assert!(!name.contains('/'), "Should be a basename, not a path");
        assert!(!name.contains('\\'), "Should be a basename, not a path");
    }

    #[test]
    fn test_worktree_and_main_repo_share_root_name() {
        // Discover repo from the current path (may be a worktree)
        let worktree_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let worktree_repo = gix::discover(worktree_path).expect("Should find repo");
        let worktree_name = get_repo_root_name(&worktree_repo).expect("Should get repo root name");

        // Discover repo from the main repo root (parent of common_dir)
        let common = std::fs::canonicalize(worktree_repo.common_dir()).unwrap();
        let main_repo_root = common.parent().unwrap();
        let main_repo = gix::discover(main_repo_root).expect("Should find main repo");
        let main_name = get_repo_root_name(&main_repo).expect("Should get main repo root name");

        assert_eq!(
            worktree_name, main_name,
            "get_repo_root_name should return the same name from both worktree and main repo"
        );
    }

    #[test]
    fn test_derive_on_git_init_repo_is_gitrepo_and_stable() {
        fn run_git(dir: &std::path::Path, args: &[&str]) {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .status()
                .expect("invoke git");
            assert!(status.success(), "git {args:?} failed");
        }

        let tmp = tempfile::tempdir().expect("create temp dir");
        let dir = tmp.path();
        run_git(dir, &["init", "-b", "testbranch"]);
        // An empty commit so HEAD is born and the branch is reported deterministically.
        run_git(
            dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@e",
                "commit",
                "--allow-empty",
                "-m",
                "init",
            ],
        );

        let d1 = derive(dir, false).expect("derive should succeed");
        let d2 = derive(dir, false).expect("derive should succeed");
        assert_eq!(
            d1.port.get(),
            d2.port.get(),
            "derived port must be stable across calls"
        );
        match &d1.source {
            PortSource::GitRepo { branch, .. } => assert_eq!(branch, "testbranch"),
            other => panic!("expected GitRepo source, got {other:?}"),
        }
    }
}
