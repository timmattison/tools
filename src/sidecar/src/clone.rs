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
    let failure = match clone(&temp) {
        Ok(()) => match std::fs::rename(&temp, place) {
            Ok(()) => return CloneOutcome::Made,
            Err(error) => error.to_string(),
        },
        Err(reason) => reason,
    };
    // The scratch directory is the own work of this run. It is never a main clone.
    let _ = std::fs::remove_dir_all(&temp);
    settle_after_failure(git, url, place, failure)
}

/// A clone step failed. Another run can have made the clone in the meantime, thus
/// look at the place again and use what is usable there.
fn settle_after_failure(git: &Git, url: &str, place: &Path, reason: String) -> CloneOutcome {
    if std::fs::symlink_metadata(place).is_ok() {
        match inspect_clone(git, url, place) {
            CloneState::Ready => return CloneOutcome::Present,
            CloneState::Incomplete => return CloneOutcome::Incomplete,
            CloneState::Missing | CloneState::Foreign { .. } => {}
        }
    }
    CloneOutcome::Failed { reason }
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// The options that give a commit an identity and turn signing off.
    const IDENTITY: [&str; 6] = [
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "-c",
        "commit.gpgsign=false",
    ];

    /// A unique scratch directory with a bare remote that holds one commit on `main`.
    struct Scratch {
        dir: PathBuf,
        remote: PathBuf,
    }

    impl Scratch {
        fn new() -> Scratch {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("the clock is after the epoch")
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "sidecar-clone-test-{}-{nanos}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            let seed = dir.join("seed");
            std::fs::create_dir_all(&seed).expect("make the seed directory");
            let dir = std::fs::canonicalize(&dir).expect("canonicalize the scratch directory");
            let seed = dir.join("seed");
            let git = Git::without_token();
            assert!(git.run(&seed, ["init", "--quiet", "-b", "main"]).ok);
            std::fs::write(seed.join("data.txt"), "1\n").expect("write the seed file");
            assert!(git.run(&seed, ["add", "data.txt"]).ok);
            let mut commit = IDENTITY.to_vec();
            commit.extend(["commit", "--quiet", "-m", "seed"]);
            assert!(git.run(&seed, commit).ok);
            let remote = dir.join("remote.git");
            let cloned = git.run(
                &dir,
                [
                    std::ffi::OsStr::new("clone"),
                    "--quiet".as_ref(),
                    "--bare".as_ref(),
                    seed.as_os_str(),
                    remote.as_os_str(),
                ],
            );
            assert!(cloned.ok, "{cloned:?}");
            Scratch { dir, remote }
        }

        fn url(&self) -> String {
            self.remote.to_string_lossy().into_owned()
        }

        /// A clone of the remote at `place`, like the one that a setup at the same time makes.
        fn clone_to(&self, place: &Path) -> Result<(), String> {
            let result = Git::without_token().run(
                &self.dir,
                [
                    std::ffi::OsStr::new("clone"),
                    "--quiet".as_ref(),
                    "--branch".as_ref(),
                    "main".as_ref(),
                    self.remote.as_os_str(),
                    place.as_os_str(),
                ],
            );
            if result.ok {
                Ok(())
            } else {
                Err(result.stderr)
            }
        }

        /// The names in the directory `dir` that start with a dot.
        fn scratch_names(&self) -> Vec<String> {
            std::fs::read_dir(&self.dir)
                .expect("read the scratch directory")
                .map(|entry| entry.expect("read an entry").file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| name.starts_with('.'))
                .collect()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn rule_7_a_failed_clone_uses_a_clone_that_another_run_made_meanwhile() {
        let scratch = Scratch::new();
        let place = scratch.dir.join("main-clone");
        let outcome = ensure_clone_with(&Git::without_token(), &scratch.url(), &place, |_| {
            scratch.clone_to(&place).expect("the other run clones");
            Err("the clone lost the race".to_string())
        });
        assert_eq!(outcome, CloneOutcome::Present);
    }

    #[test]
    fn the_loser_of_the_rename_uses_the_clone_of_the_winner() {
        let scratch = Scratch::new();
        let place = scratch.dir.join("main-clone");
        let outcome = ensure_clone_with(&Git::without_token(), &scratch.url(), &place, |temp| {
            scratch.clone_to(temp)?;
            scratch.clone_to(&place)
        });
        assert_eq!(outcome, CloneOutcome::Present);
        assert!(place.join("data.txt").is_file());
        assert_eq!(scratch.scratch_names(), Vec::<String>::new());
    }

    #[test]
    fn a_failed_clone_removes_its_scratch_directory() {
        let scratch = Scratch::new();
        let place = scratch.dir.join("main-clone");
        let outcome = ensure_clone_with(&Git::without_token(), &scratch.url(), &place, |temp| {
            std::fs::create_dir_all(temp.join("half")).expect("make half a clone");
            Err("no network".to_string())
        });
        assert_eq!(
            outcome,
            CloneOutcome::Failed {
                reason: "no network".to_string()
            }
        );
        assert!(!place.exists());
        assert_eq!(scratch.scratch_names(), Vec::<String>::new());
    }

    #[test]
    fn a_made_clone_lands_at_the_place_and_leaves_no_scratch() {
        let scratch = Scratch::new();
        let place = scratch.dir.join("日本語-clone");
        let outcome = ensure_clone(&Git::without_token(), &scratch.url(), &place);
        assert_eq!(outcome, CloneOutcome::Made);
        assert_eq!(
            inspect_clone(&Git::without_token(), &scratch.url(), &place),
            CloneState::Ready
        );
        assert_eq!(scratch.scratch_names(), Vec::<String>::new());
    }
}
