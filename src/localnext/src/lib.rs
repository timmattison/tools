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

/// Confines the relative request path `relative` under `root`.
///
/// `root` MUST already be canonical, because the containment check compares
/// canonical paths.
///
/// The path is rebuilt from only its normal components: a `.` (current dir) and
/// a leading `/` (root) are skipped, while a `..` (parent) or a Windows prefix
/// component returns [`Confined::Forbidden`] outright. The candidate is then
/// canonicalized — which resolves symlinks — and confirmed to still live under
/// `root`, so a symlink that sits inside the root but points outside it is
/// rejected as well. The textual component check alone cannot see that link;
/// the canonical containment check alone would accept a `..` that lands back
/// inside the root. Both are needed.
///
/// Rejecting every `..` outright is stricter than the Go tool this ports, which
/// hands `a/../b` to `filepath.Join` and resolves it back inside the root. The
/// stricter rule is deliberate and matches [`sirn`](https://github.com/timmattison/tools):
/// no legitimate request from a static export carries a `..`, and a rule with no
/// exceptions is a rule with no gaps.
fn confine(root: &Path, relative: &str) -> Confined {
    use std::path::Component;

    let mut sanitized = PathBuf::new();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(name) => sanitized.push(name),
            Component::CurDir | Component::RootDir => {}
            Component::ParentDir | Component::Prefix(_) => return Confined::Forbidden,
        }
    }

    // A path that does not exist has no canonical form, so this is also the
    // "missing" test.
    let Ok(canonical) = root.join(&sanitized).canonicalize() else {
        return Confined::Missing;
    };

    // A symlink that pointed outside the root now canonicalizes outside it.
    if !canonical.starts_with(root) {
        return Confined::Forbidden;
    }

    Confined::Allowed(canonical)
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
        return match confine(&root.join(STATIC_DIRECTORY), asset) {
            Confined::Forbidden => Resolution::Forbidden,
            Confined::Allowed(path) if path.is_file() => Resolution::File(path),
            Confined::Allowed(_) | Confined::Missing => Resolution::NotFound,
        };
    }

    let fallback = root.join(INDEX_FILE);

    let trimmed = decoded.trim_matches('/');
    if trimmed.is_empty() {
        return Resolution::File(fallback);
    }

    match confine(root, trimmed) {
        Confined::Forbidden => return Resolution::Forbidden,
        Confined::Allowed(path) => {
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
        Confined::Missing => {}
    }

    let html = format!("{trimmed}{HTML_SUFFIX}");
    if let Confined::Allowed(path) = confine(root, &html) {
        if path.is_file() {
            return Resolution::File(path);
        }
    }

    Resolution::Fallback(fallback)
}

/// Returns the HTTP `Content-Type` for a file, based on its extension.
///
/// The lookup is case-insensitive (`.CSS`, `.Png`, and `.JSON` resolve the same
/// as their lowercase forms). A file with no extension, a non-UTF-8 extension,
/// or an unrecognized extension falls back to `application/octet-stream`.
/// Textual types carry a `; charset=utf-8` parameter; binary types do not.
///
/// The Go tool this ports got content types for free: it handed every hit to
/// `http.ServeFile`, which sniffs the extension and sets the header itself.
/// `tiny_http` does no such thing — it sends whatever header the response
/// carries and nothing more — so this table is load-bearing. Without it a `.css`
/// file goes out as `text/plain`, the browser refuses to apply it, and the page
/// renders unstyled with no error anywhere.
///
/// The table covers what a Next.js static export emits: markup, stylesheets, the
/// JavaScript chunks and their `.map` source maps, the web manifest, fonts,
/// images (including `.avif`), and the media and archive types a project may
/// place in `public/`.
///
/// This function never panics, and never indexes a string by bytes, so a
/// multi-byte extension is handled like any other.
#[must_use]
pub fn content_type_for(path: &Path) -> &'static str {
    // A missing or non-UTF-8 extension falls through to the binary fallback.
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return OCTET_STREAM;
    };

    // Lowercase a whole `str` rather than slicing bytes, so a multi-byte
    // extension is folded correctly instead of panicking.
    let ext = ext.to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        // `application/json` is conventionally served without a charset
        // parameter, and a `.map` source map is JSON.
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "text/xml; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        _ => OCTET_STREAM,
    }
}

/// The content type of a file whose extension names nothing recognizable.
const OCTET_STREAM: &str = "application/octet-stream";

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
/// - [`Resolution::File`] — stream the file with `200`.
/// - [`Resolution::Fallback`] — warn on STDERR (the Go tool logs `Couldn't find
///   file` here), then stream `<root>/index.html` the same way. That path is the
///   one path NOT canonicalized before it is served, because an export need not
///   carry an index; a missing one yields `404` through [`serve_file`].
/// - [`Resolution::NotFound`] — `404` with an empty body.
/// - [`Resolution::Forbidden`] — `403` with an empty body.
///
/// The warning goes to STDERR so STDOUT stays free for the startup banner.
fn respond(root: &Path, request: tiny_http::Request) -> std::io::Result<()> {
    // Own the target: `request` is moved into the handlers below, and the
    // fallback warning still needs to name what was asked for.
    let target = request.url().to_string();

    match resolve_request(root, &target) {
        Resolution::File(path) => serve_file(&path, request),
        Resolution::Fallback(path) => {
            eprintln!("Couldn't find file for {target}, falling back to {INDEX_FILE}");
            serve_file(&path, request)
        }
        Resolution::NotFound => request.respond(tiny_http::Response::empty(404)),
        Resolution::Forbidden => request.respond(tiny_http::Response::empty(403)),
    }
}

/// Opens `path` only if it is a regular file, returning `None` otherwise.
///
/// A directory, a missing path, or any other non-regular entry yields `None`.
/// This guards the streaming path: a directory opens successfully on Unix, and
/// advertising its metadata length and then failing to produce bytes would hang
/// the client forever waiting for a body that never arrives.
fn open_regular_file(path: &Path) -> Option<std::fs::File> {
    let file = std::fs::File::open(path).ok()?;
    file.metadata().ok()?.is_file().then_some(file)
}

/// Streams `path` to `request`, or responds `404` if it is not a regular file.
///
/// `path` is opened through [`open_regular_file`], so a missing path, a
/// directory, or any other non-regular entry yields `404` — never a hung stream.
/// A regular file (even an empty one) streams as a `200` with a `Content-Type`
/// from [`content_type_for`] and a `Content-Length` that `tiny_http` sets from
/// the file size.
fn serve_file(path: &Path, request: tiny_http::Request) -> std::io::Result<()> {
    let Some(file) = open_regular_file(path) else {
        return request.respond(tiny_http::Response::empty(404));
    };

    // The header name and value are compile-time-known-valid, so the only
    // `expect` on the request path can never fire.
    let content_type =
        tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type_for(path).as_bytes())
            .expect("static Content-Type header is always valid");

    request.respond(tiny_http::Response::from_file(file).with_header(content_type))
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

    use super::content_type_for;

    /// Reads the Content-Type of a filename, for readability at the call sites.
    fn ct(filename: &str) -> &'static str {
        content_type_for(Path::new(filename))
    }

    #[test]
    fn html_is_html() {
        assert_eq!(ct("index.html"), "text/html; charset=utf-8");
        assert_eq!(ct("index.htm"), "text/html; charset=utf-8");
    }

    #[test]
    fn css_is_css() {
        assert_eq!(ct("app.css"), "text/css; charset=utf-8");
    }

    #[test]
    fn javascript_is_javascript() {
        assert_eq!(ct("chunk.js"), "text/javascript; charset=utf-8");
        assert_eq!(ct("chunk.mjs"), "text/javascript; charset=utf-8");
    }

    #[test]
    fn json_and_source_maps_are_json_without_a_charset() {
        assert_eq!(ct("build-manifest.json"), "application/json");
        assert_eq!(ct("chunk.js.map"), "application/json");
    }

    #[test]
    fn a_web_manifest_is_a_manifest() {
        assert_eq!(ct("site.webmanifest"), "application/manifest+json");
    }

    #[test]
    fn text_types_carry_a_charset() {
        assert_eq!(ct("robots.txt"), "text/plain; charset=utf-8");
        assert_eq!(ct("sitemap.xml"), "text/xml; charset=utf-8");
    }

    #[test]
    fn svg_is_svg() {
        assert_eq!(ct("logo.svg"), "image/svg+xml");
    }

    #[test]
    fn image_types() {
        assert_eq!(ct("logo.png"), "image/png");
        assert_eq!(ct("hero.jpg"), "image/jpeg");
        assert_eq!(ct("hero.jpeg"), "image/jpeg");
        assert_eq!(ct("spin.gif"), "image/gif");
        assert_eq!(ct("hero.webp"), "image/webp");
        assert_eq!(ct("hero.avif"), "image/avif");
        assert_eq!(ct("favicon.ico"), "image/x-icon");
    }

    #[test]
    fn font_types() {
        assert_eq!(ct("inter.woff"), "font/woff");
        assert_eq!(ct("inter.woff2"), "font/woff2");
        assert_eq!(ct("inter.ttf"), "font/ttf");
    }

    #[test]
    fn binary_and_media_types() {
        assert_eq!(ct("module.wasm"), "application/wasm");
        assert_eq!(ct("paper.pdf"), "application/pdf");
        assert_eq!(ct("clip.mp4"), "video/mp4");
        assert_eq!(ct("theme.mp3"), "audio/mpeg");
        assert_eq!(ct("beep.wav"), "audio/wav");
        assert_eq!(ct("bundle.zip"), "application/zip");
        assert_eq!(ct("bundle.gz"), "application/gzip");
    }

    #[test]
    fn the_extension_match_is_case_insensitive() {
        assert_eq!(ct("APP.CSS"), "text/css; charset=utf-8");
        assert_eq!(ct("Index.HTML"), "text/html; charset=utf-8");
        assert_eq!(ct("LOGO.PnG"), "image/png");
    }

    #[test]
    fn a_file_with_no_extension_is_an_opaque_byte_stream() {
        assert_eq!(ct("LICENSE"), "application/octet-stream");
        assert_eq!(ct(".gitignore"), "application/octet-stream");
    }

    #[test]
    fn an_unknown_extension_is_an_opaque_byte_stream() {
        assert_eq!(ct("archive.tar"), "application/octet-stream");
    }

    #[test]
    fn a_multi_byte_extension_never_panics() {
        assert_eq!(ct("notes.日本語"), "application/octet-stream");
        assert_eq!(ct("café.html"), "text/html; charset=utf-8");
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
