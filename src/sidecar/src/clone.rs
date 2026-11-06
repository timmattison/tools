//! The main clone of a sidecar.
//!
//! A main clone is made in a scratch directory and moved into place with one rename.
//! A main clone thus never appears half made, and the loser of two setups at the same
//! time uses the clone of the winner.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::git::{Git, MAIN_BRANCH};
use crate::remote::same_repository;

/// What is at the place of a main clone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloneState {
    /// Nothing is there.
    Missing,
    /// A complete clone of the sidecar is there.
    Ready,
    /// A clone of the sidecar is there, but it did not end: `HEAD` names no commit,
    /// or the index is missing. Another setup can clone it now.
    Incomplete,
    /// Something else is there.
    Foreign {
        /// Why the directory is not a clone of the sidecar.
        reason: String,
    },
}

/// What `ensure_clone` found or did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloneOutcome {
    /// A complete clone was there already.
    Present,
    /// This run made the clone.
    Made,
    /// A clone that did not end is there.
    Incomplete,
    /// Something else is there.
    Foreign {
        /// Why the directory is not a clone of the sidecar.
        reason: String,
    },
    /// The clone failed and nothing usable is there.
    Failed {
        /// The error of the clone.
        reason: String,
    },
}

/// Finds out what is at `place`. `url` is the URL of the sidecar.
#[must_use]
pub fn inspect_clone(git: &Git, url: &str, place: &Path) -> CloneState {
    match std::fs::symlink_metadata(place) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CloneState::Missing;
        }
        Err(error) => {
            return CloneState::Foreign {
                reason: format!("it cannot be read: {error}"),
            };
        }
    }
    let top = git.run(place, ["rev-parse", "--show-toplevel"]);
    let is_top = top.ok
        && matches!(
            (
                std::fs::canonicalize(&top.stdout),
                std::fs::canonicalize(place)
            ),
            (Ok(found), Ok(expected)) if found == expected
        );
    if !is_top {
        return CloneState::Foreign {
            reason: "it is not the top of a git repository.".to_string(),
        };
    }
    let origin = git.run(place, ["remote", "get-url", "origin"]);
    if !origin.ok {
        return CloneState::Foreign {
            reason: format!("it has no origin: {}", origin.stderr),
        };
    }
    if !same_repository(&origin.stdout, url) {
        return CloneState::Foreign {
            reason: format!("its origin is {}.", origin.stdout),
        };
    }
    let commit = git.run(place, ["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]);
    let index = git.run(
        place,
        ["rev-parse", "--path-format=absolute", "--git-path", "index"],
    );
    if !commit.ok || !index.ok || std::fs::symlink_metadata(&index.stdout).is_err() {
        return CloneState::Incomplete;
    }
    CloneState::Ready
}

/// Makes the main clone at `place` when it is missing. The clone is on `main`.
#[must_use]
pub fn ensure_clone(git: &Git, url: &str, place: &Path) -> CloneOutcome {
    ensure_clone_with(git, url, place, |temp| {
        let parent = temp.parent().unwrap_or(temp);
        let result = git.run(
            parent,
            [
                std::ffi::OsStr::new("clone"),
                "--quiet".as_ref(),
                "--branch".as_ref(),
                MAIN_BRANCH.as_ref(),
                url.as_ref(),
                temp.as_os_str(),
            ],
        );
        if result.ok {
            Ok(())
        } else {
            Err(result.stderr)
        }
    })
}

/// The form of `ensure_clone` that takes the clone step as a parameter, so a test can
/// act in the gap between the check and the rename.
#[doc(hidden)]
#[must_use]
pub fn ensure_clone_with(
    git: &Git,
    url: &str,
    place: &Path,
    clone: impl FnOnce(&Path) -> Result<(), String>,
) -> CloneOutcome {
    match inspect_clone(git, url, place) {
        CloneState::Missing => {}
        CloneState::Ready => return CloneOutcome::Present,
        CloneState::Incomplete => return CloneOutcome::Incomplete,
        CloneState::Foreign { reason } => return CloneOutcome::Foreign { reason },
    }
    let temp = scratch_path(place);
    match clone(&temp) {
        Ok(()) => match std::fs::rename(&temp, place) {
            Ok(()) => CloneOutcome::Made,
            Err(error) => CloneOutcome::Failed {
                reason: error.to_string(),
            },
        },
        Err(reason) => CloneOutcome::Failed { reason },
    }
}

/// The scratch path of one clone: `<parent>/.<name>.sidecar-clone-<pid>-<nanos>`.
fn scratch_path(place: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let name = place
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    place.with_file_name(format!(
        ".{name}.sidecar-clone-{}-{nanos}",
        std::process::id()
    ))
}
