//! The link of a sidecar in a worktree of the host.
//!
//! A link is a symbolic link at `<worktree root>/<link_dir>/<name>`. It always holds
//! the absolute path of its target.

use std::io;
use std::path::{Path, PathBuf};

use crate::git::Git;

/// What is at the place of a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// Nothing is there.
    Missing,
    /// The link points at the main clone.
    MainClone,
    /// The link points at a worktree of the main clone.
    Worktree {
        /// The branch of that worktree. `None` means a detached `HEAD`.
        branch: Option<String>,
    },
    /// The link points at a path that does not exist.
    Broken {
        /// The target of the link.
        target: PathBuf,
    },
    /// Something else is there. Sidecar does not touch it.
    Foreign(ForeignLink),
}

/// The kinds of thing that sidecar does not touch at the place of a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForeignLink {
    /// A real directory.
    Directory,
    /// A file that is not a link.
    File,
    /// A link to an existing path that is neither the main clone nor a worktree of it.
    OtherTarget(PathBuf),
}

/// Finds out what is at `link`. `main_clone` is the place of the main clone.
#[must_use]
pub fn inspect_link(_git: &Git, _link: &Path, _main_clone: &Path) -> LinkState {
    LinkState::Missing
}

/// Points `link` at `target` in one atomic step. The function makes the parent
/// directory, makes a new symbolic link `<dir>/.<name>.sidecar-link-<pid>-<nanos>`,
/// and renames it over `link`.
///
/// # Errors
///
/// Returns the I/O error of the first step that fails. The function removes its
/// scratch link when the rename fails.
pub fn point_link(_link: &Path, _target: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Makes a unique directory for one test.
    fn scratch() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "sidecar-link-test-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("make the scratch directory");
        dir
    }

    #[test]
    fn point_link_replaces_an_existing_link() {
        let dir = scratch();
        let old = dir.join("old");
        let new = dir.join("new");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        let link = dir.join("data").join("日本語");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&old, &link).unwrap();

        point_link(&link, &new).unwrap();

        assert_eq!(std::fs::read_link(&link).unwrap(), new);
        let names: Vec<_> = std::fs::read_dir(link.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "no scratch link stays: {names:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn point_link_makes_the_parent_directory() {
        let dir = scratch();
        let target = dir.join("target");
        std::fs::create_dir_all(&target).unwrap();
        let link = dir.join("a").join("b").join("name");

        point_link(&link, &target).unwrap();

        assert_eq!(std::fs::read_link(&link).unwrap(), target);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
