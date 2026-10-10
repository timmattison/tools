//! The identity of a node on disk: its device and its inode number.

use std::fs;
use std::io;
use std::path::Path;

/// The device and the inode number of a node. Two paths with the same identity are one node.
///
/// A rename keeps the identity. A new node that replaces an old one gets
/// another identity, also when its size, time, and data are equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct NodeIdentity {
    device: u64,
    inode: u64,
}

impl NodeIdentity {
    /// Return the identity in `metadata`.
    ///
    /// Return `None` when the file system gives no inode number. Such a file
    /// system reports 0, and 0 names no node.
    #[cfg(unix)]
    pub(crate) fn of(metadata: &fs::Metadata) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        (metadata.ino() != 0).then(|| Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    /// Return `None`. This platform gives no stable inode number through the standard library.
    #[cfg(not(unix))]
    pub(crate) fn of(_metadata: &fs::Metadata) -> Option<Self> {
        None
    }

    /// Return the identity of the node at `path`. The call does not follow a symlink.
    pub(crate) fn of_node(path: &Path) -> io::Result<Option<Self>> {
        fs::symlink_metadata(path).map(|metadata| Self::of(&metadata))
    }
}
