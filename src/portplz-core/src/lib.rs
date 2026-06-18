//! Core port-derivation primitives shared by `portplz` and related tools.
//!
//! This crate provides the stable hashing primitive used to turn an arbitrary
//! string into a deterministic, guaranteed-unprivileged TCP port number.

use sha2::{Digest, Sha256};

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
        let _ = self;
        String::new()
    }

    /// Human-readable one-line description, e.g. `Port 1234 for repo 'x' on branch 'y'`.
    #[must_use]
    pub fn describe(&self, port: DerivedPort) -> String {
        let _ = (self, port);
        String::new()
    }
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
}
