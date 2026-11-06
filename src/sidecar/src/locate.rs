//! The path of a sidecar in the worktree that the user runs in.
//!
//! `sidecar path <name>` is the helper for scripts and rules. It reads the link and
//! prints the path of the link. It makes no directory and no network call.

use std::path::PathBuf;

use crate::host::Host;

/// Why `locate` gives no path.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LocateError {
    /// The registry does not name the sidecar, or the name is not a sidecar name.
    #[error("sidecar.toml does not name the sidecar \"{name}\". Run `sidecar add {name}`.")]
    NotRegistered {
        /// The name as the user typed it.
        name: String,
    },
    /// The link is missing.
    #[error("{shown} is missing. Run `sidecar setup`.")]
    Missing {
        /// The link as a person reads it.
        shown: String,
    },
    /// The link points at a path that does not exist.
    #[error("{shown} is a broken link. Run `sidecar setup`.")]
    Broken {
        /// The link as a person reads it.
        shown: String,
    },
    /// Something else is at the place of the link.
    #[error(
        "{shown} is not a link to {} or to a worktree of it. Move it away, and run `sidecar setup`.",
        main_clone.display()
    )]
    Foreign {
        /// The link as a person reads it.
        shown: String,
        /// The main clone of the sidecar.
        main_clone: PathBuf,
    },
}

/// Finds the link of the sidecar `raw_name` in the worktree of `host`. The answer is
/// the absolute path of the link, not of its target.
///
/// # Errors
///
/// Returns a [`LocateError`] when the registry does not name the sidecar, or when the
/// link is missing, broken, or foreign.
pub fn locate(host: &Host, raw_name: &str) -> Result<PathBuf, LocateError> {
    let _ = host;
    Err(LocateError::NotRegistered {
        name: raw_name.to_string(),
    })
}
