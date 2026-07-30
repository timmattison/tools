//! Zero the Hero - locate files whose contents are nothing but zero bytes.
//!
//! [`find_all_zero_files`] is the whole tool in one call: point it at a path and
//! it walks the tree, reads every file it finds, and hands back the absolute,
//! sorted paths of the ones that are non-empty and entirely zeroes. Discovery
//! and reading overlap, every I/O error is silently skipped, and a
//! [`ScanProgress`] observer sees the running totals as they change.
//!
//! [`file_is_all_zeroes`] answers the same question for a single file. It stops
//! at the first non-zero byte rather than reading or hashing the whole file.
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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use walkdir::WalkDir;

/// Size of the buffer each read fills before the block is tested for zeroes.
///
/// 256 KiB is large enough that the per-`read` syscall overhead disappears into
/// the memory comparison, and small enough that a worker's buffer stays inside
/// a typical L2 cache.
const READ_BUFFER_LEN: usize = 256 * 1024;

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
fn reader_is_all_zeroes(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<bool> {
    let mut saw_bytes = false;

    loop {
        let filled = match reader.read(buffer) {
            Ok(0) => break,
            Ok(filled) => filled,
            // A signal arriving mid-read is not a failure of the file.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };

        saw_bytes = true;

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
    let mut file = File::open(path)?;
    reader_is_all_zeroes(&mut file, buffer)
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
        Self(std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN))
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
/// Both methods are called from the scan's own threads, from any of them, and
/// often - once per file. Implementations must be cheap and must not block.
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

    let discovered = AtomicU64::new(0);
    let scanned = AtomicU64::new(0);

    let mut found = thread::scope(|scope| {
        let walk_root = &root;
        let discovered = &discovered;

        scope.spawn(move || {
            // Dropping this sender at the end of the walk is what tells the
            // workers there is nothing left to come, so it is moved in here.
            let sender = sender;

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

                let total = discovered.fetch_add(1, Ordering::Relaxed).saturating_add(1);
                progress.files_discovered(total);

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

                        let total = scanned.fetch_add(1, Ordering::Relaxed).saturating_add(1);
                        progress.files_scanned(total);
                    }

                    matches
                })
            })
            .collect();

        // The clone each worker took is the one that matters; this one would
        // otherwise keep the channel alive for no reason.
        drop(receiver);

        workers
            .into_iter()
            .filter_map(|worker| worker.join().ok())
            .flatten()
            .collect::<Vec<_>>()
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
        let expected = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        assert_eq!(
            Jobs::default().get(),
            expected,
            "the default should use every core the machine reports"
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
