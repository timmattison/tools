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

/// Environment variable that overrides the detected user.
///
/// When set to a non-negative integer it replaces the live user id in the port
/// derivation. This lets you reproduce another user's port, or pin a stable
/// port in containers/CI where the uid differs from your workstation.
pub const PORTPLZ_UID_ENV: &str = "PORTPLZ_UID";

/// Identifies the current user so two people on the same machine derive
/// different ports for the same repo and branch.
///
/// On Unix the identity is the numeric POSIX user id; on platforms without one
/// (e.g. Windows) it falls back to the login name. Use [`UserSalt::current`] for
/// the live value, or construct a fixed [`UserSalt::Uid`]/[`UserSalt::Name`] in
/// tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserSalt {
    /// A numeric POSIX user id (the common case on Unix).
    Uid(u32),
    /// A login name, used where no numeric uid exists (e.g. Windows).
    Name(String),
}

impl UserSalt {
    /// Resolves the current user.
    ///
    /// Honors the [`PORTPLZ_UID_ENV`] override first; otherwise uses the live
    /// POSIX uid on Unix, or the login name on platforms without one.
    #[must_use]
    pub fn current() -> Self {
        if let Some(uid) = std::env::var(PORTPLZ_UID_ENV)
            .ok()
            .and_then(|raw| raw.trim().parse::<u32>().ok())
        {
            return Self::Uid(uid);
        }

        #[cfg(unix)]
        {
            // SAFETY: `getuid` is a POSIX call that always succeeds, has no
            // preconditions, and can neither fail nor invoke undefined behavior.
            Self::Uid(unsafe { libc::getuid() })
        }
        #[cfg(not(unix))]
        {
            Self::Name(
                std::env::var("USERNAME")
                    .or_else(|_| std::env::var("USER"))
                    .unwrap_or_else(|_| "unknown".to_string()),
            )
        }
    }

    /// The component mixed into the port hash to distinguish users.
    ///
    /// A uid renders as its decimal digits (no separators), which contain no
    /// newline, so prefixing it to the location's hash input keeps the
    /// user/location boundary unambiguous. For the `Name` variant, newline
    /// characters (`\n` and `\r`) are stripped from the login name for the same
    /// reason: the derived-port hash input uses `\n` as the boundary between the
    /// user and location components, so a newline inside the name would make
    /// that boundary ambiguous and let two distinct (user, location) pairs
    /// collide onto the same port.
    fn hash_component(&self) -> String {
        match self {
            Self::Uid(uid) => uid.to_string(),
            Self::Name(name) => name.chars().filter(|c| *c != '\n' && *c != '\r').collect(),
        }
    }

    /// Human-readable label appended to `--verbose` output, e.g. `uid 501`
    /// or `user 'alice'`.
    fn label(&self) -> String {
        match self {
            Self::Uid(uid) => format!("uid {uid}"),
            Self::Name(name) => format!("user '{name}'"),
        }
    }
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

/// The result of deriving a port: the port, how it was derived, and for whom.
#[derive(Debug, Clone)]
pub struct Derivation {
    pub port: DerivedPort,
    pub source: PortSource,
    pub user: UserSalt,
}

impl Derivation {
    /// One-line human-readable description including the user, e.g.
    /// `Port 51877 for repo 'foo' on branch 'main' (uid 501)`.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{} ({})",
            self.source.describe(self.port),
            self.user.label()
        )
    }
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
/// (detached HEAD falls back to just the repo-root name). `user` is mixed into
/// the hash so different users derive different ports for the same location.
///
/// # Errors
/// Returns [`DeriveError::NoBasename`] if `path` has no final path component.
pub fn derive(path: &Path, no_git: bool, user: &UserSalt) -> Result<Derivation, DeriveError> {
    let basename = path
        .file_name()
        .ok_or(DeriveError::NoBasename)?
        .to_string_lossy()
        .into_owned();

    let source = if no_git {
        PortSource::Directory { dirname: basename }
    } else {
        match gix::discover(path) {
            Ok(repo) => {
                let repo_name = get_repo_root_name(&repo).unwrap_or(basename);
                match get_git_branch(&repo) {
                    Some(branch) => PortSource::GitRepo { repo_name, branch },
                    None => PortSource::DetachedHead { repo_name },
                }
            }
            Err(_) => PortSource::Directory { dirname: basename },
        }
    };

    let hash_input = format!("{}\n{}", user.hash_component(), source.hash_input());
    let port = unprivileged_port_from_string(&hash_input);
    Ok(Derivation {
        port,
        source,
        user: user.clone(),
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
    fn test_different_users_get_different_ports() {
        let path = std::path::Path::new("/example/myrepo");
        let a = derive(path, true, &UserSalt::Uid(501)).expect("derive");
        let b = derive(path, true, &UserSalt::Uid(502)).expect("derive");
        assert_ne!(
            a.port.get(),
            b.port.get(),
            "different users must derive different ports for the same location"
        );
    }

    #[test]
    fn test_describe_includes_uid_label() {
        let path = std::path::Path::new("/example/myrepo");
        let d = derive(path, true, &UserSalt::Uid(501)).expect("derive");
        assert!(
            d.describe().contains("(uid 501)"),
            "verbose description must include the uid, got: {}",
            d.describe()
        );
    }

    #[test]
    fn test_name_hash_component_strips_newlines() {
        // A name containing newlines must not leak them into the hash component,
        // or the `\n` boundary between the user and location components becomes
        // ambiguous and two distinct (user, location) pairs could collide.
        let component = UserSalt::Name("a\nb\rc".into()).hash_component();
        assert!(
            !component.contains('\n'),
            "Name hash component must not contain a newline, got: {component:?}"
        );
        assert!(
            !component.contains('\r'),
            "Name hash component must not contain a carriage return, got: {component:?}"
        );

        // A newline-free name must pass through unchanged (no over-stripping).
        assert_eq!(
            UserSalt::Name("alice".into()).hash_component(),
            "alice",
            "a name without newlines must be unchanged"
        );
    }

    #[test]
    fn test_describe_includes_name_label() {
        let path = std::path::Path::new("/example/myrepo");
        let d = derive(path, true, &UserSalt::Name("alice".into())).expect("derive");
        assert!(
            d.describe().contains("(user 'alice')"),
            "verbose description must include the login name, got: {}",
            d.describe()
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

        let d1 = derive(dir, false, &UserSalt::Uid(501)).expect("derive should succeed");
        let d2 = derive(dir, false, &UserSalt::Uid(501)).expect("derive should succeed");
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
