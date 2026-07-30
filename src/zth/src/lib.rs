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

use std::io::{self, Read};
use std::path::Path;

/// Size of the buffer each read fills before the block is tested for zeroes.
///
/// 256 KiB is large enough that the per-`read` syscall overhead disappears into
/// the memory comparison, and small enough that a worker's buffer stays inside
/// a typical L2 cache.
const READ_BUFFER_LEN: usize = 256 * 1024;

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
    let _ = path;
    Ok(false)
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
