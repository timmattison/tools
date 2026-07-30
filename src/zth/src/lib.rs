//! Zero the Hero - locate files whose contents are nothing but zero bytes.
//!
//! [`find_all_zero_files`] is the whole tool in one call: point it at a path and
//! it walks the tree, reads every file it finds, and hands back the absolute,
//! sorted paths of the ones that are non-empty and entirely zeroes. Discovery
//! and reading overlap, every I/O error is silently skipped, and a
//! [`ScanProgress`] observer sees the running totals as they change.
//!
//! [`file_is_all_zeroes`] answers the same question for a single file. It stops
//! at the first non-zero byte rather than reading or hashing the whole file,
//! settles wholly-sparse files without reading them at all, and asks the kernel
//! not to cache what it does read - a scan touches every byte exactly once, so
//! anything it leaves in the page cache is held for no one.
//!
//! # Example
//!
//! ```rust,ignore
//! use std::path::Path;
//! use zth::{find_all_zero_files, Jobs, NoProgress};
//!
//! for path in find_all_zero_files(Path::new("/data"), Jobs::default(), &NoProgress) {
//!     println!("{}", path.display());
//! }
//! ```

use std::fs::File;
use std::io::{self, Read};
use std::num::NonZeroUsize;
use std::panic::resume_unwind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::thread;

use walkdir::WalkDir;

/// Size of the buffer each read fills before the block is tested for zeroes.
///
/// 256 KiB is large enough that the per-`read` syscall overhead disappears into
/// the memory comparison, and small enough that a worker's buffer stays inside
/// a typical L2 cache.
const READ_BUFFER_LEN: usize = 256 * 1024;

/// Size of the first read, before the file has shown any sign of being zeroes.
///
/// Almost every file is disqualified by its very first byte, so the first read
/// exists to reject rather than to consume, and reading a full
/// [`READ_BUFFER_LEN`] to look at one byte is waste in two directions: the
/// transfer time itself, and - much worse on a large scan - the page cache it
/// evicts. Three hundred thousand files at 256 KiB apiece flush the directory
/// metadata the walk is about to need, buying extra seeks in exchange for bytes
/// nothing ever looks at.
///
/// 16 KiB is several filesystem blocks, which keeps files that legitimately open
/// with a run of zeroes - padded headers, preallocated records - resolving in a
/// single read, while still asking the disk for a sixteenth of what a full
/// buffer would.
const PROBE_READ_LEN: usize = 16 * 1024;

/// A block of zero bytes that freshly-read data is compared against.
///
/// Comparing two byte slices lowers to `memcmp`, which is both vectorized and
/// early-exiting on every platform we care about - far faster than a per-byte
/// loop, and without the `unsafe` a hand-rolled word-at-a-time scan would need.
/// Lives in `.bss`, so it costs address space rather than binary size.
static ZERO_BLOCK: [u8; READ_BUFFER_LEN] = [0; READ_BUFFER_LEN];

/// Returns `true` when every byte of `bytes` is zero.
///
/// An empty slice is vacuously all zeroes; callers that care about emptiness
/// (as [`file_is_all_zeroes`] does) must track it themselves.
fn slice_is_all_zeroes(bytes: &[u8]) -> bool {
    // chunks() never yields more than ZERO_BLOCK.len() bytes, so the get() below
    // always succeeds; it is written fallibly to keep the function panic-free
    // regardless of what a future caller passes in.
    bytes.chunks(ZERO_BLOCK.len()).all(|chunk| {
        ZERO_BLOCK
            .get(..chunk.len())
            .is_some_and(|zeroes| chunk == zeroes)
    })
}

/// Reads `reader` to exhaustion, reporting whether it yielded at least one byte
/// and every byte it yielded was zero.
///
/// Returns as soon as a non-zero byte is seen. `buffer` is caller-owned so a
/// worker scanning thousands of files can reuse one allocation.
///
/// The first read asks for only [`PROBE_READ_LEN`] bytes and the rest ask for
/// the whole buffer: until the probe comes back clean the likeliest outcome by
/// far is rejection, and once it does the likeliest outcome is reading to the
/// end. A buffer shorter than the probe simply caps both.
fn reader_is_all_zeroes(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<bool> {
    let mut saw_bytes = false;
    let mut window = PROBE_READ_LEN.min(buffer.len());

    // Slicing is bounded by the min() above and by the assignment below, both
    // of which clamp to the buffer's length, so the loop ends on a read of zero
    // bytes rather than on a failed slice.
    while let Some(target) = buffer.get_mut(..window) {
        let filled = match reader.read(target) {
            Ok(0) => break,
            Ok(filled) => filled,
            // A signal arriving mid-read is not a failure of the file, and it
            // is not evidence about the probe either - retry at the same width.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };

        saw_bytes = true;
        window = buffer.len();

        // Only the bytes this read actually filled are meaningful; the tail of
        // the buffer still holds the previous read's data.
        if !slice_is_all_zeroes(&buffer[..filled]) {
            return Ok(false);
        }
    }

    Ok(saw_bytes)
}

/// Returns `true` when `path` names a file that is both non-empty and made up
/// entirely of zero bytes.
///
/// Reading stops at the first non-zero byte, so a large file whose first byte
/// is non-zero costs a single read. Empty files return `false`: a file with no
/// bytes has no zero bytes either.
///
/// # Errors
///
/// Returns any [`io::Error`] raised while opening or reading the file - it does
/// not exist, it is a directory, permissions deny the read, and so on.
pub fn file_is_all_zeroes(path: &Path) -> io::Result<bool> {
    let mut buffer = vec![0_u8; READ_BUFFER_LEN];
    file_is_all_zeroes_with_buffer(path, &mut buffer)
}

/// [`file_is_all_zeroes`] against a caller-supplied scratch buffer.
///
/// The scanning workers hold one buffer for their entire lifetime, so the read
/// path never allocates per file.
fn file_is_all_zeroes_with_buffer(path: &Path, buffer: &mut [u8]) -> io::Result<bool> {
    let mut scanned = ScanFile::open(path)?;

    // A file that is nothing but hole is already answered, and the answer cost
    // no reads at all - which is the difference between settling a 64 GiB
    // sparse file instantly and grinding through it a buffer at a time.
    if whole_file_is_a_hole(&scanned.file) {
        return Ok(true);
    }

    reader_is_all_zeroes(&mut scanned, buffer)
}

/// A file opened for a single sequential pass whose contents the page cache is
/// asked not to keep.
///
/// zth reads a byte exactly once and never comes back for it, so every page it
/// caches is a page held for no one. At a few hundred files that is harmless;
/// at the few hundred thousand this tool is built for, the reads evict the
/// directory metadata the walk is still working through, and the scan buys
/// extra seeks in exchange for bytes nothing will ever look at again. Telling
/// the kernel up front is what keeps a long scan from competing with itself.
///
/// Where the hint lands in a file's life is a platform question - some kernels
/// take it at open, others only once the reading is done - so it is asked for
/// here at both ends and each platform answers the half it can. A kernel that
/// refuses is not an error: the scan reads the file either way, it just leaves
/// a colder cache behind.
struct ScanFile {
    file: File,
}

impl ScanFile {
    /// Opens `path` for reading and asks that its contents not be retained.
    ///
    /// # Errors
    ///
    /// Returns any [`io::Error`] from opening the file. Failure to apply the
    /// cache hint is not one of them.
    fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        keep_out_of_the_page_cache(&file);
        Ok(Self { file })
    }
}

impl Read for ScanFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.file.read(buffer)
    }
}

impl Drop for ScanFile {
    fn drop(&mut self) {
        release_from_the_page_cache(&self.file);
    }
}

/// Asks the kernel, before any reading happens, not to retain this file's
/// contents.
///
/// Darwin answers this one outright: `F_NOCACHE` keeps the file's pages out of
/// the unified buffer cache for the life of the descriptor, which leaves
/// [`release_from_the_page_cache`] nothing to do afterwards.
#[cfg(target_vendor = "apple")]
fn keep_out_of_the_page_cache(file: &File) {
    use std::os::unix::io::AsRawFd;

    // SAFETY: the descriptor is borrowed from a File that outlives the call,
    // and fcntl touches nothing but that descriptor's own flags. The result is
    // deliberately ignored - a kernel that will not take the hint changes how
    // much cache the scan leaves behind, never what it finds.
    unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) };
}

/// Linux cannot refuse the cache up front for buffered reads, so this declares
/// the access pattern instead - which is the truth, and lets readahead work in
/// the scan's favor. The pages themselves are dropped by
/// [`release_from_the_page_cache`] once they have been read.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn keep_out_of_the_page_cache(file: &File) {
    use std::os::unix::io::AsRawFd;

    // SAFETY: as above. A zero length means "through the end of the file".
    unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_SEQUENTIAL) };
}

/// Platforms with no cache hint to offer read the file the ordinary way.
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
fn keep_out_of_the_page_cache(_file: &File) {}

/// Drops whatever this file left in the page cache, once the scan is done with
/// it.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn release_from_the_page_cache(file: &File) {
    use std::os::unix::io::AsRawFd;

    // SAFETY: the descriptor is borrowed from a File that outlives the call.
    unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
}

/// Darwin refused the cache at open and has nothing left to drop; platforms
/// with no hint at all never had anything to drop either.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn release_from_the_page_cache(_file: &File) {}

/// Returns `true` when `file` holds at least one byte and every one of them
/// lives in a hole - a range the filesystem never allocated, which reads back
/// as zeroes without any of it existing on disk.
///
/// Such a file is all zeroes by definition, and answering that way costs one
/// `lseek` against metadata already in memory instead of a read per block. A
/// filesystem that cannot answer the question - one with no sparse support at
/// all, or a network mount - reports `false`, which is not a wrong answer but a
/// deferral: the caller reads the file the ordinary way and decides for itself.
///
/// The read position is left where it was found, so a caller may treat this as
/// a pure question about the file.
#[cfg(unix)]
fn whole_file_is_a_hole(file: &File) -> bool {
    use std::os::unix::io::AsRawFd;

    // SAFETY: the descriptor is borrowed from a File that outlives this call,
    // and lseek touches nothing but that descriptor's own offset.
    let first_data = unsafe { libc::lseek(file.as_raw_fd(), 0, libc::SEEK_DATA) };

    if first_data >= 0 {
        // There is data somewhere, so the file has to be read. The lseek left
        // the offset sitting on it; a leading hole is zeroes the read would
        // have to see anyway, so give the caller back the file it handed in.
        if first_data > 0 {
            // SAFETY: as above.
            unsafe { libc::lseek(file.as_raw_fd(), 0, libc::SEEK_SET) };
        }
        return false;
    }

    // ENXIO is the single error that answers the question: no data at or after
    // offset zero. Everything else - EINVAL, ENOTSUP, a filesystem with no
    // concept of holes - means the question went unanswered, and an unanswered
    // question is not a yes.
    if io::Error::last_os_error().raw_os_error() != Some(libc::ENXIO) {
        return false;
    }

    // An empty file reports ENXIO for the same reason a sparse one does, and
    // zth counts a file with no bytes as having no zero bytes either. This is
    // the only branch that needs the size, so the fstat stays off the path
    // every ordinary file takes.
    file.metadata().is_ok_and(|metadata| metadata.len() > 0)
}

/// Sparse-file interrogation needs `lseek(SEEK_DATA)`, which is a POSIX
/// extension. Elsewhere every file is read the ordinary way.
#[cfg(not(unix))]
fn whole_file_is_a_hole(_file: &File) -> bool {
    false
}

/// How many files are read concurrently. Always at least one.
///
/// Scanning is dominated by waiting on the storage device, so the useful range
/// runs well past the core count on network or spinning-rust volumes and sits
/// near it on NVMe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Jobs(NonZeroUsize);

impl Jobs {
    /// Builds a worker count, clamping `count` up to one.
    ///
    /// Zero workers would mean nothing ever reads the discovered files, so it is
    /// treated as a request for the minimum rather than as an error.
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self(NonZeroUsize::new(count).unwrap_or(NonZeroUsize::MIN))
    }

    /// The machine's available parallelism, or one when it cannot be determined.
    #[must_use]
    pub fn available_parallelism() -> Self {
        Self(thread::available_parallelism().unwrap_or(NonZeroUsize::MIN))
    }

    /// The number of workers, guaranteed non-zero.
    #[must_use]
    pub fn get(self) -> usize {
        self.0.get()
    }
}

impl Default for Jobs {
    /// Defaults to [`Jobs::available_parallelism`].
    fn default() -> Self {
        Self::available_parallelism()
    }
}

/// Receives running totals while [`find_all_zero_files`] works.
///
/// Both methods are called from the scan's own threads and often - once per
/// file - so implementations must be cheap and must not block.
///
/// Each total is delivered in order and never repeats a value or goes
/// backwards, even though several worker threads produce them. Progress bars
/// depend on that: indicatif reads a backwards position as a seek and throws
/// away the estimate it has been building. Calls to the two methods still
/// interleave freely with each other, since discovery and scanning run at the
/// same time.
///
/// An implementation that panics aborts the scan: whichever of the scan's
/// threads the panic fires in, [`find_all_zero_files`] re-raises it to its
/// caller rather than returning the matches the surviving threads happened to
/// collect. A result that is quietly missing files is worse than no result.
pub trait ScanProgress: Sync {
    /// Called once per file the directory walk turns up, with the running total
    /// of files discovered so far.
    ///
    /// This total is the scan's denominator, and it keeps growing while the walk
    /// is still running.
    fn files_discovered(&self, total: u64);

    /// Called once per file a worker finishes with - whether it matched, did not
    /// match, or could not be read - with the running total of files scanned.
    ///
    /// This total is the scan's numerator, and it always ends up equal to the
    /// final discovered total.
    fn files_scanned(&self, total: u64);
}

/// A [`ScanProgress`] that discards every update, for callers with nothing to
/// display.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProgress;

impl ScanProgress for NoProgress {
    fn files_discovered(&self, _total: u64) {}
    fn files_scanned(&self, _total: u64) {}
}

/// Recursively finds every file under `root` that is non-empty and contains
/// nothing but zero bytes, returning their absolute paths in sorted order.
///
/// One thread walks the tree while `jobs` workers read the files it turns up, so
/// discovery keeps running - and `progress` keeps hearing about it - while the
/// reads are still in flight. `root` may name a file rather than a directory,
/// in which case only that file is scanned.
///
/// Symlinks are never followed: the tree could otherwise escape `root` entirely,
/// or lead a worker into an endless read of something like `/dev/zero`.
///
/// Every I/O error - an unreadable directory, a vanished file, a permission
/// denial, a missing `root` - is silently skipped. Nothing is printed, and the
/// scan carries on.
///
/// # Panics
///
/// Panics if `progress` panics. The scan's own threads are the ones calling it,
/// so a panicking observer is re-raised here - with its original message and
/// location - once the remaining threads have wound down. The alternative is
/// returning whatever the surviving threads collected, and since the return
/// type is a bare `Vec` the caller could not tell that apart from a complete
/// answer. Which thread the observer panicked in makes no difference to this.
#[must_use]
pub fn find_all_zero_files(root: &Path, jobs: Jobs, progress: &dyn ScanProgress) -> Vec<PathBuf> {
    // Resolving the root once is what makes every result absolute: walkdir hands
    // back paths built by joining onto the root it was given.
    let root = absolute_root(root);

    // Unbounded on purpose. A bounded queue would stall the walk whenever the
    // workers fell behind, which is exactly when an honest "files discovered"
    // total - and therefore an honest time estimate - matters most. The queue
    // holds paths, so its high-water mark is bounded by how far the walk can run
    // ahead of the reads.
    let (sender, receiver) = crossbeam::channel::unbounded::<PathBuf>();

    // Guarded rather than atomic so the count and the callback that announces it
    // happen together: workers report from several threads, and an observer that
    // sees totals out of order sees the scan going backwards.
    let scanned = Mutex::new(0_u64);

    let mut found = thread::scope(|scope| {
        let walk_root = &root;

        let walker = scope.spawn(move || {
            // Dropping this sender at the end of the walk is what tells the
            // workers there is nothing left to come, so it is moved in here.
            let sender = sender;

            // The walk is this one thread, so its running total needs nothing
            // more than a local counter to stay in order.
            let mut discovered = 0_u64;

            for entry in WalkDir::new(walk_root)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
            {
                // Directories have no contents to read, and a symlink is left
                // alone rather than followed.
                if !entry.file_type().is_file() {
                    continue;
                }

                discovered = discovered.saturating_add(1);
                progress.files_discovered(discovered);

                if sender.send(entry.into_path()).is_err() {
                    // Every worker is gone; nothing will read what we send.
                    break;
                }
            }
        });

        let workers: Vec<_> = (0..jobs.get())
            .map(|_| {
                let receiver = receiver.clone();
                let scanned = &scanned;

                scope.spawn(move || {
                    // One buffer per worker, reused for every file it reads.
                    let mut buffer = vec![0_u8; READ_BUFFER_LEN];
                    let mut matches = Vec::new();

                    for path in receiver {
                        // An unreadable file is not a match, and not a reason to
                        // say anything about it.
                        if file_is_all_zeroes_with_buffer(&path, &mut buffer).unwrap_or(false) {
                            matches.push(path);
                        }

                        // Counting and announcing under one lock is what keeps
                        // the totals in order. An observer that panics poisons
                        // the mutex; recovering the count is better than taking
                        // every other worker down with it.
                        let mut total = scanned.lock().unwrap_or_else(PoisonError::into_inner);
                        *total = total.saturating_add(1);
                        progress.files_scanned(*total);
                    }

                    matches
                })
            })
            .collect();

        // The clone each worker took is the one that matters; this one would
        // otherwise keep the channel alive for no reason.
        drop(receiver);

        // A panicking observer is carried out of the scan rather than absorbed.
        // Swallowing a worker's panic would also swallow every match that
        // worker had accumulated, and this function returns a bare Vec: a
        // truncated answer is indistinguishable from a complete one, so the
        // only honest reply to a broken observer is the panic itself. Which
        // thread it fired in must not change that, so the walker is joined the
        // same way. Re-raising the original payload keeps the observer's own
        // message and location intact.
        //
        // Doing this from inside the scope is safe: thread::scope catches the
        // closure's unwind, waits for every thread still running, and only then
        // re-raises. Nothing here can deadlock waiting on them - the channel is
        // unbounded, so the walker never blocks on the workers, and no lock is
        // held across the joins.
        let mut matches = Vec::new();
        for worker in workers {
            matches.extend(worker.join().unwrap_or_else(|panic| resume_unwind(panic)));
        }
        walker.join().unwrap_or_else(|panic| resume_unwind(panic));

        matches
    });

    found.sort_unstable();
    found
}

/// Resolves `root` to an absolute path so every path built from it is absolute.
///
/// Canonicalizing also collapses `..` and resolves symlinked parents, which is
/// what a user pasting a result back into another command wants. A root that
/// cannot be canonicalized - most often because it does not exist - still gets
/// made absolute so the walk can fail on it normally and report nothing.
fn absolute_root(root: &Path) -> PathBuf {
    std::fs::canonicalize(root)
        .or_else(|_| std::path::absolute(root))
        .unwrap_or_else(|_| root.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// A reader that yields zero bytes and records the size of every request it
    /// is handed.
    ///
    /// Reading is the expensive half of a scan on a spinning disk, so how many
    /// bytes the scanner asks for - not just what it concludes - is behavior
    /// worth pinning down.
    struct RecordingReader {
        remaining: usize,
        first_byte: u8,
        position: usize,
        requests: Vec<usize>,
    }

    impl RecordingReader {
        /// A reader with `remaining` zero bytes to give out.
        fn zeroes(remaining: usize) -> Self {
            Self {
                remaining,
                first_byte: 0,
                position: 0,
                requests: Vec::new(),
            }
        }

        /// A reader whose very first byte is non-zero and whose rest is zeroes.
        fn rejected_at_the_first_byte(remaining: usize) -> Self {
            Self {
                remaining,
                first_byte: 0xFF,
                position: 0,
                requests: Vec::new(),
            }
        }

        /// Total bytes the scanner asked this reader for.
        fn bytes_requested(&self) -> usize {
            self.requests.iter().sum()
        }
    }

    impl Read for RecordingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.requests.push(buffer.len());

            let filled = buffer.len().min(self.remaining);
            buffer[..filled].fill(0);
            if self.position == 0 && filled > 0 {
                buffer[0] = self.first_byte;
            }

            self.remaining -= filled;
            self.position += filled;
            Ok(filled)
        }
    }

    /// The whole point of the probe: a file that is disqualified by its first
    /// byte - the overwhelming majority of them - must not drag a full buffer
    /// off the platter to prove it.
    #[test]
    fn a_rejected_file_costs_no_more_than_the_probe() {
        let mut reader = RecordingReader::rejected_at_the_first_byte(READ_BUFFER_LEN * 4);
        let mut buffer = vec![0_u8; READ_BUFFER_LEN];

        let all_zeroes = reader_is_all_zeroes(&mut reader, &mut buffer)
            .expect("an in-memory reader should not fail");

        assert!(!all_zeroes, "a leading 0xFF byte disqualifies the reader");
        assert_eq!(
            reader.bytes_requested(),
            PROBE_READ_LEN,
            "rejecting a file must read the probe and nothing more"
        );
    }

    #[test]
    fn the_first_read_only_asks_for_the_probe() {
        let mut reader = RecordingReader::zeroes(READ_BUFFER_LEN * 4);
        let mut buffer = vec![0_u8; READ_BUFFER_LEN];

        let _ = reader_is_all_zeroes(&mut reader, &mut buffer)
            .expect("an in-memory reader should not fail");

        assert_eq!(
            reader.requests.first().copied(),
            Some(PROBE_READ_LEN),
            "the first read is a probe, not a full buffer"
        );
    }

    /// Once the probe comes back clean the file is very likely all zeroes, so
    /// there is nothing left to be cautious about - go as wide as the buffer
    /// allows and let the reads run sequentially.
    #[test]
    fn surviving_the_probe_escalates_to_the_full_buffer() {
        let mut reader = RecordingReader::zeroes(READ_BUFFER_LEN * 4);
        let mut buffer = vec![0_u8; READ_BUFFER_LEN];

        let all_zeroes = reader_is_all_zeroes(&mut reader, &mut buffer)
            .expect("an in-memory reader should not fail");

        assert!(all_zeroes, "the reader yields nothing but zeroes");
        assert_eq!(
            reader.requests.get(1).copied(),
            Some(READ_BUFFER_LEN),
            "reads after the probe should use the whole buffer"
        );
    }

    #[test]
    fn a_buffer_smaller_than_the_probe_is_not_overrun() {
        let mut reader = RecordingReader::zeroes(64);
        let mut buffer = vec![0_u8; 8];

        let all_zeroes = reader_is_all_zeroes(&mut reader, &mut buffer)
            .expect("an in-memory reader should not fail");

        assert!(all_zeroes, "a short buffer still sees nothing but zeroes");
        assert!(
            reader.requests.iter().all(|&request| request <= 8),
            "no read may ask for more than the caller's buffer holds, got {:?}",
            reader.requests
        );
    }

    /// Writes `contents` to a uniquely-named file inside a fresh temp dir.
    ///
    /// The returned [`TempDir`] owns the directory: hold onto it for as long as
    /// the path is used, or the fixture is deleted out from under the test.
    fn file_with(contents: &[u8]) -> (TempDir, PathBuf) {
        file_named("data.bin", contents)
    }

    /// Same as [`file_with`], but lets the test pick the file name.
    fn file_named(name: &str, contents: &[u8]) -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("creating a temp dir should succeed");
        let path = dir.path().join(name);
        fs::write(&path, contents).expect("writing the fixture should succeed");
        (dir, path)
    }

    /// Unwraps a scan that is expected to complete without an I/O error.
    fn scan(path: &Path) -> bool {
        file_is_all_zeroes(path).expect("reading the fixture should succeed")
    }

    #[test]
    fn all_zero_file_is_reported() {
        let (_dir, path) = file_with(&[0_u8; 1024]);
        assert!(scan(&path), "a 1 KiB block of zeroes is all zeroes");
    }

    #[test]
    fn single_zero_byte_file_is_reported() {
        let (_dir, path) = file_with(&[0_u8]);
        assert!(scan(&path), "one zero byte is still all zeroes");
    }

    #[test]
    fn empty_file_is_not_reported() {
        let (_dir, path) = file_with(&[]);
        assert!(!scan(&path), "an empty file has no zero bytes to match");
    }

    #[test]
    fn leading_non_zero_byte_is_rejected() {
        let mut contents = vec![0_u8; 4096];
        contents[0] = 1;
        let (_dir, path) = file_with(&contents);
        assert!(!scan(&path), "a non-zero first byte disqualifies the file");
    }

    #[test]
    fn trailing_non_zero_byte_is_rejected() {
        let mut contents = vec![0_u8; 4096];
        contents[4095] = 0xFF;
        let (_dir, path) = file_with(&contents);
        assert!(!scan(&path), "a non-zero last byte disqualifies the file");
    }

    #[test]
    fn all_zero_file_larger_than_the_read_buffer_is_reported() {
        let (_dir, path) = file_with(&vec![0_u8; READ_BUFFER_LEN * 2 + 1]);
        assert!(scan(&path), "zeroes spanning several reads are all zeroes");
    }

    #[test]
    fn non_zero_byte_at_the_end_of_the_first_read_is_rejected() {
        let mut contents = vec![0_u8; READ_BUFFER_LEN * 2];
        contents[READ_BUFFER_LEN - 1] = 1;
        let (_dir, path) = file_with(&contents);
        assert!(
            !scan(&path),
            "the last byte of a full buffer must be tested"
        );
    }

    #[test]
    fn non_zero_byte_at_the_end_of_the_probe_is_rejected() {
        let mut contents = vec![0_u8; PROBE_READ_LEN * 2];
        contents[PROBE_READ_LEN - 1] = 1;
        let (_dir, path) = file_with(&contents);
        assert!(
            !scan(&path),
            "the last byte the probe covers must still be tested"
        );
    }

    #[test]
    fn non_zero_byte_just_past_the_probe_is_rejected() {
        let mut contents = vec![0_u8; PROBE_READ_LEN * 2];
        contents[PROBE_READ_LEN] = 1;
        let (_dir, path) = file_with(&contents);
        assert!(
            !scan(&path),
            "escalating past the probe must not skip the byte it stopped on"
        );
    }

    #[test]
    fn an_all_zero_file_exactly_the_size_of_the_probe_is_reported() {
        let (_dir, path) = file_with(&vec![0_u8; PROBE_READ_LEN]);
        assert!(
            scan(&path),
            "a file that ends exactly where the probe does is still all zeroes"
        );
    }

    #[test]
    fn non_zero_byte_on_a_read_buffer_boundary_is_rejected() {
        let mut contents = vec![0_u8; READ_BUFFER_LEN * 2];
        contents[READ_BUFFER_LEN] = 1;
        let (_dir, path) = file_with(&contents);
        assert!(
            !scan(&path),
            "the first byte of a later buffer must be tested"
        );
    }

    #[test]
    fn file_shorter_than_the_read_buffer_only_tests_the_bytes_it_has() {
        let mut contents = vec![0_u8; 7];
        contents[6] = 9;
        let (_dir, path) = file_with(&contents);
        assert!(
            !scan(&path),
            "a partial final read must not be padded with the buffer's stale zeroes"
        );
    }

    #[test]
    fn multi_byte_file_name_is_handled() {
        let (_dir, path) = file_named("日本語-🎉-café.bin", &[0_u8; 64]);
        assert!(scan(&path), "non-ASCII file names are ordinary paths");
    }

    #[test]
    fn jobs_preserves_the_requested_worker_count() {
        assert_eq!(Jobs::new(7).get(), 7, "a requested worker count is honored");
    }

    #[test]
    fn jobs_clamps_zero_workers_up_to_one() {
        assert_eq!(
            Jobs::new(0).get(),
            1,
            "zero workers would leave discovered files unread"
        );
    }

    #[test]
    fn jobs_defaults_to_available_parallelism() {
        let expected = thread::available_parallelism().map_or(1, NonZeroUsize::get);
        assert_eq!(
            Jobs::default().get(),
            expected,
            "the default should use every core the machine reports"
        );
    }

    /// Creates a file of `len` bytes that was never written to, so the
    /// filesystem backs it with a hole rather than with blocks of zeroes.
    #[cfg(unix)]
    fn sparse_file(len: u64) -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("creating a temp dir should succeed");
        let path = dir.path().join("sparse.bin");
        File::create(&path)
            .expect("creating the fixture should succeed")
            .set_len(len)
            .expect("extending the fixture should succeed");
        (dir, path)
    }

    /// Opens a fixture for the hole tests.
    #[cfg(unix)]
    fn open(path: &Path) -> File {
        File::open(path).expect("opening the fixture should succeed")
    }

    /// The flag `F_GETFL` reports back once `F_NOCACHE` has been set on a
    /// descriptor.
    ///
    /// xnu calls it `FNOCACHE`. It is not one of the `open(2)` flags, so `libc`
    /// gives it no name, but `fcntl(F_GETFL)` returns it and that is what makes
    /// the hint observable rather than merely intended.
    #[cfg(target_vendor = "apple")]
    const FNOCACHE_FLAG: libc::c_int = 0x0004_0000;

    #[cfg(target_vendor = "apple")]
    #[test]
    fn a_file_opened_for_scanning_is_kept_out_of_the_page_cache() {
        use std::os::unix::io::AsRawFd;

        let (_dir, path) = file_with(&[0_u8; 4096]);
        let scanned = ScanFile::open(&path).expect("opening the fixture should succeed");

        // SAFETY: the descriptor is borrowed from a ScanFile that outlives the
        // call, and F_GETFL only reads the descriptor's flags.
        let flags = unsafe { libc::fcntl(scanned.file.as_raw_fd(), libc::F_GETFL) };

        assert!(flags >= 0, "F_GETFL should succeed on an open file");
        assert_ne!(
            flags & FNOCACHE_FLAG,
            0,
            "a scanned file's pages are read once and never wanted again, \
             so the descriptor must be marked uncached"
        );
    }

    /// Uncached reads are the whole point, but they are worth nothing if they
    /// come back different. This is the guard on the hint: same bytes, same
    /// short-read behavior, across a length that is not a multiple of anything.
    #[test]
    fn a_file_opened_for_scanning_reads_back_every_byte() {
        let contents: Vec<u8> = (0..10_007_u32).map(|byte| (byte % 251) as u8).collect();
        let (_dir, path) = file_with(&contents);

        let mut scanned = ScanFile::open(&path).expect("opening the fixture should succeed");
        let mut read_back = Vec::new();
        scanned
            .read_to_end(&mut read_back)
            .expect("reading the fixture should succeed");

        assert_eq!(
            read_back, contents,
            "an uncached read must return exactly what a cached one would"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_wholly_unwritten_file_is_recognized_as_a_hole() {
        let (_dir, path) = sparse_file(1 << 20);
        assert!(
            whole_file_is_a_hole(&open(&path)),
            "a 1 MiB file that was never written has no data anywhere in it"
        );
    }

    /// An empty file reports "no data at or after offset zero" exactly as a
    /// wholly-sparse one does, and zth does not count it as all zeroes - so the
    /// fast path has to tell the two apart rather than trusting the `lseek`.
    #[cfg(unix)]
    #[test]
    fn an_empty_file_is_not_a_hole() {
        let (_dir, path) = file_with(&[]);
        assert!(
            !whole_file_is_a_hole(&open(&path)),
            "an empty file has no zero bytes, so it must not take the fast path"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_file_of_written_zeroes_is_not_a_hole() {
        let (_dir, path) = file_with(&vec![0_u8; 1 << 20]);
        assert!(
            !whole_file_is_a_hole(&open(&path)),
            "zeroes that were actually written occupy blocks, not a hole"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_file_with_content_is_not_a_hole() {
        let (_dir, path) = file_with(&[7_u8; 4096]);
        assert!(
            !whole_file_is_a_hole(&open(&path)),
            "a file with content must be read, not waved through"
        );
    }

    /// The fast path runs before the read, so anything it does to the file
    /// position is something the read inherits.
    #[cfg(unix)]
    #[test]
    fn asking_about_a_hole_leaves_the_read_position_at_the_start() {
        let (_dir, path) = file_with(&[1_u8, 2, 3, 4]);
        let mut file = open(&path);

        let _ = whole_file_is_a_hole(&file);

        let mut contents = Vec::new();
        file.read_to_end(&mut contents)
            .expect("reading the fixture should succeed");
        assert_eq!(
            contents,
            vec![1_u8, 2, 3, 4],
            "the question must not consume any of the file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_wholly_unwritten_file_is_reported_as_all_zeroes() {
        let (_dir, path) = sparse_file(1 << 20);
        assert!(
            scan(&path),
            "a hole reads back as zeroes, so a file that is nothing but hole is all zeroes"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritten_file_of_zero_length_is_not_reported() {
        let (_dir, path) = sparse_file(0);
        assert!(
            !scan(&path),
            "a zero-length file is empty however it was created"
        );
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = TempDir::new().expect("creating a temp dir should succeed");
        let missing = dir.path().join("definitely-not-here.bin");
        assert!(
            file_is_all_zeroes(&missing).is_err(),
            "a missing file must surface as an error, not as a false negative"
        );
    }
}
