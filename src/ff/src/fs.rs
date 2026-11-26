//! The seam between the walk and the operating system.
//!
//! Each method of [`FileSystem`] is one system call, and it gives back the raw
//! error of that call, `EINTR` included. The walk decides what to do with each
//! error, and it is the one place that does an interrupted call again. A test
//! can thus put a fault between the walk and the kernel, and prove what the
//! walk does with it.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::NonNull;

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
///
/// The standard library cannot do this job. After a read error,
/// `std::fs::ReadDir` ends its stream, thus a caller cannot read the same
/// position again. `walkdir` and `ignore` read through `ReadDir`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFs;

/// A directory stream that [`RealFs`] opened. It closes when it drops.
#[derive(Debug)]
pub struct RealDir {
    stream: NonNull<libc::DIR>,
}

// SAFETY: `RealDir` owns its stream, and it is not `Sync`. Thus only the thread
// that holds the `RealDir` can use the stream, and a move to another thread
// moves the only handle. A `DIR` has no tie to the thread that opened it.
unsafe impl Send for RealDir {}

impl Drop for RealDir {
    fn drop(&mut self) {
        // SAFETY: the stream is open, and nothing uses it after this call. An
        // error of `closedir` frees the stream too, thus there is nothing to
        // do again.
        unsafe {
            libc::closedir(self.stream.as_ptr());
        }
    }
}

impl RealDir {
    fn descriptor(&self) -> libc::c_int {
        // SAFETY: the stream is open until `self` drops.
        unsafe { libc::dirfd(self.stream.as_ptr()) }
    }
}

/// Makes a C string of `bytes`.
fn c_string(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a path holds a NUL byte"))
}

/// Sets the `errno` of this thread to 0. `readdir` gives `NULL` at the end of
/// a stream and after an error, and only `errno` tells the two apart.
fn clear_errno() {
    #[cfg(target_os = "macos")]
    // SAFETY: `__error` gives the address of the `errno` of this thread.
    unsafe {
        *libc::__error() = 0;
    }
    #[cfg(target_os = "linux")]
    // SAFETY: `__errno_location` gives the address of the `errno` of this
    // thread.
    unsafe {
        *libc::__errno_location() = 0;
    }
}

/// The device id of `stat`, as one type on each platform.
fn device_id(stat: &libc::stat) -> u64 {
    #[cfg(target_os = "macos")]
    {
        u64::from(stat.st_dev.cast_unsigned())
    }
    #[cfg(not(target_os = "macos"))]
    {
        stat.st_dev
    }
}

impl FileSystem for RealFs {
    type Dir = RealDir;

    fn open_dir(&self, path: &Path, link: Link) -> io::Result<RealDir> {
        let path = c_string(path.as_os_str().as_bytes())?;
        let mut flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
        if link == Link::NoFollow {
            flags |= libc::O_NOFOLLOW;
        }
        // SAFETY: `path` is a NUL-terminated string that lives for the call.
        let descriptor = unsafe { libc::open(path.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `descriptor` is an open directory, and this function owns it.
        let stream = unsafe { libc::fdopendir(descriptor) };
        if let Some(stream) = NonNull::new(stream) {
            return Ok(RealDir { stream });
        }
        let error = io::Error::last_os_error();
        // SAFETY: `fdopendir` failed, thus `descriptor` is still open and still
        // owned by this function.
        unsafe {
            libc::close(descriptor);
        }
        Err(error)
    }

    fn device(&self, dir: &RealDir) -> io::Result<u64> {
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the descriptor is open, and `stat` is writable memory for
        // one `stat` struct.
        if unsafe { libc::fstat(dir.descriptor(), stat.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fstat` succeeded, thus it wrote each field of the struct.
        let stat = unsafe { stat.assume_init() };
        Ok(device_id(&stat))
    }

    fn read_entry(&self, dir: &mut RealDir) -> io::Result<Option<RawEntry>> {
        loop {
            clear_errno();
            // SAFETY: the stream is open, and `&mut` keeps each other use of
            // it out until this call returns.
            let entry = unsafe { libc::readdir(dir.stream.as_ptr()) };
            if entry.is_null() {
                let error = io::Error::last_os_error();
                return match error.raw_os_error() {
                    None | Some(0) => Ok(None),
                    Some(_) => Err(error),
                };
            }
            // SAFETY: `readdir` gave a valid entry, and the entry stays valid
            // until the next call on this stream. The name is NUL-terminated.
            let (name, d_type) = unsafe {
                let entry = &*entry;
                (
                    CStr::from_ptr(entry.d_name.as_ptr()).to_bytes(),
                    entry.d_type,
                )
            };
            if name == b"." || name == b".." {
                continue;
            }
            let kind = match d_type {
                libc::DT_DIR => EntryKind::Directory,
                libc::DT_UNKNOWN => EntryKind::Unknown,
                _ => EntryKind::Other,
            };
            return Ok(Some(RawEntry {
                name: OsStr::from_bytes(name).to_os_string(),
                kind,
            }));
        }
    }

    fn is_directory(&self, dir: &RealDir, name: &OsStr) -> io::Result<bool> {
        let name = c_string(name.as_bytes())?;
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the descriptor is open, `name` is NUL-terminated, and `stat`
        // is writable memory for one `stat` struct.
        let status = unsafe {
            libc::fstatat(
                dir.descriptor(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fstatat` succeeded, thus it wrote each field of the struct.
        let stat = unsafe { stat.assume_init() };
        Ok(stat.st_mode & libc::S_IFMT == libc::S_IFDIR)
    }
}
