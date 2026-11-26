//! The parallel walk of one or more directory trees.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::fs::{EntryKind, FileSystem, Link};
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

/// The count of the interruptions in a row after which the walk stops to do
/// one call again.
///
/// A signal interrupts one call, thus a real interruption ends after one more
/// try. Only a call that each try interrupts reaches this bound. The walk then
/// reports the subtree of that call as incomplete, and it does not hang.
pub const MAX_RETRIES_IN_A_ROW: u32 = 1000;

/// Tells if `error` says that an entry went away, or changed its type, after
/// the walk read its name. Such an entry is not part of the tree any more, thus
/// its loss is not a failure.
fn vanished(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    ) || error.raw_os_error() == Some(libc::ELOOP)
}

/// Tells if the directory `name` in `parent` is a File Provider directory.
///
/// macOS keeps a File Provider directory at `~/Library/CloudStorage/<name>`
/// for Google Drive, Proton Drive, Dropbox, and others, and iCloud Drive at
/// `~/Library/Mobile Documents`. Such a directory answers `readdir` over the
/// network.
#[must_use]
pub fn is_cloud_directory(parent: &Path, name: &OsStr) -> bool {
    parent.ends_with(CLOUD_STORAGE)
        || (name == MOBILE_DOCUMENTS && parent.file_name() == Some(OsStr::new(LIBRARY)))
}

/// The directory of a home that holds the File Provider directories.
const CLOUD_STORAGE: &str = "Library/CloudStorage";
/// The directory of a home that holds iCloud Drive.
const MOBILE_DOCUMENTS: &str = "Mobile Documents";
/// The directory of a home that holds [`MOBILE_DOCUMENTS`].
const LIBRARY: &str = "Library";

/// Walks each root in parallel, and gives each matching path to `sink` when
/// the walk finds it.
///
/// The walk does not follow a symbolic link below a root. It does an
/// interrupted call again in place. If `sink` returns an error, the walk stops
/// and puts the error in [`Report::output_error`].
pub fn walk<F, S>(
    fs: &F,
    roots: &[WalkRoot],
    options: Options,
    matcher: &Matcher,
    sink: S,
) -> Report
where
    F: FileSystem,
    S: Fn(&Path) -> io::Result<()> + Sync,
{
    let walker = Walker {
        fs,
        options,
        matcher,
        sink,
        stop: AtomicBool::new(false),
        entries: AtomicU64::new(0),
        matches: AtomicU64::new(0),
        retried: AtomicU64::new(0),
        found: Mutex::new(Findings::default()),
    };
    rayon::scope(|scope| {
        for root in roots {
            let walker = &walker;
            scope.spawn(move |scope| walker.walk_root(scope, root));
        }
    });
    walker.into_report()
}

/// What the walk keeps under one lock: each problem that it found.
#[derive(Default)]
struct Findings {
    incomplete: Vec<Incomplete>,
    denied: Vec<PathBuf>,
    cloud_skipped: Vec<PathBuf>,
    output_error: Option<io::Error>,
}

/// The state that each task of one walk shares.
struct Walker<'a, F, S> {
    fs: &'a F,
    options: Options,
    matcher: &'a Matcher,
    sink: S,
    stop: AtomicBool,
    entries: AtomicU64,
    matches: AtomicU64,
    retried: AtomicU64,
    found: Mutex<Findings>,
}

impl<'a, F, S> Walker<'a, F, S>
where
    F: FileSystem,
    S: Fn(&Path) -> io::Result<()> + Sync,
{
    /// Opens a root, and reads it.
    fn walk_root<'s>(&'s self, scope: &rayon::Scope<'s>, root: &'s WalkRoot) {
        let dir = match self.retry(|| self.fs.open_dir(&root.path, Link::Follow)) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotADirectory => {
                self.match_file_root(&root.path, error);
                return;
            }
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                self.denied(root.path.clone());
                return;
            }
            Err(error) => {
                self.incomplete(root.path.clone(), error);
                return;
            }
        };
        let device = if root.stay_on_device {
            match self.retry(|| self.fs.device(&dir)) {
                Ok(device) => Some(device),
                Err(error) => {
                    self.incomplete(root.path.clone(), error);
                    return;
                }
            }
        } else {
            None
        };
        self.read_dir(scope, root, device, root.path.clone(), dir);
    }

    /// Opens a directory below a root, and reads it.
    fn enter<'s>(
        &'s self,
        scope: &rayon::Scope<'s>,
        root: &'s WalkRoot,
        device: Option<u64>,
        path: PathBuf,
    ) {
        if self.stop.load(Ordering::Relaxed) {
            return;
        }
        let dir = match self.retry(|| self.fs.open_dir(&path, Link::NoFollow)) {
            Ok(dir) => dir,
            Err(error) if vanished(&error) => return,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                self.denied(path);
                return;
            }
            Err(error) => {
                self.incomplete(path, error);
                return;
            }
        };
        if let Some(device) = device {
            match self.retry(|| self.fs.device(&dir)) {
                Ok(found) if found == device => {}
                Ok(_) => return,
                Err(error) => {
                    self.incomplete(path, error);
                    return;
                }
            }
        }
        self.read_dir(scope, root, device, path, dir);
    }

    /// Reads each entry of `dir`. It gives each match to the sink, and it
    /// starts one task for each directory to enter.
    fn read_dir<'s>(
        &'s self,
        scope: &rayon::Scope<'s>,
        root: &'s WalkRoot,
        device: Option<u64>,
        path: PathBuf,
        mut dir: F::Dir,
    ) {
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            let entry = match self.retry(|| self.fs.read_entry(&mut dir)) {
                Ok(Some(entry)) => entry,
                Ok(None) => return,
                Err(error) => {
                    self.incomplete(path, error);
                    return;
                }
            };
            self.entries.fetch_add(1, Ordering::Relaxed);
            let child = path.join(&entry.name);
            if self.matcher.is_match(&entry.name) {
                self.matches.fetch_add(1, Ordering::Relaxed);
                if let Err(error) = (self.sink)(&child) {
                    self.stop_on_output_error(error);
                    return;
                }
            }
            let is_directory = match entry.kind {
                EntryKind::Directory => true,
                EntryKind::Other => false,
                EntryKind::Unknown => {
                    match self.retry(|| self.fs.is_directory(&dir, &entry.name)) {
                        Ok(is_directory) => is_directory,
                        Err(error) if vanished(&error) => false,
                        Err(error) => {
                            self.incomplete(child, error);
                            continue;
                        }
                    }
                }
            };
            if !is_directory || root.prune.contains(&child) {
                continue;
            }
            if !self.options.include_cloud && is_cloud_directory(&path, &entry.name) {
                self.findings().cloud_skipped.push(child);
                continue;
            }
            scope.spawn(move |scope| self.enter(scope, root, device, child));
        }
    }

    /// Matches a root that is not a directory by its name, as `find` does.
    /// `error` is the `ENOTDIR` of the open. A component above the last one
    /// can give that error too, thus the root must exist.
    fn match_file_root(&self, path: &Path, error: io::Error) {
        if std::fs::metadata(path).is_err() {
            self.incomplete(path.to_path_buf(), error);
            return;
        }
        let Some(name) = path.file_name() else {
            return;
        };
        if self.matcher.is_match(name) {
            self.matches.fetch_add(1, Ordering::Relaxed);
            if let Err(error) = (self.sink)(path) {
                self.stop_on_output_error(error);
            }
        }
    }

    fn denied(&self, path: PathBuf) {
        self.findings().denied.push(path);
    }

    /// Does `call`, and does it again after each `EINTR`. An interrupted call
    /// did nothing, thus the same call reads the same position again. After
    /// [`MAX_RETRIES_IN_A_ROW`] interruptions in a row, it gives the `EINTR`
    /// back, and the caller reports the subtree as incomplete.
    fn retry<T>(&self, mut call: impl FnMut() -> io::Result<T>) -> io::Result<T> {
        let mut retries = 0;
        loop {
            match call() {
                Err(error)
                    if error.kind() == io::ErrorKind::Interrupted
                        && retries < MAX_RETRIES_IN_A_ROW =>
                {
                    retries += 1;
                    self.retried.fetch_add(1, Ordering::Relaxed);
                }
                result => return result,
            }
        }
    }

    fn incomplete(&self, path: PathBuf, error: io::Error) {
        self.findings().incomplete.push(Incomplete { path, error });
    }

    fn stop_on_output_error(&self, error: io::Error) {
        self.stop.store(true, Ordering::Relaxed);
        let mut findings = self.findings();
        if findings.output_error.is_none() {
            findings.output_error = Some(error);
        }
    }

    fn findings(&self) -> std::sync::MutexGuard<'_, Findings> {
        // A task that panicked while it held the lock left complete lists, so
        // the lists stay usable.
        self.found
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn into_report(self) -> Report {
        let mut findings = self
            .found
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        findings
            .incomplete
            .sort_by(|left, right| left.path.cmp(&right.path));
        findings.denied.sort();
        findings.cloud_skipped.sort();
        Report {
            entries: self.entries.into_inner(),
            matches: self.matches.into_inner(),
            retried_interruptions: self.retried.into_inner(),
            incomplete: findings.incomplete,
            denied: findings.denied,
            cloud_skipped: findings.cloud_skipped,
            output_error: findings.output_error,
        }
    }
}
