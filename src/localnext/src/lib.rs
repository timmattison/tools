//! `localnext` — serves a statically exported Next.js build.
//!
//! A Next.js project configured with `output: 'export'` builds into an `out`
//! directory. This library crate holds the reusable pieces of the `localnext`
//! binary so they can be exercised directly by unit tests. It starts with
//! locating that export root.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

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
    /// Serve this existing regular file (`200`, or `206`/`416` for a `Range`
    /// request — see [`httpfile::serve_file`]).
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

/// Resolves an HTTP request target against the export root.
///
/// `root` MUST already be canonical — [`find_root`] guarantees that, and the
/// confinement check compares canonical paths. `target` is the raw request
/// target as `tiny_http::Request::url()` gives it: a percent-encoded path that
/// may carry a `?query`. Stripping the query and percent-decoding happen inside
/// this function, so no caller can bypass the traversal defense by forgetting a
/// step.
///
/// The target is decoded exactly **once**. That is part of the security
/// property, not an oversight: a doubly-encoded `%252e%252e%252f` decodes to the
/// literal text `%2e%2e%2f`, which is an ordinary path component that matches no
/// file — never to `../`.
///
/// A decoded path under `/static/` is confined under `<root>/static` and is
/// answered on its own terms: a regular file is served, and anything else — a
/// missing asset, or the directory itself — is [`Resolution::NotFound`] rather
/// than the single-page fallback, because an asset that 200s with HTML is worse
/// than one that 404s.
///
/// Every other path is trimmed of leading and trailing `/` — matching the Go
/// tool's `strings.Trim(path, "/")` — and then tried in order: the path itself,
/// then that path as a directory holding an `index.html`, then the path with
/// `.html` appended, then the fallback. A path that is empty once trimmed (`/`,
/// or nothing at all) is the export's own index.
///
/// Two notes on the directory step, which the issue's summary of the Go tool
/// omits:
///
/// - **A directory holding an `index.html` serves that file.** The Go code hands
///   the resolved path to `http.ServeFile`, which does exactly this. A Next.js
///   export configured with `trailingSlash: true` writes `out/about/index.html`,
///   so dropping the step would send every such route to the fallback.
/// - **A directory with no `index.html` does not get a listing.** `http.ServeFile`
///   renders one; this port does not, because a listing exposes the whole export
///   tree and this tool already has a better answer for an unmatched path. Such a
///   directory falls through to the `.html` step and then to the fallback.
#[must_use]
pub fn resolve_request(root: &Path, target: &str) -> Resolution {
    let path = target.split('?').next().unwrap_or(target);
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy();

    if let Some(asset) = decoded.strip_prefix(STATIC_PREFIX) {
        return match httpfile::resolve_under_root(&root.join(STATIC_DIRECTORY), asset) {
            httpfile::PathResolution::Forbidden => Resolution::Forbidden,
            httpfile::PathResolution::Allowed(path) if path.is_file() => Resolution::File(path),
            httpfile::PathResolution::Allowed(_) | httpfile::PathResolution::Missing => {
                Resolution::NotFound
            }
        };
    }

    let fallback = root.join(INDEX_FILE);

    let trimmed = decoded.trim_matches('/');
    if trimmed.is_empty() {
        return Resolution::File(fallback);
    }

    match httpfile::resolve_under_root(root, trimmed) {
        httpfile::PathResolution::Forbidden => return Resolution::Forbidden,
        httpfile::PathResolution::Allowed(path) => {
            if path.is_file() {
                return Resolution::File(path);
            }
            // `is_file` is what keeps a directory off the file-serving path, so
            // the directory's own index is the only way it can be served.
            let index = path.join(INDEX_FILE);
            if index.is_file() {
                return Resolution::File(index);
            }
        }
        httpfile::PathResolution::Missing => {}
    }

    let html = format!("{trimmed}{HTML_SUFFIX}");
    if let httpfile::PathResolution::Allowed(path) = httpfile::resolve_under_root(root, &html) {
        if path.is_file() {
            return Resolution::File(path);
        }
    }

    Resolution::Fallback(fallback)
}

/// Renders the two-line startup banner.
///
/// The first line names the binary and the buildinfo version string; the second
/// names the export root being served and the URL it is served on. `addr` is the
/// address the server actually bound, which is not always the address that was
/// asked for: `--port 0` lets the operating system assign one, and a banner that
/// echoed the request would then name a port nothing is listening on.
///
/// The returned string carries no trailing newline, so a caller prints it with
/// `println!`.
#[must_use]
pub fn banner(version: &str, root: &Path, addr: SocketAddr) -> String {
    format!(
        "localnext {version}\nServing {} on http://{addr}",
        root.display()
    )
}

/// Spawns a fixed pool of `workers` threads serving `root` on `server`.
///
/// Each worker loops on `server.recv()`; the pool shuts down when the server is
/// unblocked (`server.unblock()` once per worker), at which point `recv()` errors
/// and the workers exit. Returns the worker handles so the caller can join them.
/// At least one worker is always spawned even when `workers` is `0`.
#[must_use]
pub fn serve(
    server: Arc<tiny_http::Server>,
    root: Arc<PathBuf>,
    workers: usize,
) -> Vec<JoinHandle<()>> {
    (0..workers.max(1))
        .map(|_| {
            let server = Arc::clone(&server);
            let root = Arc::clone(&root);
            std::thread::spawn(move || {
                // `recv()` errors when the server is unblocked, ending the loop.
                while let Ok(request) = server.recv() {
                    // A request or mid-response IO error (a client that hung up,
                    // say) is swallowed so a single bad request can never panic
                    // a worker and poison the pool.
                    let _ = respond(&root, request);
                }
            })
        })
        .collect()
}

/// Handles one request: resolves its target under `root` and answers it.
///
/// The raw request target goes straight to [`resolve_request`], which strips the
/// query and percent-decodes it internally, so this dispatcher cannot bypass the
/// traversal defense by forgetting a step. The four resolutions answer as:
///
/// - [`Resolution::File`] — hand the file to [`httpfile::serve_file`], which
///   streams it with `200` (or answers a `Range` header with `206`/`416`; see
///   its doc for the full byte-range contract).
/// - [`Resolution::Fallback`] — warn on STDERR (the Go tool logs `Couldn't find
///   file` here), then stream `<root>/index.html` the same way. That path is the
///   one path NOT canonicalized before it is served, because an export need not
///   carry an index; a missing one yields `404` through [`httpfile::serve_file`].
/// - [`Resolution::NotFound`] — `404` with an empty body.
/// - [`Resolution::Forbidden`] — `403` with an empty body.
///
/// The warning goes to STDERR so STDOUT stays free for the startup banner.
fn respond(root: &Path, request: tiny_http::Request) -> std::io::Result<()> {
    // Own the target: `request` is moved into the handlers below, and the
    // fallback warning still needs to name what was asked for.
    let target = request.url().to_string();

    match resolve_request(root, &target) {
        Resolution::File(path) => httpfile::serve_file(&path, request),
        Resolution::Fallback(path) => {
            eprintln!("Couldn't find file for {target}, falling back to {INDEX_FILE}");
            httpfile::serve_file(&path, request)
        }
        Resolution::NotFound => request.respond(tiny_http::Response::empty(404)),
        Resolution::Forbidden => request.respond(tiny_http::Response::empty(403)),
    }
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

        assert_eq!(resolve_request(&root, "/caf%C3%A9"), Resolution::File(html));
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

#[cfg(test)]
mod banner_tests {
    use super::banner;
    use std::net::SocketAddr;
    use std::path::Path;

    /// A fixed buildinfo-shaped version string, so the assertion below reads as
    /// the line a user actually sees.
    const VERSION: &str = "0.1.0 (abc1234, clean)";

    #[test]
    fn the_banner_names_the_version_the_root_and_the_bound_address() {
        let addr = SocketAddr::from(([127, 0, 0, 1], 8080));

        assert_eq!(
            banner(VERSION, Path::new("/projects/site/out"), addr),
            "localnext 0.1.0 (abc1234, clean)\nServing /projects/site/out on http://127.0.0.1:8080"
        );
    }

    #[test]
    fn an_ipv6_address_renders_in_its_bracketed_form() {
        let addr: SocketAddr = "[::1]:4173".parse().expect("parse ipv6 address");

        assert_eq!(
            banner(VERSION, Path::new("/projects/site/out"), addr),
            "localnext 0.1.0 (abc1234, clean)\nServing /projects/site/out on http://[::1]:4173"
        );
    }
}
