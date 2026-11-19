//! The one status item of each target: the lock that marks it, and the calls
//! to the kernel that detach it from the terminal and that stop it.
//!
//! A [`Target`] is the Mac whose values a status item shows: this Mac, or the
//! destination of `--host`. Each target has its own lock file, so one status
//! item of each target can run, and a stop of one target leaves the others
//! alone.
//!
//! The status item that runs holds a write lock (`fcntl` with `F_SETLK`) on
//! the whole lock file. Any process can ask the kernel for the holder of that
//! lock (`F_GETLK`), and the kernel gives the PID of the holder. The kernel
//! releases the lock when the process ends for any cause, `SIGKILL` included.
//! Thus a stale lock cannot occur, and the file holds no data.
//!
//! The lock is a POSIX record lock. Two rules of those locks shape this
//! module:
//!
//! - The kernel releases the lock of a process when that process closes any
//!   descriptor of the file, not only the descriptor that took the lock. Thus
//!   the holder never opens the lock file a second time. [`holder`] opens and
//!   closes the file, so a process calls it only before its [`claim`].
//! - A process never conflicts with its own lock. Thus [`holder`] gives
//!   `None` in the process that holds the lock.
//!
//! The lock file is in the data directory of the user, not in `$TMPDIR` or in
//! a cache directory. macOS deletes old files in those. When the file of a
//! held lock goes, the next start makes a new file and locks it. Then two
//! status items run, and neither one finds the other.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::num::NonZeroU32;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::connection::Destination;

/// The name of the directory of `glance` in the data directory of the user.
pub(crate) const DIRECTORY_NAME: &str = "glance";

/// Gives the data directory of the current user. On macOS it is
/// `~/Library/Application Support`.
///
/// # Errors
///
/// Returns an error of kind [`io::ErrorKind::NotFound`] when the data
/// directory of the user is not known.
pub(crate) fn user_data_directory() -> io::Result<PathBuf> {
    dirs::data_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "the data directory of this user is not known",
        )
    })
}

/// The name of the lock file of the local status item.
const LOCAL_FILE_NAME: &str = "local.lock";

/// The start of the name of the lock file of a host.
const HOST_FILE_PREFIX: &str = "host-";

/// The end of the name of each lock file.
const LOCK_FILE_SUFFIX: &str = ".lock";

/// The autosave name of the local status item.
const LOCAL_AUTOSAVE_NAME: &str = "glance.local";

/// The start of the autosave name of the status item of a host.
const HOST_AUTOSAVE_PREFIX: &str = "glance.host.";

/// The `l_whence` of a lock record that starts at the start of the file. This
/// is `SEEK_SET`, which `libc` gives as a wider integer than the field.
const FROM_THE_START: libc::c_short = 0;

/// How many times [`claim`] tries again when the holder ends between the
/// refusal of the lock and the look at the holder.
const CLAIM_ATTEMPTS: u32 = 10;

/// The longest time that [`stop`] waits for the status item to end.
///
/// The status item ends at once on `SIGTERM`, through
/// [`end_with_success_on_terminate`]. The bound only stops a wait for a
/// process that ignores the signal.
pub const STOP_BOUND: Duration = Duration::from_secs(10);

/// The time between two looks at the lock while [`stop`] waits.
const STOP_POLL: Duration = Duration::from_millis(10);

/// The Mac whose values a status item shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// This Mac.
    Local,
    /// The second Mac at this `--host` destination, as the user typed it.
    Host(Destination),
}

impl Target {
    /// Gives the name of the lock file of this target.
    ///
    /// The local target gives `local.lock`. A host gives `host-`, the
    /// destination, and `.lock`. Each byte of the destination that is not a
    /// lowercase ASCII letter, a digit, `.`, `-`, `_`, or `@` becomes `%` and
    /// two uppercase hex digits. Thus the name holds no `/`, and two different
    /// destinations never give the same name. The default file system of macOS
    /// folds case, so a raw uppercase letter could name the same file as its
    /// lowercase form.
    #[must_use]
    pub fn file_name(&self) -> String {
        match self {
            Target::Local => LOCAL_FILE_NAME.to_owned(),
            Target::Host(destination) => {
                format!(
                    "{HOST_FILE_PREFIX}{}{LOCK_FILE_SUFFIX}",
                    escaped(destination)
                )
            }
        }
    }

    /// Gives the autosave name of the status item of this target.
    ///
    /// AppKit keeps the position of a status item under its autosave name,
    /// and a menu bar manager such as Bartender keys each item on that name
    /// and the bundle identifier. With no name, each status item gets
    /// `Item-0`, so all of them share one key. The local target gives
    /// `glance.local`. A host gives `glance.host.` and the destination, with
    /// the escape of [`Target::file_name`]. Thus two different destinations
    /// never give the same name.
    #[must_use]
    pub fn autosave_name(&self) -> String {
        match self {
            Target::Local => LOCAL_AUTOSAVE_NAME.to_owned(),
            Target::Host(destination) => format!("{HOST_AUTOSAVE_PREFIX}{}", escaped(destination)),
        }
    }
}

/// Gives `destination` with each byte that is not a lowercase ASCII letter, a
/// digit, `.`, `-`, `_`, or `@` written as `%` and two uppercase hex digits.
///
/// The result holds no `/` and no uppercase letter from the destination, and
/// two different destinations never give the same result.
fn escaped(destination: &Destination) -> String {
    let mut text = String::new();
    for byte in destination.as_str().bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-_@".contains(&byte) {
            text.push(char::from(byte));
        } else {
            text.push_str(&format!("%{byte:02X}"));
        }
    }
    text
}

/// The lock file that marks the status item of one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockFile(PathBuf);

impl LockFile {
    /// Gives the lock file of `target` below the data directory `data`:
    /// `glance/` and the [file name](Target::file_name) of the target.
    #[must_use]
    pub fn in_data_directory(data: &Path, target: &Target) -> Self {
        Self(data.join(DIRECTORY_NAME).join(target.file_name()))
    }

    /// Gives the lock file of `target` for the current user. On macOS the
    /// lock file of this Mac is
    /// `~/Library/Application Support/glance/local.lock`.
    ///
    /// # Errors
    ///
    /// Returns an error of kind [`io::ErrorKind::NotFound`] when the data
    /// directory of the user is not known.
    pub fn for_user(target: &Target) -> io::Result<Self> {
        user_data_directory().map(|data| Self::in_data_directory(&data, target))
    }

    /// Gives the path of the lock file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Makes the directory and the lock file when they do not exist.
    ///
    /// A background start calls this before it starts its copy, so a problem
    /// with the file reaches the person and not the copy, which has no
    /// terminal.
    ///
    /// # Errors
    ///
    /// Returns the error of the file system when the directory or the file
    /// cannot be made or opened.
    pub fn prepare(&self) -> io::Result<()> {
        self.open_or_make().map(drop)
    }

    /// Opens the lock file to read and to write, and makes the file and its
    /// directory first when they do not exist. The open never empties the
    /// file, because another process can hold its lock.
    fn open_or_make(&self) -> io::Result<File> {
        if let Some(directory) = self.0.parent() {
            fs::create_dir_all(directory)?;
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.0)
    }

    /// Opens the lock file when it exists, and gives `None` when it does not.
    fn open_existing(&self) -> io::Result<Option<File>> {
        match OpenOptions::new().read(true).write(true).open(&self.0) {
            Ok(file) => Ok(Some(file)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl fmt::Display for LockFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0.display())
    }
}

/// The PID of a process. It is always larger than zero.
///
/// `kill` sends a signal with a PID of zero to each process in the group of
/// the caller, and a signal with a PID of -1 to each process of the user.
/// Thus [`stop`] takes only this type, and this type holds no such value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pid(libc::pid_t);

impl Pid {
    /// Gives the PID `raw`, or `None` when `raw` is not larger than zero.
    #[must_use]
    pub fn new(raw: libc::pid_t) -> Option<Self> {
        (raw > 0).then_some(Self(raw))
    }

    /// Gives the PID of this process.
    #[must_use]
    pub fn of_this_process() -> Self {
        // SAFETY: `getpid` takes nothing, always works, and changes nothing.
        Self(unsafe { libc::getpid() })
    }
}

impl fmt::Display for Pid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// The lock that the status item holds while it runs. A drop closes the file,
/// and the kernel then releases the lock.
#[derive(Debug)]
pub struct Claim {
    _file: File,
}

/// The reason that [`claim`] did not get the lock.
#[derive(Debug)]
pub enum ClaimError {
    /// Another process holds the lock.
    Held(Pid),
    /// The lock file cannot be opened, or the kernel refused the call.
    Io(io::Error),
}

/// The reason that [`stop`] did not stop the status item.
#[derive(Debug)]
pub enum StopError {
    /// The kernel refused the signal.
    Signal(Pid, io::Error),
    /// The lock file cannot be read.
    Look(io::Error),
    /// The status item still runs after [`STOP_BOUND`].
    StillRuns(Pid),
}

impl fmt::Display for StopError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StopError::Signal(pid, error) => write!(
                formatter,
                "cannot stop the status item that runs as PID {pid}: {error}"
            ),
            StopError::Look(error) => {
                write!(
                    formatter,
                    "cannot read the lock of the status item: {error}"
                )
            }
            StopError::StillRuns(pid) => write!(
                formatter,
                "the status item that runs as PID {pid} did not stop in {} s",
                STOP_BOUND.as_secs()
            ),
        }
    }
}

/// Gives a lock record of `kind` that covers the whole file.
fn whole_file(kind: libc::c_short) -> libc::flock {
    libc::flock {
        l_start: 0,
        l_len: 0,
        l_pid: 0,
        l_type: kind,
        l_whence: FROM_THE_START,
    }
}

/// Gives the PID of the process that holds the lock of `file`, or `None` when
/// no other process holds it.
fn holder_of(file: &File) -> io::Result<Option<Pid>> {
    let mut record = whole_file(libc::F_WRLCK);
    // SAFETY: The descriptor is open for the life of `file`. `F_GETLK` takes a
    // pointer to one `flock` record, and the kernel writes only into that
    // record.
    let result = unsafe {
        libc::fcntl(
            file.as_raw_fd(),
            libc::F_GETLK,
            std::ptr::from_mut(&mut record),
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    if record.l_type == libc::F_UNLCK {
        return Ok(None);
    }
    Pid::new(record.l_pid).map(Some).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the kernel gave the PID {} as the holder", record.l_pid),
        )
    })
}

/// Gives the PID of the status item that runs, or `None` when no status item
/// runs.
///
/// The holder of the lock never calls this function, because the close of
/// the file releases its own lock (see the module documentation).
///
/// # Errors
///
/// Returns the error of the file system or of the kernel when the lock file
/// cannot be opened or its lock cannot be read.
pub fn holder(lock: &LockFile) -> io::Result<Option<Pid>> {
    match lock.open_existing()? {
        Some(file) => holder_of(&file),
        None => Ok(None),
    }
}

/// Takes the lock for this process, and makes the lock file first when it
/// does not exist. The lock stays until the [`Claim`] drops or the process
/// ends.
///
/// # Errors
///
/// Returns [`ClaimError::Held`] with the PID of the holder when another
/// process holds the lock. Returns [`ClaimError::Io`] when the lock file
/// cannot be opened, and when the kernel refuses the call for a different
/// cause.
pub fn claim(lock: &LockFile) -> Result<Claim, ClaimError> {
    let file = lock.open_or_make().map_err(ClaimError::Io)?;
    for _ in 0..CLAIM_ATTEMPTS {
        let record = whole_file(libc::F_WRLCK);
        // SAFETY: The descriptor is open for the life of `file`. `F_SETLK`
        // takes a pointer to one `flock` record and only reads it.
        let result =
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, std::ptr::from_ref(&record)) };
        if result != -1 {
            return Ok(Claim { _file: file });
        }
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::EAGAIN | libc::EACCES)) {
            return Err(ClaimError::Io(error));
        }
        if let Some(pid) = holder_of(&file).map_err(ClaimError::Io)? {
            return Err(ClaimError::Held(pid));
        }
        // The holder ended between the refusal and the look. Try again.
    }
    Err(ClaimError::Io(io::Error::new(
        io::ErrorKind::WouldBlock,
        "the lock changed hands at each try",
    )))
}

/// Stops the status item that runs as `pid`, and waits until its lock is
/// free.
///
/// The function sends `SIGTERM`. A process that ended already is not an
/// error. The wait ends when the lock is free or another process holds it,
/// and it lasts for [`STOP_BOUND`] at most.
///
/// # Errors
///
/// Returns [`StopError::Signal`] when the kernel refuses the signal,
/// [`StopError::Look`] when the lock cannot be read, and
/// [`StopError::StillRuns`] when the process holds the lock after the bound.
pub fn stop(lock: &LockFile, pid: Pid) -> Result<(), StopError> {
    // SAFETY: `kill` takes two integers and touches no memory of this
    // process. The `Pid` type holds only values larger than zero, so the
    // signal goes to one process and never to a group.
    if unsafe { libc::kill(pid.0, libc::SIGTERM) } == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(StopError::Signal(pid, error));
        }
    }
    let deadline = Instant::now() + STOP_BOUND;
    loop {
        if holder(lock).map_err(StopError::Look)? != Some(pid) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(StopError::StillRuns(pid));
        }
        thread::sleep(STOP_POLL);
    }
}

/// Makes this process the leader of a new session, which has no controlling
/// terminal. Thus the close of the terminal of the start sends it no
/// `SIGHUP`, and the keys of that terminal send it no signal.
///
/// # Errors
///
/// Returns the error of the kernel. `setsid` fails only for a process that
/// leads a process group already.
pub fn start_a_session() -> io::Result<()> {
    // SAFETY: `setsid` takes nothing and changes only the session of this
    // process.
    if unsafe { libc::setsid() } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Points the standard output and the standard error of this process at
/// `/dev/null`.
///
/// The background copy calls this when its start has the ready line. The
/// start ends after that, and nobody reads the two pipes of the copy any
/// more. A write to a pipe with no reader fails, and Rust ignores `SIGPIPE`,
/// so `println!` and `eprintln!` then panic. A write to `/dev/null` always
/// works.
///
/// # Errors
///
/// Returns the error of the kernel when `/dev/null` cannot be opened or a
/// descriptor cannot be replaced.
pub fn send_output_nowhere() -> io::Result<()> {
    let null = OpenOptions::new().write(true).open("/dev/null")?;
    for target in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        // SAFETY: `dup2` takes two integers and touches no memory of this
        // process. The first descriptor is open for the life of `null`, and
        // the copy at `target` stays open after `null` drops.
        if unsafe { libc::dup2(null.as_raw_fd(), target) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The handler of `SIGTERM`: ends the process with status 0 at once.
extern "C" fn end_with_success(_signal: libc::c_int) {
    // SAFETY: `_exit` is async-signal-safe. It runs no destructor and no
    // handler of `atexit`, and the status item has nothing to save.
    unsafe { libc::_exit(libc::EXIT_SUCCESS) }
}

/// Makes `SIGTERM` end this process with status 0.
///
/// A stop is a quit on purpose, as the Quit item of the menu is. launchd
/// starts a job with `KeepAlive` and `SuccessfulExit` set to `false` again
/// after an exit with a status other than 0, and after an end by a signal.
/// The status 0 thus keeps a status item that a person stopped stopped.
///
/// # Errors
///
/// Returns the error of the kernel when it refuses the handler.
pub fn end_with_success_on_terminate() -> io::Result<()> {
    let handler: extern "C" fn(libc::c_int) = end_with_success;
    // SAFETY: The handler calls only `_exit`, which is async-signal-safe.
    let previous = unsafe { libc::signal(libc::SIGTERM, handler as libc::sighandler_t) };
    if previous == libc::SIG_ERR {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Ends this process `seconds` seconds from now, through `SIGALRM`.
pub fn end_after(seconds: NonZeroU32) {
    // SAFETY: `alarm` takes one integer and sets only the timer of this
    // process. The default action of `SIGALRM` ends the process.
    unsafe { libc::alarm(seconds.get()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A data directory for one test.
    fn data_directory() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("glance-data-")
            .tempdir()
            .expect("make a data directory")
    }

    /// The target of the host at `destination`.
    fn host(destination: &str) -> Target {
        Target::Host(destination.parse().expect("a destination"))
    }

    #[test]
    fn the_local_lock_file_is_in_the_glance_directory_of_the_data_directory() {
        let lock = LockFile::in_data_directory(Path::new("/data"), &Target::Local);
        assert_eq!(lock.path(), Path::new("/data/glance/local.lock"));
    }

    #[test]
    fn the_lock_file_of_a_host_is_beside_the_local_lock_file() {
        let lock = LockFile::in_data_directory(Path::new("/data"), &host("m1-ts"));
        assert_eq!(lock.path(), Path::new("/data/glance/host-m1-ts.lock"));
    }

    #[test]
    fn the_lock_file_of_the_user_is_below_the_data_directory_of_the_user() {
        let data = dirs::data_dir().expect("the data directory of this user");
        let target = host("tim@m1-ts");
        let lock = LockFile::for_user(&target).expect("the lock file of this user");
        assert_eq!(lock, LockFile::in_data_directory(&data, &target));
    }

    #[test]
    fn a_host_name_keeps_lowercase_letters_digits_and_the_marks_of_a_host() {
        assert_eq!(
            host("tim@m4-pro_mac.local").file_name(),
            "host-tim@m4-pro_mac.local.lock"
        );
        assert_eq!(
            host("tim@M4-Pro_Mac.local").file_name(),
            "host-tim@%4D4-%50ro_%4Dac.local.lock"
        );
    }

    #[test]
    fn a_host_name_escapes_each_other_byte() {
        assert_eq!(
            host("ssh://h:22").file_name(),
            "host-ssh%3A%2F%2Fh%3A22.lock"
        );
        assert_eq!(host("a b").file_name(), "host-a%20b.lock");
        assert_eq!(host("café").file_name(), "host-caf%C3%A9.lock");
        assert_eq!(host("日本").file_name(), "host-%E6%97%A5%E6%9C%AC.lock");
        assert_eq!(host("🎉").file_name(), "host-%F0%9F%8E%89.lock");
    }

    #[test]
    fn a_host_name_never_holds_a_slash() {
        let name = host("../../etc/passwd").file_name();
        assert!(!name.contains('/'), "{name}");
        assert_eq!(name, "host-..%2F..%2Fetc%2Fpasswd.lock");
    }

    #[test]
    fn two_destinations_that_differ_only_in_case_give_names_that_differ_after_case_folding() {
        for (a, b) in [("Alpha", "alpha"), ("Tim@m1", "tim@m1"), ("ALPHA", "alpha")] {
            // A case-folding file system compares the names in lowercase.
            assert_ne!(
                host(a).file_name().to_ascii_lowercase(),
                host(b).file_name().to_ascii_lowercase(),
                "{a} and {b}"
            );
        }
    }

    #[test]
    fn two_destinations_that_differ_only_in_case_make_two_lock_files() {
        let data = data_directory();
        for destination in ["Alpha", "alpha"] {
            LockFile::in_data_directory(data.path(), &host(destination))
                .prepare()
                .expect("prepare the lock file");
        }
        // On a case-folding volume (the macOS default), the old names give one file.
        let files = fs::read_dir(data.path().join(DIRECTORY_NAME))
            .expect("read the glance directory")
            .count();
        assert_eq!(files, 2);
    }

    #[test]
    fn two_different_destinations_give_two_different_names() {
        assert_ne!(host("a/b").file_name(), host("a%2Fb").file_name());
        assert_ne!(host("local").file_name(), Target::Local.file_name());
    }

    #[test]
    fn the_local_status_item_has_the_autosave_name_glance_local() {
        assert_eq!(Target::Local.autosave_name(), "glance.local");
    }

    #[test]
    fn the_autosave_name_of_a_host_is_glance_host_and_the_escaped_destination() {
        assert_eq!(host("m1-ts").autosave_name(), "glance.host.m1-ts");
        assert_eq!(
            host("tim@M4 Pro/日").autosave_name(),
            "glance.host.tim@%4D4%20%50ro%2F%E6%97%A5"
        );
    }

    #[test]
    fn two_different_destinations_give_two_different_autosave_names() {
        for (a, b) in [
            ("Alpha", "alpha"),
            ("Tim@m1", "tim@m1"),
            ("a/b", "a%2Fb"),
            ("a b", "a%20b"),
        ] {
            assert_ne!(
                host(a).autosave_name(),
                host(b).autosave_name(),
                "{a} and {b}"
            );
        }
        assert_ne!(host("local").autosave_name(), Target::Local.autosave_name());
    }

    #[test]
    fn a_missing_lock_file_has_no_holder() {
        let data = data_directory();
        let lock = LockFile::in_data_directory(data.path(), &Target::Local);
        assert_eq!(holder(&lock).expect("look at the lock"), None);
        assert!(!lock.path().exists(), "a look made the lock file");
    }

    #[test]
    fn a_prepared_lock_file_exists_and_has_no_holder() {
        let data = data_directory();
        let lock = LockFile::in_data_directory(data.path(), &Target::Local);
        lock.prepare().expect("prepare the lock file");
        assert!(lock.path().is_file());
        assert_eq!(holder(&lock).expect("look at the lock"), None);
    }

    #[test]
    fn a_claim_makes_the_directory_and_the_lock_file() {
        let data = data_directory();
        let lock = LockFile::in_data_directory(data.path(), &Target::Local);
        let claim = claim(&lock);
        assert!(claim.is_ok(), "{claim:?}");
        assert!(lock.path().is_file());
    }

    #[test]
    fn a_pid_is_larger_than_zero() {
        assert_eq!(Pid::new(0), None);
        assert_eq!(Pid::new(-1), None);
        assert_eq!(Pid::new(1).map(|pid| pid.to_string()), Some("1".to_owned()));
    }

    #[test]
    fn the_pid_of_this_process_is_the_id_of_this_process() {
        assert_eq!(
            Pid::of_this_process().to_string(),
            std::process::id().to_string()
        );
    }

    #[test]
    fn a_lock_record_starts_at_the_start_of_the_file() {
        assert_eq!(libc::c_int::from(FROM_THE_START), libc::SEEK_SET);
        let record = whole_file(libc::F_WRLCK);
        assert_eq!((record.l_start, record.l_len), (0, 0));
    }

    #[test]
    fn a_stop_that_timed_out_names_the_pid_and_the_bound() {
        let pid = Pid::new(4242).expect("a PID");
        assert_eq!(
            StopError::StillRuns(pid).to_string(),
            "the status item that runs as PID 4242 did not stop in 10 s"
        );
    }
}
