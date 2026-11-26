//! The seam between the walk and the operating system.
//!
//! Each method of [`FileSystem`] is one system call, and it gives back the raw
//! error of that call, `EINTR` included. The walk decides what to do with each
//! error, and it is the one place that does an interrupted call again. A test
//! can thus put a fault between the walk and the kernel, and prove what the
//! walk does with it.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;

/// What the directory entry says about the type of an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A directory. A symbolic link to a directory is [`EntryKind::Other`].
    Directory,
    /// Any entry that is not a directory.
    Other,
    /// The file system did not give the type. The walk asks
    /// [`FileSystem::is_directory`].
    Unknown,
}

/// One entry of a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntry {
    /// The name of the entry. It is never `.` or `..`.
    pub name: OsString,
    /// The type that the directory entry gives.
    pub kind: EntryKind,
}

/// How [`FileSystem::open_dir`] treats a symbolic link at the last component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// Open the directory that the link points to. The walk uses this for a
    /// root that the user named.
    Follow,
    /// Refuse a link. The walk uses this for each directory below a root.
    NoFollow,
}

/// The calls that a walk makes. Each call gives back its raw error.
pub trait FileSystem: Sync {
    /// An open directory.
    type Dir: Send;

    /// Opens the directory at `path` for reading.
    ///
    /// # Errors
    ///
    /// Returns the error of the call, for example `EINTR`, `EACCES`, or
    /// `ENOENT`.
    fn open_dir(&self, path: &Path, link: Link) -> io::Result<Self::Dir>;

    /// Gives the device id of an open directory.
    ///
    /// # Errors
    ///
    /// Returns the error of the call.
    fn device(&self, dir: &Self::Dir) -> io::Result<u64>;

    /// Reads the next entry of `dir`, or `None` at the end. It never gives
    /// `.` or `..`. After an `EINTR`, a call reads the same position again.
    ///
    /// # Errors
    ///
    /// Returns the error of the call.
    fn read_entry(&self, dir: &mut Self::Dir) -> io::Result<Option<RawEntry>>;

    /// Tells if the entry `name` of `dir` is a directory. It does not follow
    /// a symbolic link.
    ///
    /// # Errors
    ///
    /// Returns the error of the call.
    fn is_directory(&self, dir: &Self::Dir, name: &OsStr) -> io::Result<bool>;
}

/// The file system of the machine, through `libc`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFs;

/// A directory that [`RealFs`] opened.
#[derive(Debug)]
pub struct RealDir;

impl FileSystem for RealFs {
    type Dir = RealDir;

    fn open_dir(&self, _path: &Path, _link: Link) -> io::Result<RealDir> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn device(&self, _dir: &RealDir) -> io::Result<u64> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn read_entry(&self, _dir: &mut RealDir) -> io::Result<Option<RawEntry>> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn is_directory(&self, _dir: &RealDir, _name: &OsStr) -> io::Result<bool> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
