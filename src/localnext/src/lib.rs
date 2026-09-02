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
    let candidate = if cwd.file_name().is_some_and(|name| name == ROOT_DIRECTORY) {
        cwd.to_path_buf()
    } else {
        let child = cwd.join(ROOT_DIRECTORY);
        if !child.exists() {
            return Err(RootError::NotFound(cwd.to_path_buf()));
        }
        child
    };

    candidate
        .canonicalize()
        .map_err(|source| RootError::Canonicalize {
            path: candidate.clone(),
            source,
        })
}

/// The URL prefix that routes to [`STATIC_DIRECTORY`] inside the export root.
const STATIC_PREFIX: &str = "/static/";

/// The file a directory (and the single-page fallback) is served from.
const INDEX_FILE: &str = "index.html";

/// The extension appended to an extensionless route before the fallback is used.
const HTML_SUFFIX: &str = ".html";

/// What a request resolves to under the export root.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Serve this existing regular file with `200`.
    File(PathBuf),
    /// Nothing under the root matched. Serve this path — always
    /// `<root>/index.html` — as the single-page fallback, after the caller logs
    /// a warning.
    Fallback(PathBuf),
    /// A request under `/static/` matched no file -> `404`.
    NotFound,
    /// The request tried to leave the root -> `403`.
    Forbidden,
}

/// The result of confining one relative request path under a directory.
enum Confined {
    /// The path exists and its canonical form lives under the directory.
    Allowed(PathBuf),
    /// The path tried to leave the directory.
    Forbidden,
    /// The path does not exist.
    Missing,
}

/// Confines `relative` under `root`, which MUST already be canonical.
fn confine(_root: &Path, _relative: &str) -> Confined {
    Confined::Missing
}

/// Resolves an HTTP request target against the export root.
#[must_use]
pub fn resolve_request(_root: &Path, _target: &str) -> Resolution {
    Resolution::Fallback(PathBuf::new())
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

    use super::{resolve_request, Resolution, INDEX_FILE, STATIC_DIRECTORY, STATIC_PREFIX};
    use std::path::{Path, PathBuf};

    /// Creates a temp dir and returns it alongside its canonicalized path.
    ///
    /// The root MUST be canonicalized: on macOS a `TempDir` lives under `/var`,
    /// which is a symlink to `/private/var`, so an uncanonicalized root would make
    /// the confinement check reject the root's own files.
    fn canonical_root() -> (TempDir, PathBuf) {
        let dir = temp_dir();
        let root = dir.path().canonicalize().expect("canonicalize root");
        (dir, root)
    }

    /// Writes an empty file at `path`, creating every parent directory first.
    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent directories");
        }
        std::fs::write(path, b"").expect("write file");
    }

    #[test]
    fn the_static_url_prefix_names_the_static_directory() {
        assert_eq!(STATIC_PREFIX, format!("/{STATIC_DIRECTORY}/"));
    }

    #[test]
    fn the_root_request_serves_index_html() {
        let (_dir, root) = canonical_root();
        let index = root.join(INDEX_FILE);
        touch(&index);

        assert_eq!(resolve_request(&root, "/"), Resolution::File(index));
    }

    #[test]
    fn an_exact_file_beats_the_html_file_of_the_same_name() {
        let (_dir, root) = canonical_root();
        let exact = root.join("about");
        touch(&exact);
        touch(&root.join("about.html"));

        assert_eq!(resolve_request(&root, "/about"), Resolution::File(exact));
    }

    #[test]
    fn the_html_file_beats_the_fallback() {
        let (_dir, root) = canonical_root();
        let html = root.join("about.html");
        touch(&html);

        assert_eq!(resolve_request(&root, "/about"), Resolution::File(html));
    }

    #[test]
    fn an_unknown_path_falls_back_to_index_html() {
        let (_dir, root) = canonical_root();
        touch(&root.join(INDEX_FILE));

        assert_eq!(
            resolve_request(&root, "/nothing/here"),
            Resolution::Fallback(root.join(INDEX_FILE))
        );
    }

    #[test]
    fn a_directory_holding_an_index_serves_that_index() {
        let (_dir, root) = canonical_root();
        let index = root.join("about").join(INDEX_FILE);
        touch(&index);

        assert_eq!(resolve_request(&root, "/about"), Resolution::File(index));
    }

    #[test]
    fn a_directory_without_an_index_falls_through_to_the_html_file() {
        let (_dir, root) = canonical_root();
        std::fs::create_dir(root.join("about")).expect("create directory");
        let html = root.join("about.html");
        touch(&html);

        assert_eq!(resolve_request(&root, "/about"), Resolution::File(html));
    }

    #[test]
    fn a_static_request_serves_the_static_file() {
        let (_dir, root) = canonical_root();
        let asset = root.join(STATIC_DIRECTORY).join("app.css");
        touch(&asset);

        assert_eq!(
            resolve_request(&root, "/static/app.css"),
            Resolution::File(asset)
        );
    }

    #[test]
    fn a_missing_static_file_is_not_found_rather_than_the_fallback() {
        let (_dir, root) = canonical_root();
        touch(&root.join(STATIC_DIRECTORY).join("app.css"));
        touch(&root.join(INDEX_FILE));

        assert_eq!(
            resolve_request(&root, "/static/missing.css"),
            Resolution::NotFound
        );
    }

    #[test]
    fn the_static_directory_itself_is_not_found() {
        let (_dir, root) = canonical_root();
        touch(&root.join(STATIC_DIRECTORY).join("app.css"));

        assert_eq!(resolve_request(&root, "/static/"), Resolution::NotFound);
    }

    #[test]
    fn a_traversal_attempt_is_forbidden() {
        let (_dir, root) = canonical_root();
        touch(&root.join(INDEX_FILE));

        assert_eq!(
            resolve_request(&root, "/../../etc/passwd"),
            Resolution::Forbidden
        );
    }

    #[test]
    fn an_encoded_traversal_attempt_is_forbidden() {
        let (_dir, root) = canonical_root();
        touch(&root.join(INDEX_FILE));

        assert_eq!(
            resolve_request(&root, "/%2e%2e%2fetc/passwd"),
            Resolution::Forbidden
        );
    }

    #[test]
    fn a_double_encoded_traversal_never_leaves_the_root() {
        let (_dir, root) = canonical_root();
        touch(&root.join(INDEX_FILE));

        let resolution = resolve_request(&root, "/%252e%252e%252fetc/passwd");

        assert_ne!(resolution, Resolution::Forbidden);
        assert_eq!(resolution, Resolution::Fallback(root.join(INDEX_FILE)));
    }

    #[test]
    fn a_query_string_is_stripped() {
        let (_dir, root) = canonical_root();
        let html = root.join("about.html");
        touch(&html);

        assert_eq!(
            resolve_request(&root, "/about?utm=1"),
            Resolution::File(html)
        );
        assert_eq!(
            resolve_request(&root, "/about?utm=1"),
            resolve_request(&root, "/about")
        );
    }

    #[test]
    fn a_multi_byte_name_is_reachable_through_its_encoded_form() {
        let (_dir, root) = canonical_root();
        let html = root.join("café.html");
        touch(&html);

        assert_eq!(
            resolve_request(&root, "/caf%C3%A9"),
            Resolution::File(html)
        );
    }

    #[test]
    fn leading_and_trailing_slashes_are_trimmed() {
        let (_dir, root) = canonical_root();
        let html = root.join("about.html");
        touch(&html);

        assert_eq!(resolve_request(&root, "//about//"), Resolution::File(html));
        assert_eq!(
            resolve_request(&root, "//about//"),
            resolve_request(&root, "/about")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_that_points_outside_the_root_is_forbidden() {
        let (_dir, root) = canonical_root();
        let (_outside_dir, outside) = canonical_root();
        let secret = outside.join("secret.html");
        touch(&secret);

        std::os::unix::fs::symlink(&secret, root.join("escape.html")).expect("create symlink");

        assert_eq!(
            resolve_request(&root, "/escape.html"),
            Resolution::Forbidden
        );
    }
}
