//! The parallel walk of one or more directory trees.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use crate::fs::FileSystem;
use crate::pattern::Matcher;

/// One tree to walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkRoot {
    /// The directory where the walk starts. If it is a symbolic link, the walk
    /// follows it.
    pub path: PathBuf,
    /// The directories below `path` that the walk does not enter. Each one is
    /// written as the walk writes it: `path` joined with the names below it.
    pub prune: BTreeSet<PathBuf>,
    /// When true, the walk does not enter a directory on a device that is not
    /// the device of `path`.
    pub stay_on_device: bool,
}

impl WalkRoot {
    /// A root that crosses each mount point and prunes nothing.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            prune: BTreeSet::new(),
            stay_on_device: false,
        }
    }
}

/// The choices that apply to each root of a walk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// When true, the walk enters the File Provider directories. See
    /// [`is_cloud_directory`].
    pub include_cloud: bool,
}

/// A subtree that the walk could not read completely.
#[derive(Debug)]
pub struct Incomplete {
    /// The directory that the walk could not read to its end.
    pub path: PathBuf,
    /// The error that stopped the read.
    pub error: io::Error,
}

/// What a walk did. Each list is sorted by path.
#[derive(Debug, Default)]
pub struct Report {
    /// The count of the entries that the walk read, directories included.
    pub entries: u64,
    /// The count of the entries that matched.
    pub matches: u64,
    /// The count of the calls that returned `EINTR` and that the walk did
    /// again.
    pub retried_interruptions: u64,
    /// The subtrees that the walk could not read completely.
    pub incomplete: Vec<Incomplete>,
    /// The directories that the system refused to open, with `EACCES` or
    /// `EPERM`. System Integrity Protection and the privacy settings of macOS
    /// refuse some directories on each machine, thus a refusal is normal.
    pub denied: Vec<PathBuf>,
    /// The File Provider directories that the walk did not enter.
    pub cloud_skipped: Vec<PathBuf>,
    /// The error of the sink that stopped the walk, if one did.
    pub output_error: Option<io::Error>,
}

impl Report {
    /// Tells if each subtree of each root was read to its end.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.incomplete.is_empty()
    }
}

/// Tells if the directory `name` in `parent` is a File Provider directory.
///
/// macOS keeps a File Provider directory at `~/Library/CloudStorage/<name>`
/// for Google Drive, Proton Drive, Dropbox, and others, and iCloud Drive at
/// `~/Library/Mobile Documents`. Such a directory answers `readdir` over the
/// network.
#[must_use]
pub fn is_cloud_directory(_parent: &Path, _name: &std::ffi::OsStr) -> bool {
    false
}

/// Walks each root in parallel, and gives each matching path to `sink` when
/// the walk finds it.
///
/// The walk does not follow a symbolic link below a root. It does an
/// interrupted call again in place. If `sink` returns an error, the walk stops
/// and puts the error in [`Report::output_error`].
pub fn walk<F, S>(
    _fs: &F,
    _roots: &[WalkRoot],
    _options: Options,
    _matcher: &Matcher,
    _sink: S,
) -> Report
where
    F: FileSystem,
    S: Fn(&Path) -> io::Result<()> + Sync,
{
    Report::default()
}
