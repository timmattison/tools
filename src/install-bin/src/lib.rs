//! install-bin — install a locally built binary without tripping macOS's
//! code-signature cache.
//!
//! Why this exists: on Apple Silicon macOS, copying over an *existing* binary
//! reuses the destination inode, and the kernel caches code signatures per
//! vnode. The cache still holds the old build's signature, so every exec of the
//! new bytes dies with SIGKILL (Code Signature Invalid). The fix is to unlink
//! the destination before copying so the installed file always lands on a fresh
//! inode the kernel has never cached.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Outcome of a successful [`install_binary`] call.
pub struct InstallResult {
    /// The path the binary was installed to.
    pub dest: PathBuf,
    /// Whether an existing file at `dest` was replaced (always onto a fresh
    /// inode — the old file is unlinked, never overwritten in place).
    pub replaced_existing: bool,
}

/// Errors that can occur while installing a binary.
#[derive(Debug, Error)]
pub enum InstallError {
    /// The source path does not exist.
    #[error("source binary does not exist: {0}")]
    SourceMissing(PathBuf),
    /// The source path exists but is not a regular file.
    #[error("source is not a regular file: {0}")]
    SourceNotRegularFile(PathBuf),
    /// The source and destination resolve to the same file — installing would
    /// destroy the source.
    #[error("source and destination are the same file: {0}")]
    SameFile(PathBuf),
}

/// Copy `source` to `dest` such that `dest` always ends up on a fresh inode:
/// the destination is unlinked first, never overwritten in place. Creates the
/// destination directory if needed and carries over the source's file mode.
///
/// # Errors
///
/// Returns [`InstallError::SourceMissing`] if `source` does not exist,
/// [`InstallError::SourceNotRegularFile`] if `source` is not a regular file,
/// and [`InstallError::SameFile`] if `source` and `dest` resolve to the same
/// file (which would otherwise destroy the source).
pub fn install_binary(source: &Path, dest: &Path) -> Result<InstallResult, InstallError> {
    let _ = (source, dest);
    todo!("install_binary behavior not yet implemented")
}
