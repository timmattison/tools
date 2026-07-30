//! Zero the Hero - locate files whose contents are nothing but zero bytes.
//!
//! The headline entry point is [`file_is_all_zeroes`], which answers the
//! question for a single file while reading as few bytes as it can get away
//! with: it stops at the first non-zero byte instead of hashing or reading the
//! whole file.
//!
//! # Example
//!
//! ```rust,ignore
//! use zth::file_is_all_zeroes;
//! use std::path::Path;
//!
//! if file_is_all_zeroes(Path::new("sparse.img"))? {
//!     println!("nothing but zeroes");
//! }
//! ```

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
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
        assert!(!scan(&path), "the last byte of a full buffer must be tested");
    }

    #[test]
    fn non_zero_byte_on_a_read_buffer_boundary_is_rejected() {
        let mut contents = vec![0_u8; READ_BUFFER_LEN * 2];
        contents[READ_BUFFER_LEN] = 1;
        let (_dir, path) = file_with(&contents);
        assert!(!scan(&path), "the first byte of a later buffer must be tested");
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
    fn missing_file_is_an_error() {
        let dir = TempDir::new().expect("creating a temp dir should succeed");
        let missing = dir.path().join("definitely-not-here.bin");
        assert!(
            file_is_all_zeroes(&missing).is_err(),
            "a missing file must surface as an error, not as a false negative"
        );
    }
}
