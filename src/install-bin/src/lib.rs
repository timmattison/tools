//! install-bin — install a locally built binary without tripping macOS's
//! code-signature cache.
//!
//! Why this exists: on Apple Silicon macOS, copying over an *existing* binary
//! reuses the destination inode, and the kernel caches code signatures per
//! vnode. The cache still holds the old build's signature, so every exec of the
//! new bytes dies with SIGKILL (Code Signature Invalid). The fix is to copy the
//! new bytes to a temp file in the destination directory and then atomically
//! `rename` it over the destination: rename swaps in the temp's brand-new inode
//! (which the kernel has never cached) and, being atomic, never leaves the
//! destination missing or half-written — a failed copy leaves any existing
//! destination untouched.

use std::fs::{self, Permissions};
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use thiserror::Error;
use wait_timeout::ChildExt;

/// Outcome of a successful [`install_binary`] call.
pub struct InstallResult {
    /// The path the binary was installed to.
    pub dest: PathBuf,
    /// Whether an existing file at `dest` was replaced (always onto a fresh
    /// inode — the atomic rename swaps in the temp file's fresh inode, never
    /// overwriting the old file in place).
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
    /// An underlying filesystem operation failed.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Copy `source` to `dest` such that `dest` always ends up on a fresh inode:
/// the bytes are written to a temp file in the destination directory and then
/// atomically renamed over `dest`, giving `dest` the temp's brand-new inode
/// rather than overwriting the old file in place. The rename is atomic, so a
/// failed copy leaves any existing `dest` untouched. Creates the destination
/// directory if needed and carries over the source's file mode.
///
/// # Errors
///
/// Returns [`InstallError::SourceMissing`] if `source` does not exist,
/// [`InstallError::SourceNotRegularFile`] if `source` exists but is not a
/// regular file, [`InstallError::SameFile`] if `source` and `dest` resolve to
/// the same file (which would otherwise destroy the source), or
/// [`InstallError::Io`] if an underlying filesystem operation (copy, permission
/// change, or rename) fails.
pub fn install_binary(source: &Path, dest: &Path) -> Result<InstallResult, InstallError> {
    let source_meta =
        fs::metadata(source).map_err(|_| InstallError::SourceMissing(source.to_path_buf()))?;
    if !source_meta.file_type().is_file() {
        return Err(InstallError::SourceNotRegularFile(source.to_path_buf()));
    }

    let replaced_existing = dest.exists();

    // Refuse a self-install before any destructive op: if dest already exists
    // and resolves to the same file as source, unlinking dest would destroy the
    // source. Both canonicalizations must succeed for the paths to be equal.
    if replaced_existing {
        if let (Ok(source_real), Ok(dest_real)) = (fs::canonicalize(source), fs::canonicalize(dest))
        {
            if source_real == dest_real {
                return Err(InstallError::SameFile(source_real));
            }
        }
    }

    // Create the destination directory tree if it does not exist yet.
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    // Copy to a temp file in the destination directory, then atomically rename
    // it over dest. rename() gives dest the temp's brand-new inode (the macOS
    // signature-cache fix) AND is atomic — dest is never left missing or
    // partially written, and a failed copy leaves any existing dest untouched.
    let dest_dir = dest.parent().unwrap_or_else(|| Path::new("."));
    let stem = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "install".into());
    // Parallel-safe unique temp name (per CLAUDE.md: key on pid + nanos so
    // concurrent installs never collide).
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dest_dir.join(format!(
        ".{stem}.install-tmp-{}-{nanos}",
        std::process::id()
    ));

    // On any failure, best-effort remove the temp so we don't litter, and leave
    // any existing dest untouched.
    if let Err(err) = fs::copy(source, &tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(InstallError::from(err));
    }
    if let Err(err) = fs::set_permissions(&tmp, Permissions::from_mode(source_meta.mode() & 0o7777))
    {
        let _ = fs::remove_file(&tmp);
        return Err(InstallError::from(err));
    }
    if let Err(err) = fs::rename(&tmp, dest) {
        let _ = fs::remove_file(&tmp);
        return Err(InstallError::from(err));
    }

    Ok(InstallResult {
        dest: dest.to_path_buf(),
        replaced_existing,
    })
}

/// Default timeout for the post-install exec verification performed by
/// [`verify_exec`]: long enough for any real CLI to print `--version`, short
/// enough that a hung binary can't wedge the installer.
pub const DEFAULT_VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// The signal number for `SIGKILL` (9). A `SIGKILL` at exec on macOS is the
/// tell-tale code-signature-cache rejection this whole tool exists to prevent.
const SIGKILL: i32 = 9;

/// Guidance shown when a freshly installed binary is `SIGKILL`ed at exec on
/// macOS, which almost always means the kernel rejected its code signature.
/// Ported verbatim from the `SIGKILL_DARWIN_HINT` constant in the original
/// TypeScript `install-bin`.
const SIGKILL_DARWIN_HINT: &str = "SIGKILL at exec on macOS usually means the kernel rejected the code signature (stale per-vnode signature cache from an in-place overwrite, or an unsigned/modified binary). Reinstalling onto a fresh inode or `codesign -f -s - <path>` fixes it.";

/// The outcome of exec'ing a freshly installed binary once to prove the kernel
/// will actually run it. A normal exit (any code) means exec succeeded — the
/// signature check already passed — so only signal deaths, timeouts, and spawn
/// failures are verdicts against the binary.
#[derive(Debug)]
pub enum ExecVerdict {
    /// The binary exec'd and exited normally with this code. Any exit code
    /// counts as OK because reaching exit at all proves exec (and thus the
    /// signature check) succeeded.
    Ok {
        /// The process's exit code.
        exit_code: i32,
    },
    /// The binary died from a signal. `signal` is the raw signal number
    /// (`9` == `SIGKILL`), and `hint` explains the likely cause.
    Signal {
        /// The raw signal number that killed the process (`9` == `SIGKILL`).
        signal: i32,
        /// Human-readable guidance about the likely cause.
        hint: String,
    },
    /// The binary did not finish within the timeout and was killed.
    Timeout {
        /// Human-readable description of the timeout.
        hint: String,
    },
    /// The binary could not be spawned or waited on.
    SpawnError {
        /// Human-readable description of the spawn/wait failure.
        hint: String,
    },
}

impl ExecVerdict {
    /// Whether the binary exec'd cleanly — i.e. this is an [`ExecVerdict::Ok`].
    #[must_use]
    pub fn is_ok(&self) -> bool {
        matches!(self, ExecVerdict::Ok { .. })
    }

    /// Whether the binary was killed by `SIGKILL` (signal 9). On macOS this is
    /// the tell-tale code-signature-cache rejection, and the CLI uses it to
    /// decide whether to re-sign ad-hoc and retry the exec check once.
    #[must_use]
    pub fn is_sigkill(&self) -> bool {
        matches!(self, ExecVerdict::Signal { signal, .. } if *signal == SIGKILL)
    }
}

/// Exec the installed binary once to prove the kernel will actually run it.
///
/// Spawns `bin arg` with stdio fully redirected to null and waits up to
/// `timeout`. A normal exit (any code) is [`ExecVerdict::Ok`]; a signal death is
/// [`ExecVerdict::Signal`]; exceeding `timeout` kills the child and yields
/// [`ExecVerdict::Timeout`]; a spawn or wait failure yields
/// [`ExecVerdict::SpawnError`].
pub fn verify_exec(bin: &Path, arg: &str, timeout: Duration) -> ExecVerdict {
    // stdin/stdout/stderr all go to null: the verified binary's output must not
    // pollute install-bin's own output, and an unread pipe could otherwise
    // deadlock the timed wait below.
    let mut child = match Command::new(bin)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            return ExecVerdict::SpawnError {
                hint: format!("exec failed: {err}"),
            }
        }
    };

    match child.wait_timeout(timeout) {
        Ok(Some(status)) => match status.signal() {
            Some(sig) => {
                let hint = if sig == SIGKILL && cfg!(target_os = "macos") {
                    SIGKILL_DARWIN_HINT.to_string()
                } else {
                    format!("process died from signal {sig}")
                };
                ExecVerdict::Signal { signal: sig, hint }
            }
            None => ExecVerdict::Ok {
                exit_code: status.code().unwrap_or(0),
            },
        },
        Ok(None) => {
            // The child outlived the timeout: kill it and reap the zombie so the
            // installer doesn't wedge waiting on a binary that never returns.
            let _ = child.kill();
            let _ = child.wait();
            ExecVerdict::Timeout {
                hint: format!("exec did not finish within {timeout:?}"),
            }
        }
        // Waiting on the child itself failed — treat it like a spawn failure
        // rather than silently claiming the binary is fine.
        Err(err) => ExecVerdict::SpawnError {
            hint: format!("waiting on exec failed: {err}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_sigkill_is_true_only_for_signal_nine() {
        assert!(
            ExecVerdict::Signal {
                signal: SIGKILL,
                hint: String::new(),
            }
            .is_sigkill(),
            "a signal-9 death is a SIGKILL"
        );
        assert!(
            !ExecVerdict::Signal {
                signal: 15,
                hint: String::new(),
            }
            .is_sigkill(),
            "SIGTERM (15) is not a SIGKILL"
        );
        assert!(!ExecVerdict::Ok { exit_code: 0 }.is_sigkill());
        assert!(!ExecVerdict::Timeout {
            hint: String::new()
        }
        .is_sigkill());
        assert!(!ExecVerdict::SpawnError {
            hint: String::new()
        }
        .is_sigkill());
    }
}
