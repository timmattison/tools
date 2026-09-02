//! `localnext` — serves a statically exported Next.js build.
//!
//! A Next.js project configured with `output: 'export'` builds into an `out`
//! directory. This library crate holds the reusable pieces of the `localnext`
//! binary so they can be exercised directly by unit tests. It starts with
//! locating that export root.

use std::path::{Path, PathBuf};

/// The directory a Next.js static export builds into.
pub const ROOT_DIRECTORY: &str = "out";

/// The directory inside the export root that holds the static assets.
pub const STATIC_DIRECTORY: &str = "static";

/// Error locating the Next.js export root.
#[derive(Debug, thiserror::Error)]
pub enum RootError {
    /// Neither the current directory nor a child of it is the expected `out` directory.
    #[error("couldn't find the expected root directory 'out' under {0}")]
    NotFound(PathBuf),
    /// The root was found but could not be canonicalized.
    #[error("couldn't resolve the root directory {path}: {source}")]
    Canonicalize {
        /// The path that could not be canonicalized.
        path: PathBuf,
        /// The underlying filesystem error.
        source: std::io::Error,
    },
}

/// Locates the Next.js static-export root, starting from `cwd`.
///
/// The rules, in order:
///
/// 1. When the final component of `cwd` is named `out`, `cwd` itself is the root
///    — the caller already stood inside the export.
/// 2. Otherwise, when `cwd/out` exists, that is the root.
/// 3. Otherwise there is no export to serve.
///
/// Two deliberate departures from the Go tool this ports:
///
/// - **The returned root is canonicalized.** The Go version returns the path as
///   it was joined. A later stage confines every request under this root, and
///   that check compares canonical paths: on macOS a temporary directory lives
///   under `/var`, which is a symlink to `/private/var`, so an uncanonicalized
///   root would make the confinement check reject the root's own files.
/// - **Rule 2 is a plain existence check**, matching Go's `os.Stat`. A path named
///   `out` that is not a directory is still taken as the root; refusing it would
///   be behavior this port does not carry over.
///
/// # Errors
///
/// Returns [`RootError::NotFound`] when neither rule 1 nor rule 2 matches, and
/// [`RootError::Canonicalize`] when the chosen root cannot be resolved to a
/// canonical path (it was removed between the check and the resolution, or a
/// component of it is unreadable).
pub fn find_root(cwd: &Path) -> Result<PathBuf, RootError> {
    Err(RootError::NotFound(cwd.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::{find_root, RootError, ROOT_DIRECTORY};
    use tempfile::TempDir;

    /// Creates a temp dir. Every test path is derived from it, so concurrent runs
    /// of this same test binary never share a filesystem location.
    fn temp_dir() -> TempDir {
        TempDir::new().expect("temp dir")
    }

    #[test]
    fn a_cwd_named_out_is_itself_the_root() {
        let dir = temp_dir();
        let out = dir.path().join(ROOT_DIRECTORY);
        std::fs::create_dir(&out).expect("create out dir");
        let expected = out.canonicalize().expect("canonicalize expected root");

        assert_eq!(find_root(&out).expect("find root"), expected);
    }

    #[test]
    fn a_cwd_holding_an_out_child_resolves_to_that_child() {
        let dir = temp_dir();
        let out = dir.path().join(ROOT_DIRECTORY);
        std::fs::create_dir(&out).expect("create out dir");
        let expected = out.canonicalize().expect("canonicalize expected root");

        assert_eq!(find_root(dir.path()).expect("find root"), expected);
    }

    #[test]
    fn a_cwd_with_no_out_under_it_is_not_found() {
        let dir = temp_dir();

        let err = find_root(dir.path()).expect_err("no out directory");
        match err {
            RootError::NotFound(path) => assert_eq!(path, dir.path()),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn a_cwd_named_out_wins_over_an_out_child() {
        let dir = temp_dir();
        let outer = dir.path().join(ROOT_DIRECTORY);
        let inner = outer.join(ROOT_DIRECTORY);
        std::fs::create_dir_all(&inner).expect("create nested out dirs");
        let expected = outer.canonicalize().expect("canonicalize expected root");

        assert_eq!(find_root(&outer).expect("find root"), expected);
    }
}
