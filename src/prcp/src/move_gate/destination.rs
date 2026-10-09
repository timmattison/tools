//! The destination snapshot: what a destination tree holds at one moment, and how two moments differ.
//!
//! # Why
//!
//! The Blake3 check proves each copy at the moment that prcp makes it. It
//! cannot see a person or a program that works in the destination later in
//! the run: a file that appears, a file that changes, or a file that moves
//! away. The gate thus takes a snapshot of each destination tree before the
//! first copy, and walks the tree again after the last copy.
//!
//! # Rules
//!
//! The walk never follows a symlink, as the walk of the plan does not. A root
//! that does not exist gives an empty tree. A node that leaves between the
//! read of its directory and the read of its metadata counts as gone, not as
//! an error.
//!
//! Two snapshots of one node agree when the kind and the identity agree. A
//! file must also keep its size and its modification time, and a symlink its
//! target. A directory may change its own size and time, because both change
//! each time that a name inside it changes, and the walk sees each name.
//!
//! The comparison leaves out the paths that prcp writes. The records of the
//! copy loop judge those paths.

use super::{FileStamp, NodeIdentity};
use crate::plan::{node_kind, NodeKind};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// One node of a destination tree at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DestinationNode {
    kind: NodeKind,
    stamp: FileStamp,
}

impl DestinationNode {
    /// Make the node at `path` from metadata that does not follow a symlink.
    fn from_metadata(path: &Path, metadata: &fs::Metadata) -> io::Result<Self> {
        Ok(Self {
            kind: node_kind(path, metadata.file_type())?,
            stamp: FileStamp::from_metadata(metadata),
        })
    }

    /// Return the identity of the node.
    pub(super) fn identity(&self) -> Option<NodeIdentity> {
        self.stamp.identity
    }

    /// Return true when `other` is the same node in the same state. See the module docs.
    fn agrees_with(&self, other: &Self) -> bool {
        if self.kind != other.kind || self.stamp.identity != other.stamp.identity {
            return false;
        }
        match self.kind {
            NodeKind::File => self.stamp == other.stamp,
            NodeKind::Directory | NodeKind::Symlink { .. } | NodeKind::Special(_) => true,
        }
    }
}

/// One way in which a destination tree differs from an earlier snapshot of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TreeChange {
    /// A node that the earlier snapshot did not hold.
    New {
        /// The path of the node.
        path: PathBuf,
        /// The identity of the node now.
        identity: Option<NodeIdentity>,
    },
    /// A node of the earlier snapshot with another kind, identity, or state now.
    Changed {
        /// The path of the node.
        path: PathBuf,
    },
    /// A node of the earlier snapshot that is gone now.
    Missing {
        /// The path of the node.
        path: PathBuf,
        /// The identity that the node had in the earlier snapshot.
        identity: Option<NodeIdentity>,
    },
    /// A part that the walk cannot read now, and that it could read before.
    Unreadable {
        /// The path that failed.
        path: PathBuf,
        /// The text of the error.
        message: String,
    },
}

/// Every node under one destination root at one moment, plus the parts that the walk could not read.
#[derive(Debug, Clone, Default)]
pub(super) struct DestinationTree {
    nodes: BTreeMap<PathBuf, DestinationNode>,
    errors: BTreeMap<PathBuf, String>,
}

impl DestinationTree {
    /// Walk `root` now. See the module docs for the rules.
    pub(super) fn take(root: &Path) -> Self {
        let mut tree = Self::default();
        let walk = WalkDir::new(root)
            .follow_links(false)
            .follow_root_links(false)
            .sort_by_file_name();
        for result in walk {
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    let gone = error
                        .io_error()
                        .is_some_and(|io| io.kind() == io::ErrorKind::NotFound);
                    if !gone {
                        let path = error.path().unwrap_or(root).to_path_buf();
                        tree.errors.insert(path, error.to_string());
                    }
                    continue;
                }
            };
            let node = entry
                .metadata()
                .map_err(io::Error::from)
                .and_then(|metadata| DestinationNode::from_metadata(entry.path(), &metadata));
            match node {
                Ok(node) => {
                    tree.nodes.insert(entry.into_path(), node);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    tree.errors.insert(entry.into_path(), error.to_string());
                }
            }
        }
        tree
    }

    /// Compare this later snapshot with `before`, and return each difference in path order.
    ///
    /// The paths in `written` are left out, because prcp writes them. A node
    /// under a part that the walk cannot read now is not reported as gone.
    pub(super) fn changes_since(
        &self,
        before: &Self,
        written: &BTreeSet<PathBuf>,
    ) -> Vec<TreeChange> {
        let mut changes = Vec::new();
        for (path, message) in &self.errors {
            if !before.errors.contains_key(path) {
                changes.push(TreeChange::Unreadable {
                    path: path.clone(),
                    message: message.clone(),
                });
            }
        }
        for (path, node) in &self.nodes {
            if written.contains(path) {
                continue;
            }
            match before.nodes.get(path) {
                None => changes.push(TreeChange::New {
                    path: path.clone(),
                    identity: node.identity(),
                }),
                Some(old) if !old.agrees_with(node) => {
                    changes.push(TreeChange::Changed { path: path.clone() });
                }
                Some(_) => {}
            }
        }
        for (path, old) in &before.nodes {
            if written.contains(path) || self.nodes.contains_key(path) || self.hides(path) {
                continue;
            }
            changes.push(TreeChange::Missing {
                path: path.clone(),
                identity: old.identity(),
            });
        }
        changes.sort_by(|a, b| a.path().cmp(b.path()));
        changes
    }

    /// Return true when a part that the walk cannot read now holds `path`.
    fn hides(&self, path: &Path) -> bool {
        self.errors
            .keys()
            .any(|unreadable| path != unreadable && path.starts_with(unreadable))
    }
}

impl TreeChange {
    /// Return the path that the change is about.
    pub(super) fn path(&self) -> &Path {
        match self {
            Self::New { path, .. }
            | Self::Changed { path }
            | Self::Missing { path, .. }
            | Self::Unreadable { path, .. } => path,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for brevity and clear failure messages"
)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Write a file and make its parent directories.
    fn write_file(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn a_root_that_does_not_exist_gives_an_empty_tree() {
        let temp = TempDir::new().unwrap();

        let tree = DestinationTree::take(&temp.path().join("missing"));

        assert!(tree.nodes.is_empty());
        assert!(tree.errors.is_empty());
    }

    #[test]
    fn an_unchanged_tree_has_no_changes() {
        let temp = TempDir::new().unwrap();
        write_file(&temp.path().join("a.txt"), "a");
        write_file(&temp.path().join("sub").join("b.txt"), "b");
        let before = DestinationTree::take(temp.path());

        let now = DestinationTree::take(temp.path());

        assert_eq!(now.changes_since(&before, &BTreeSet::new()), vec![]);
    }

    #[test]
    fn a_directory_whose_contents_change_is_not_itself_a_change() {
        let temp = TempDir::new().unwrap();
        let sub = temp.path().join("sub");
        write_file(&sub.join("b.txt"), "b");
        let before = DestinationTree::take(temp.path());
        let written = sub.join("c.txt");
        write_file(&written, "c");

        let now = DestinationTree::take(temp.path());

        let paths = BTreeSet::from([written]);
        assert_eq!(now.changes_since(&before, &paths), vec![]);
    }

    #[test]
    fn a_written_path_is_left_out_of_every_change() {
        let temp = TempDir::new().unwrap();
        let old = temp.path().join("old.txt");
        let new = temp.path().join("new.txt");
        write_file(&old, "old");
        let before = DestinationTree::take(temp.path());
        fs::write(&old, "a longer text than before").unwrap();
        write_file(&new, "new");

        let now = DestinationTree::take(temp.path());

        let written = BTreeSet::from([old, new]);
        assert_eq!(now.changes_since(&before, &written), vec![]);
    }

    #[test]
    fn changes_come_in_path_order() {
        let temp = TempDir::new().unwrap();
        let a = temp.path().join("a.txt");
        let b = temp.path().join("b.txt");
        let c = temp.path().join("c.txt");
        write_file(&b, "b");
        write_file(&c, "c");
        let before = DestinationTree::take(temp.path());
        write_file(&a, "a");
        fs::remove_file(&b).unwrap();
        fs::write(&c, "a longer text than before").unwrap();

        let now = DestinationTree::take(temp.path());

        let paths: Vec<PathBuf> = now
            .changes_since(&before, &BTreeSet::new())
            .iter()
            .map(|change| change.path().to_path_buf())
            .collect();
        assert_eq!(paths, vec![a, b, c]);
    }
}
