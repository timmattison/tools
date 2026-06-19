//! `sirn` ("Serve It Right Now") — a tiny, zero-config HTTP file server.
//!
//! This library crate holds the reusable pieces of the `sirn` binary so they can
//! be exercised directly by unit tests. The first such piece is the
//! [`content_type_for`] extension → MIME lookup used to label served files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// Returns the HTTP `Content-Type` for a file, based on its extension.
///
/// The lookup is case-insensitive (`.HTML`, `.Png`, and `.JSON` resolve the same
/// as their lowercase forms). Files with no extension, a non-UTF-8 extension, or
/// an unrecognized extension fall back to `application/octet-stream`. Textual
/// types carry a `; charset=utf-8` parameter; binary types do not.
///
/// This function never panics.
#[must_use]
pub fn content_type_for(path: &Path) -> &'static str {
    const OCTET_STREAM: &str = "application/octet-stream";

    // A missing or non-UTF-8 extension falls through to the binary fallback.
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return OCTET_STREAM;
    };

    // Normalize the extension to lowercase for case-insensitive matching.
    let ext = ext.to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        // application/json is conventionally served without a charset parameter.
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xml" => "text/xml; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        _ => OCTET_STREAM,
    }
}

/// HTML-escapes `s` for safe insertion into element text or a double-quoted
/// attribute value. Escapes `&`, `<`, `>`, `"`, and `'`.
#[must_use]
pub fn html_escape(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Renders a directory-listing HTML page for `url_path` (the request path, e.g.
/// `/` or `/sub/`) given `entries` as `(name, is_dir)` pairs (already sorted by
/// the caller). Each entry name is HTML-escaped; directories are marked with a
/// trailing `/` in both the displayed text and the link target. Entry hrefs are
/// absolute (built from `url_path`) so they resolve correctly regardless of
/// whether `url_path` ends in a slash. A `..` parent link is included unless
/// `url_path` is the root (`/` or empty).
#[must_use]
pub fn render_directory_listing(url_path: &str, entries: &[(String, bool)]) -> String {
    use std::fmt::Write as _;

    // A normalized base always ends in `/`, so hrefs are `{base}{name}` and
    // resolve the same whether or not the request path ended in a slash.
    let base = if url_path.ends_with('/') {
        url_path.to_string()
    } else {
        format!("{url_path}/")
    };
    let escaped_path = html_escape(url_path);

    let mut page = String::new();
    let _ = writeln!(page, "<!DOCTYPE html>");
    let _ = writeln!(page, "<html>");
    let _ = writeln!(page, "<head>");
    let _ = writeln!(page, "<meta charset=\"utf-8\">");
    let _ = writeln!(page, "<title>{escaped_path}</title>");
    let _ = writeln!(page, "</head>");
    let _ = writeln!(page, "<body>");
    let _ = writeln!(page, "<h1>{escaped_path}</h1>");
    let _ = writeln!(page, "<ul>");

    // A `..` parent link precedes the entries unless this is the root listing.
    if url_path != "/" && !url_path.is_empty() {
        let parent_href = html_escape(&parent_of(&base));
        let _ = writeln!(page, "<li><a href=\"{parent_href}\">../</a></li>");
    }

    for (name, is_dir) in entries {
        let suffix = if *is_dir { "/" } else { "" };
        let text = format!("{}{suffix}", html_escape(name));
        let href = format!("{}{suffix}", html_escape(&format!("{base}{name}")));
        let _ = writeln!(page, "<li><a href=\"{href}\">{text}</a></li>");
    }

    let _ = writeln!(page, "</ul>");
    let _ = writeln!(page, "</body>");
    let _ = writeln!(page, "</html>");
    page
}

/// Returns the parent href for a normalized base (one that ends in `/`).
///
/// The trailing `/` is dropped, then everything up to and including the last
/// remaining `/` is kept (e.g. `/a/b/` -> `/a/`; `/sub/` -> `/`). UTF-8 safe:
/// it splits on the ASCII `/` boundary via [`str::rsplit_once`], never indexing
/// raw bytes.
fn parent_of(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some((prefix, _)) => format!("{prefix}/"),
        None => "/".to_string(),
    }
}

/// The result of resolving a request URL path against a directory-mode root.
#[derive(Debug, PartialEq, Eq)]
pub enum PathResolution {
    /// The request resolved to this existing, in-root canonical path.
    Allowed(PathBuf),
    /// The request tried to escape the root (`..` traversal or a symlink that
    /// points outside it) -> `403`.
    Forbidden,
    /// The request resolved to a path inside the root that does not exist -> `404`.
    Missing,
}

/// Resolves an HTTP request URL path against `root`, confining the result to it.
///
/// `root` MUST already be canonicalized by the caller. `url_path` is the request
/// path with any `?query` already stripped (e.g. `/`, `/sub/file.txt`). The path
/// is rebuilt from only its normal components — a leading `/` (root) and `.`
/// (current dir) components are skipped, while any `..` (parent) or Windows
/// prefix component is treated as an escape attempt and rejected. The candidate
/// is then canonicalized (resolving symlinks) and confirmed to live under `root`;
/// a symlink pointing outside the root is therefore rejected even though it sits
/// inside it textually.
///
/// Returns [`PathResolution::Forbidden`] for an escape attempt, `Missing` when
/// the in-root path does not exist, and `Allowed(canonical)` otherwise.
#[must_use]
pub fn resolve_under_root(root: &Path, url_path: &str) -> PathResolution {
    use std::path::Component;

    // Rebuild the request path from only its normal components. A `..` or a
    // Windows prefix is a textual escape attempt and is rejected outright; the
    // leading `/` (root) and any `.` (current dir) are simply skipped.
    let mut sanitized = PathBuf::new();
    for component in Path::new(url_path).components() {
        match component {
            Component::Normal(c) => sanitized.push(c),
            Component::CurDir | Component::RootDir => {}
            Component::ParentDir | Component::Prefix(_) => return PathResolution::Forbidden,
        }
    }

    let candidate = root.join(&sanitized);

    // A non-existent path canonicalizes to an error -> Missing.
    let canonical = match candidate.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => return PathResolution::Missing,
    };

    // A symlink that pointed outside the root now canonicalizes outside it.
    if !canonical.starts_with(root) {
        return PathResolution::Forbidden;
    }

    PathResolution::Allowed(canonical)
}

/// Error building the route map for files mode.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RouteError {
    /// Two distinct input files share a basename, so they would be served at the
    /// same `/<basename>` URL. The payload is the bare colliding basename.
    #[error("duplicate basename '{0}': two files would be served at the same URL")]
    DuplicateBasename(String),
}

/// Builds the `/<basename>` -> file route map for files mode.
///
/// Each input file is served at `/<basename>` where basename is the final path
/// component. Two inputs sharing a basename are a hard error (routes must be
/// unambiguous and self-documenting). The returned [`BTreeMap`] is sorted by URL
/// key, giving deterministic ordering for the startup banner and tests. Paths are
/// kept exactly as given (not canonicalized); existence is checked per-request.
///
/// # Errors
/// Returns [`RouteError::DuplicateBasename`] when two inputs share a basename.
pub fn build_routes(files: &[PathBuf]) -> Result<BTreeMap<String, PathBuf>, RouteError> {
    let mut routes: BTreeMap<String, PathBuf> = BTreeMap::new();

    for file in files {
        // The basename is the final path component; fall back to the whole path's
        // lossy string for odd inputs (e.g. a trailing-slash or root path). Using
        // `to_string_lossy` keeps multi-byte UTF-8 names intact with no slicing.
        let basename = file
            .file_name()
            .map_or_else(|| file.to_string_lossy(), |name| name.to_string_lossy())
            .into_owned();
        let url = format!("/{basename}");

        if routes.contains_key(&url) {
            return Err(RouteError::DuplicateBasename(basename));
        }
        routes.insert(url, file.clone());
    }

    Ok(routes)
}

/// The serving mode chosen from the positional arguments.
#[derive(Debug, PartialEq, Eq)]
pub enum ModeDecision {
    /// Serve a single directory as a browsable tree. `None` means "no directory
    /// argument was given — serve the current directory"; `Some(path)` means
    /// "serve this directory argument".
    Directory(Option<PathBuf>),
    /// Serve the given positional files in files mode (`/<basename>` routes).
    Files,
}

/// Error classifying the positional arguments into a [`ModeDecision`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ModeError {
    /// A directory argument was mixed with other arguments. A directory can only
    /// be served on its own; mixing it with files is ambiguous (and would
    /// otherwise hang, since a directory cannot be streamed as a file).
    #[error("cannot serve directory '{0}' alongside other arguments: pass a single directory to serve it as a tree, or list only files")]
    DirectoryMixedWithFiles(String),
}

/// Classifies positional `files` into a [`ModeDecision`], using `is_dir` to test
/// whether each argument is a directory on disk.
///
/// Rules:
/// - no arguments -> `Directory(None)` (caller serves the current directory),
/// - exactly one argument that is a directory -> `Directory(Some(arg))`,
/// - one or more arguments, none a directory -> `Files`,
/// - any directory mixed with other arguments -> `Err(DirectoryMixedWithFiles)`.
///
/// `is_dir` is injected (rather than calling the filesystem directly) so the
/// decision logic is unit-testable without touching disk.
///
/// # Errors
/// Returns [`ModeError::DirectoryMixedWithFiles`] when a directory argument is
/// given together with any other argument.
pub fn decide_mode(
    files: &[PathBuf],
    is_dir: impl Fn(&Path) -> bool,
) -> Result<ModeDecision, ModeError> {
    match files {
        // No positional arguments: serve the current directory as a tree.
        [] => Ok(ModeDecision::Directory(None)),
        // Exactly one argument that is a directory: serve it as a tree.
        [only] if is_dir(only) => Ok(ModeDecision::Directory(Some(only.clone()))),
        // Two or more arguments, or a single non-directory argument. A directory
        // anywhere in the list is ambiguous (and would hang), so it is a hard
        // error; otherwise every argument is a file, so this is files mode.
        _ => match files.iter().find(|file| is_dir(file)) {
            Some(dir) => Err(ModeError::DirectoryMixedWithFiles(
                dir.to_string_lossy().into_owned(),
            )),
            None => Ok(ModeDecision::Files),
        },
    }
}

/// Builds the multi-line startup banner for files mode.
///
/// `version` is the buildinfo version string, `bind` the bind address, `port`
/// the resolved port, `source` the optional port-derivation description (included
/// only under `--verbose`), and `routes` the sorted route map. Every served
/// file's full `http://<bind>:<port>/<basename>` URL appears on its own line.
#[must_use]
pub fn files_banner(
    version: &str,
    bind: &str,
    port: u16,
    source: Option<&str>,
    routes: &BTreeMap<String, PathBuf>,
) -> String {
    use std::fmt::Write as _;

    let mut banner = format!("sirn {version}\n");
    if let Some(source) = source {
        let _ = writeln!(banner, "{source}");
    }
    let _ = writeln!(banner, "Serving on http://{bind}:{port}");
    for url_path in routes.keys() {
        let _ = writeln!(banner, "  http://{bind}:{port}{url_path}");
    }
    banner
}

/// Serves `routes` on `server` using a fixed pool of `workers` threads.
///
/// Each worker loops on `server.recv()`; the pool shuts down when the server is
/// unblocked (`server.unblock()` once per worker), at which point `recv()` errors
/// and the workers exit. Returns the worker handles so the caller can join them.
/// At least one worker is always spawned even if `workers` is `0`.
#[must_use]
pub fn serve(
    server: Arc<tiny_http::Server>,
    routes: Arc<BTreeMap<String, PathBuf>>,
    workers: usize,
) -> Vec<JoinHandle<()>> {
    (0..workers.max(1))
        .map(|_| {
            let server = Arc::clone(&server);
            let routes = Arc::clone(&routes);
            std::thread::spawn(move || {
                // `recv()` errors when the server is unblocked, ending the loop.
                while let Ok(request) = server.recv() {
                    // A request or mid-response IO error (e.g. a client
                    // disconnecting) is swallowed so a single bad request can
                    // never panic a worker and poison the pool.
                    let _ = respond(&routes, request);
                }
            })
        })
        .collect()
}

/// How often the availability monitor restats the served files.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Spawns the files-mode availability monitor on its own thread.
///
/// Polls every served file once per [`POLL_INTERVAL`] and prints a line for each
/// availability transition (`Ready to serve <name>` / `Warning!  File <name> not
/// found...`), mirroring the original Java tool. The thread exits promptly when
/// `shutdown` is signalled — either an explicit `()` is sent or the sender is
/// dropped — so it never delays process teardown by up to a full poll interval.
/// Returns the join handle so the caller can wait for a clean shutdown.
#[must_use]
pub fn spawn_monitor(
    routes: Arc<BTreeMap<String, PathBuf>>,
    shutdown: Receiver<()>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut monitor = AvailabilityMonitor::new(&routes);
        loop {
            for transition in monitor.poll() {
                println!("{}", transition.message());
            }
            match shutdown.recv_timeout(POLL_INTERVAL) {
                // Explicit shutdown, or the sender was dropped: stop polling.
                Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                // Timed out without a signal: poll again.
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    })
}

/// Opens `path` only if it is a regular file, returning `None` otherwise.
///
/// A directory, a missing path, or any other non-regular entry yields `None`.
/// This guards the streaming path: advertising a directory's metadata length
/// and then failing to read its bytes would hang the client waiting for a body
/// that never arrives.
fn open_regular_file(path: &Path) -> Option<std::fs::File> {
    let file = std::fs::File::open(path).ok()?;
    // A directory opens successfully on Unix, so confirm the entry is a regular
    // file before letting it onto the streaming path.
    file.metadata().ok()?.is_file().then_some(file)
}

/// Streams `path` to `request`, or responds `404` if it is not a regular file.
///
/// `path` is opened through [`open_regular_file`], so a missing path, a
/// directory, or any other non-regular entry yields `404` (never a hung stream).
/// A regular file (even empty) streams as a `200` with a `Content-Length` (set
/// by `tiny_http` from the file size) and a `Content-Type` from its extension.
fn serve_file(path: &Path, request: tiny_http::Request) -> std::io::Result<()> {
    let Some(file) = open_regular_file(path) else {
        return request.respond(tiny_http::Response::empty(404));
    };

    // The header name and value are compile-time-known-valid, so the only
    // `expect` on the request path can never fire.
    let content_type =
        tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type_for(path).as_bytes())
            .expect("static Content-Type header is always valid");

    let response = tiny_http::Response::from_file(file).with_header(content_type);
    request.respond(response)
}

/// Handles one request: looks up its path in `routes` and streams the file.
///
/// The lookup path is the request URL with any `?query` stripped (no
/// percent-decoding — exact match). An unregistered path, or a registered path
/// whose target is missing or is a directory, yields `404`; a registered
/// regular file (even empty) streams as a `200` (see [`serve_file`]).
fn respond(routes: &BTreeMap<String, PathBuf>, request: tiny_http::Request) -> std::io::Result<()> {
    let path = request.url().split('?').next().unwrap_or("");

    let Some(file_path) = routes.get(path) else {
        return request.respond(tiny_http::Response::empty(404));
    };

    serve_file(file_path, request)
}

/// A change in a served file's on-disk availability between two monitor polls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    /// A previously-missing file now exists ("Ready to serve …").
    Appeared(String),
    /// A previously-present file is now missing ("Warning! … not found…").
    Disappeared(String),
}

impl Transition {
    /// The exact log line for this transition, mirroring the Java tool's wording.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Transition::Appeared(name) => format!("Ready to serve {name}"),
            Transition::Disappeared(name) => format!("Warning!  File {name} not found..."),
        }
    }
}

/// Tracks the on-disk presence of each served file and reports availability
/// transitions between successive polls.
///
/// Each file starts assumed-present (optimistic): a file that is present at
/// startup produces no message (the startup banner already announced it), while
/// a file that is already missing at the first poll is reported exactly once.
pub struct AvailabilityMonitor {
    /// Per file: its display name, the path to stat, and last-observed presence.
    files: Vec<MonitoredFile>,
}

/// One served file's monitor state: its display name, on-disk path, and the
/// presence observed at the most recent poll.
struct MonitoredFile {
    /// The basename shown in log lines (the route key without the leading `/`).
    name: String,
    /// The path stat-ed each poll to determine presence.
    path: PathBuf,
    /// Last-observed presence; initialized `true` (assumed present at startup).
    present: bool,
}

impl AvailabilityMonitor {
    /// Builds a monitor from a files-mode route map (`/<basename>` -> path).
    ///
    /// The display name is the route key with its single leading `/` removed.
    /// Every file starts assumed-present so that present-at-startup files emit
    /// nothing on the first poll (the startup banner already announced them).
    #[must_use]
    pub fn new(routes: &BTreeMap<String, PathBuf>) -> Self {
        let files = routes
            .iter()
            .map(|(url, path)| MonitoredFile {
                name: url.strip_prefix('/').unwrap_or(url).to_string(),
                path: path.clone(),
                present: true,
            })
            .collect();
        Self { files }
    }

    /// Stats every file and returns the transitions since the last poll, in the
    /// monitor's file order.
    ///
    /// Each file's stored presence is updated so a steady state (no change since
    /// the previous poll) produces nothing on the next poll. Any stat error is
    /// treated as the file being absent.
    pub fn poll(&mut self) -> Vec<Transition> {
        let mut transitions = Vec::new();
        for entry in &mut self.files {
            // Any stat error (permissions, broken symlink, etc.) counts as absent.
            let now = entry.path.try_exists().unwrap_or(false);
            if now != entry.present {
                transitions.push(if now {
                    Transition::Appeared(entry.name.clone())
                } else {
                    Transition::Disappeared(entry.name.clone())
                });
                entry.present = now;
            }
        }
        transitions
    }
}

#[cfg(test)]
mod tests {
    use super::content_type_for;
    use std::path::Path;

    /// Asserts the Content-Type for a given filename, for readability.
    fn ct(filename: &str) -> &'static str {
        content_type_for(Path::new(filename))
    }

    #[test]
    fn html_and_htm_are_html() {
        assert_eq!(ct("index.html"), "text/html; charset=utf-8");
        assert_eq!(ct("index.htm"), "text/html; charset=utf-8");
    }

    #[test]
    fn css_is_css() {
        assert_eq!(ct("style.css"), "text/css; charset=utf-8");
    }

    #[test]
    fn js_and_mjs_are_javascript() {
        assert_eq!(ct("app.js"), "text/javascript; charset=utf-8");
        assert_eq!(ct("module.mjs"), "text/javascript; charset=utf-8");
    }

    #[test]
    fn json_has_no_charset() {
        assert_eq!(ct("data.json"), "application/json");
    }

    #[test]
    fn txt_is_plain() {
        assert_eq!(ct("notes.txt"), "text/plain; charset=utf-8");
    }

    #[test]
    fn md_is_markdown() {
        assert_eq!(ct("README.md"), "text/markdown; charset=utf-8");
    }

    #[test]
    fn csv_is_csv() {
        assert_eq!(ct("rows.csv"), "text/csv; charset=utf-8");
    }

    #[test]
    fn xml_is_xml() {
        assert_eq!(ct("feed.xml"), "text/xml; charset=utf-8");
    }

    #[test]
    fn image_types() {
        assert_eq!(ct("logo.png"), "image/png");
        assert_eq!(ct("photo.jpg"), "image/jpeg");
        assert_eq!(ct("photo.jpeg"), "image/jpeg");
        assert_eq!(ct("anim.gif"), "image/gif");
        assert_eq!(ct("icon.svg"), "image/svg+xml");
        assert_eq!(ct("pic.webp"), "image/webp");
        assert_eq!(ct("favicon.ico"), "image/x-icon");
    }

    #[test]
    fn document_and_binary_types() {
        assert_eq!(ct("doc.pdf"), "application/pdf");
        assert_eq!(ct("mod.wasm"), "application/wasm");
        assert_eq!(ct("archive.zip"), "application/zip");
        assert_eq!(ct("blob.gz"), "application/gzip");
    }

    #[test]
    fn font_types() {
        assert_eq!(ct("font.woff"), "font/woff");
        assert_eq!(ct("font.woff2"), "font/woff2");
    }

    #[test]
    fn media_types() {
        assert_eq!(ct("clip.mp4"), "video/mp4");
        assert_eq!(ct("song.mp3"), "audio/mpeg");
        assert_eq!(ct("sound.wav"), "audio/wav");
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert_eq!(ct("INDEX.HTML"), "text/html; charset=utf-8");
        assert_eq!(ct("Logo.Png"), "image/png");
        assert_eq!(ct("DATA.JSON"), "application/json");
        assert_eq!(ct("Photo.JPEG"), "image/jpeg");
    }

    #[test]
    fn text_types_carry_charset() {
        for name in ["a.html", "a.css", "a.js", "a.txt", "a.md", "a.csv", "a.xml"] {
            assert!(
                ct(name).contains("; charset=utf-8"),
                "{name} should carry a utf-8 charset, got {}",
                ct(name)
            );
        }
    }

    #[test]
    fn binary_types_do_not_carry_charset() {
        for name in ["a.png", "a.jpg", "a.pdf", "a.zip", "a.mp4", "a.wasm"] {
            assert!(
                !ct(name).contains("charset"),
                "{name} should not carry a charset, got {}",
                ct(name)
            );
        }
    }

    #[test]
    fn unknown_extension_is_octet_stream() {
        assert_eq!(ct("mystery.xyz"), "application/octet-stream");
    }

    #[test]
    fn no_extension_is_octet_stream() {
        assert_eq!(ct("README"), "application/octet-stream");
        assert_eq!(ct("Makefile"), "application/octet-stream");
    }

    #[test]
    fn dotfile_without_extension_is_octet_stream() {
        // A leading-dot file like ".gitignore" has no Path extension.
        assert_eq!(ct(".gitignore"), "application/octet-stream");
    }
}

#[cfg(test)]
mod route_tests {
    use super::{build_routes, RouteError};
    use std::path::PathBuf;

    #[test]
    fn distinct_basenames_map_to_their_files() {
        let a = PathBuf::from("a/x.txt");
        let b = PathBuf::from("b/y.txt");
        let routes = build_routes(&[a.clone(), b.clone()]).expect("distinct basenames are ok");

        assert_eq!(routes.len(), 2);
        assert_eq!(routes.get("/x.txt"), Some(&a));
        assert_eq!(routes.get("/y.txt"), Some(&b));
    }

    #[test]
    fn single_file_maps_to_one_route() {
        let only = PathBuf::from("some/dir/report.pdf");
        let routes = build_routes(std::slice::from_ref(&only)).expect("a single file is ok");

        assert_eq!(routes.len(), 1);
        assert_eq!(routes.get("/report.pdf"), Some(&only));
    }

    #[test]
    fn empty_input_yields_empty_map() {
        let routes = build_routes(&[]).expect("no files is ok");
        assert!(routes.is_empty());
    }

    #[test]
    fn duplicate_basename_across_dirs_is_an_error() {
        let one = PathBuf::from("a/dup.txt");
        let two = PathBuf::from("b/dup.txt");
        let err = build_routes(&[one, two]).expect_err("same basename collides");

        assert_eq!(err, RouteError::DuplicateBasename("dup.txt".to_string()));
    }

    #[test]
    fn utf8_basenames_collide_correctly() {
        // Japanese filename (multi-byte UTF-8) shared across two dirs collides on
        // its bare basename, with no byte-index slicing panic.
        let one = PathBuf::from("dir1/日本語.txt");
        let two = PathBuf::from("dir2/日本語.txt");
        let err = build_routes(&[one, two]).expect_err("same utf-8 basename collides");

        assert_eq!(err, RouteError::DuplicateBasename("日本語.txt".to_string()));
    }

    #[test]
    fn distinct_utf8_basenames_map_without_panicking() {
        let accented = PathBuf::from("docs/café.md");
        let emoji = PathBuf::from("assets/🎉.bin");
        let routes =
            build_routes(&[accented.clone(), emoji.clone()]).expect("distinct utf-8 names are ok");

        assert_eq!(routes.len(), 2);
        assert_eq!(routes.get("/café.md"), Some(&accented));
        assert_eq!(routes.get("/🎉.bin"), Some(&emoji));
    }
}

#[cfg(test)]
mod banner_tests {
    use super::files_banner;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// A 2-entry route map used across the banner assertions.
    fn sample_routes() -> BTreeMap<String, PathBuf> {
        let mut routes = BTreeMap::new();
        routes.insert("/a.txt".to_string(), PathBuf::from("dir/a.txt"));
        routes.insert("/b.css".to_string(), PathBuf::from("other/b.css"));
        routes
    }

    #[test]
    fn banner_includes_version_and_bind_port() {
        let routes = sample_routes();
        let banner = files_banner("0.1.0 (abc1234, clean)", "127.0.0.1", 8080, None, &routes);

        assert!(
            banner.contains("0.1.0 (abc1234, clean)"),
            "banner should include the version string, got:\n{banner}"
        );
        assert!(
            banner.contains("127.0.0.1:8080"),
            "banner should include bind:port, got:\n{banner}"
        );
    }

    #[test]
    fn banner_includes_every_route_url() {
        let routes = sample_routes();
        let banner = files_banner("0.1.0", "127.0.0.1", 8080, None, &routes);

        assert!(
            banner.contains("http://127.0.0.1:8080/a.txt"),
            "banner should include the full URL for a.txt, got:\n{banner}"
        );
        assert!(
            banner.contains("http://127.0.0.1:8080/b.css"),
            "banner should include the full URL for b.css, got:\n{banner}"
        );
    }

    #[test]
    fn banner_includes_source_when_some() {
        let routes = sample_routes();
        let source = "Port 8080 for repo 'sirn' on branch 'main'";
        let banner = files_banner("0.1.0", "127.0.0.1", 8080, Some(source), &routes);

        assert!(
            banner.contains(source),
            "banner should include the derivation source under --verbose, got:\n{banner}"
        );
    }

    #[test]
    fn banner_omits_source_when_none() {
        let routes = sample_routes();
        let source = "Port 8080 for repo 'sirn' on branch 'main'";
        let banner = files_banner("0.1.0", "127.0.0.1", 8080, None, &routes);

        assert!(
            !banner.contains(source),
            "banner should not include any derivation source when None, got:\n{banner}"
        );
    }
}

#[cfg(test)]
mod transition_tests {
    use super::Transition;

    #[test]
    fn appeared_reads_ready_to_serve() {
        let transition = Transition::Appeared("foo.txt".to_string());
        assert_eq!(transition.message(), "Ready to serve foo.txt");
    }

    #[test]
    fn disappeared_reads_warning_not_found() {
        let transition = Transition::Disappeared("foo.txt".to_string());
        assert_eq!(transition.message(), "Warning!  File foo.txt not found...");
    }

    #[test]
    fn utf8_name_survives_both_transitions() {
        // Multi-byte UTF-8 names must format without any byte-index slicing panic.
        let appeared = Transition::Appeared("café.txt".to_string());
        assert_eq!(appeared.message(), "Ready to serve café.txt");

        let disappeared = Transition::Disappeared("日本語.txt".to_string());
        assert_eq!(
            disappeared.message(),
            "Warning!  File 日本語.txt not found..."
        );
    }
}

#[cfg(test)]
mod monitor_tests {
    use super::{AvailabilityMonitor, Transition};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    /// Builds a single-route map (`/<basename>` -> `path`) for these tests.
    fn route_for(basename: &str, path: &Path) -> BTreeMap<String, PathBuf> {
        let mut routes = BTreeMap::new();
        routes.insert(format!("/{basename}"), path.to_path_buf());
        routes
    }

    #[test]
    fn present_then_absent_yields_one_disappeared() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("x.txt");
        std::fs::write(&path, b"hi").expect("write file");
        let routes = route_for("x.txt", &path);

        let mut monitor = AvailabilityMonitor::new(&routes);
        // Present at startup -> nothing on the first poll.
        assert_eq!(monitor.poll(), Vec::<Transition>::new());

        std::fs::remove_file(&path).expect("remove file");
        assert_eq!(
            monitor.poll(),
            vec![Transition::Disappeared("x.txt".to_string())]
        );
    }

    #[test]
    fn absent_then_present_yields_one_appeared() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("x.txt");
        // Do not create the file yet.
        let routes = route_for("x.txt", &path);

        let mut monitor = AvailabilityMonitor::new(&routes);
        // Absent at the first poll -> reported once.
        assert_eq!(
            monitor.poll(),
            vec![Transition::Disappeared("x.txt".to_string())]
        );

        std::fs::write(&path, b"hi").expect("write file");
        assert_eq!(
            monitor.poll(),
            vec![Transition::Appeared("x.txt".to_string())]
        );
    }

    #[test]
    fn steady_state_yields_no_messages() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("x.txt");
        std::fs::write(&path, b"hi").expect("write file");
        let routes = route_for("x.txt", &path);

        let mut monitor = AvailabilityMonitor::new(&routes);
        assert_eq!(monitor.poll(), Vec::<Transition>::new());
        assert_eq!(monitor.poll(), Vec::<Transition>::new());
    }

    #[test]
    fn initial_absence_reported_once() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("x.txt");
        // Never created.
        let routes = route_for("x.txt", &path);

        let mut monitor = AvailabilityMonitor::new(&routes);
        assert_eq!(
            monitor.poll(),
            vec![Transition::Disappeared("x.txt".to_string())]
        );
        // Still absent -> not reported again.
        assert_eq!(monitor.poll(), Vec::<Transition>::new());
    }

    #[test]
    fn utf8_name_does_not_panic() {
        let dir = TempDir::new().expect("temp dir");
        let name = "日本語.txt";
        let path = dir.path().join(name);
        std::fs::write(&path, b"hi").expect("write file");
        let routes = route_for(name, &path);

        let mut monitor = AvailabilityMonitor::new(&routes);
        // Present at startup -> nothing first.
        assert_eq!(monitor.poll(), Vec::<Transition>::new());

        std::fs::remove_file(&path).expect("remove file");
        assert_eq!(
            monitor.poll(),
            vec![Transition::Disappeared(name.to_string())]
        );

        std::fs::write(&path, b"hi").expect("recreate file");
        assert_eq!(monitor.poll(), vec![Transition::Appeared(name.to_string())]);
    }
}

#[cfg(test)]
mod serve_file_tests {
    use super::open_regular_file;
    use tempfile::TempDir;

    #[test]
    fn directory_is_not_opened_as_a_regular_file() {
        // A directory is openable via `File::open` on Unix, but it is not a
        // regular file: streaming it would advertise the directory's metadata
        // length and then hang the client. It must never be opened here.
        let dir = TempDir::new().expect("temp dir");
        assert!(open_regular_file(dir.path()).is_none());
    }

    #[test]
    fn regular_file_is_opened() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("x.txt");
        std::fs::write(&path, b"hi").expect("write file");
        assert!(open_regular_file(&path).is_some());
    }

    #[test]
    fn missing_path_is_not_opened() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("does-not-exist.txt");
        assert!(open_regular_file(&path).is_none());
    }
}

#[cfg(test)]
mod mode_tests {
    use super::{decide_mode, ModeDecision, ModeError};
    use std::path::PathBuf;

    #[test]
    fn no_args_is_directory_mode_for_current_dir() {
        // No directory argument means "serve the current directory".
        let decision = decide_mode(&[], |_| false);
        assert_eq!(decision, Ok(ModeDecision::Directory(None)));
    }

    #[test]
    fn single_directory_arg_is_directory_mode_for_that_dir() {
        let dir = PathBuf::from("somedir");
        let decision = decide_mode(std::slice::from_ref(&dir), |_| true);
        assert_eq!(decision, Ok(ModeDecision::Directory(Some(dir))));
    }

    #[test]
    fn single_file_arg_is_files_mode() {
        let decision = decide_mode(&[PathBuf::from("a.txt")], |_| false);
        assert_eq!(decision, Ok(ModeDecision::Files));
    }

    #[test]
    fn multiple_files_is_files_mode() {
        let decision = decide_mode(&[PathBuf::from("a.txt"), PathBuf::from("b.css")], |_| false);
        assert_eq!(decision, Ok(ModeDecision::Files));
    }

    #[test]
    fn directory_mixed_with_a_file_is_an_error() {
        // `is_dir` returns true only for "somedir"; mixing it with a file is a
        // hard startup error so it can never reach the hanging serve path.
        let files = [PathBuf::from("a.txt"), PathBuf::from("somedir")];
        let decision = decide_mode(&files, |p| p == std::path::Path::new("somedir"));
        assert_eq!(
            decision,
            Err(ModeError::DirectoryMixedWithFiles("somedir".to_string()))
        );
    }

    #[test]
    fn file_mixed_with_a_directory_in_either_order_is_an_error() {
        // Same as above, reversed order: the error must not depend on position.
        let files = [PathBuf::from("somedir"), PathBuf::from("a.txt")];
        let decision = decide_mode(&files, |p| p == std::path::Path::new("somedir"));
        assert_eq!(
            decision,
            Err(ModeError::DirectoryMixedWithFiles("somedir".to_string()))
        );
    }
}

#[cfg(test)]
mod listing_tests {
    use super::{html_escape, render_directory_listing};

    /// Builds an owned `(name, is_dir)` entry list from string slices.
    fn entries(items: &[(&str, bool)]) -> Vec<(String, bool)> {
        items
            .iter()
            .map(|(name, is_dir)| ((*name).to_string(), *is_dir))
            .collect()
    }

    #[test]
    fn html_escape_escapes_all_five() {
        let escaped = html_escape("a<b>&\"'c");
        assert!(escaped.contains("&lt;"), "should escape '<', got {escaped}");
        assert!(escaped.contains("&gt;"), "should escape '>', got {escaped}");
        assert!(
            escaped.contains("&amp;"),
            "should escape '&', got {escaped}"
        );
        assert!(
            escaped.contains("&quot;"),
            "should escape '\"', got {escaped}"
        );
        assert!(
            escaped.contains("&#39;"),
            "should escape '\\'' as &#39;, got {escaped}"
        );
        assert!(
            !escaped.contains('<'),
            "no raw '<' should remain, got {escaped}"
        );
    }

    #[test]
    fn listing_lists_entry_names() {
        let html = render_directory_listing("/", &entries(&[("a.txt", false), ("docs", true)]));
        assert!(html.contains("a.txt"), "should list a.txt, got:\n{html}");
        assert!(html.contains("docs"), "should list docs, got:\n{html}");
    }

    #[test]
    fn listing_marks_directories() {
        let html = render_directory_listing("/", &entries(&[("a.txt", false), ("docs", true)]));
        assert!(
            html.contains("docs/"),
            "directory should be marked with a trailing slash, got:\n{html}"
        );
        assert!(
            html.contains("href=\"/docs/\""),
            "directory href should end in docs/, got:\n{html}"
        );
    }

    #[test]
    fn listing_html_escapes_names() {
        let html = render_directory_listing("/", &entries(&[("a<b>.txt", false)]));
        assert!(
            html.contains("a&lt;b&gt;.txt"),
            "name should be HTML-escaped, got:\n{html}"
        );
        assert!(
            !html.contains("a<b>.txt"),
            "raw unescaped name must not appear, got:\n{html}"
        );
    }

    #[test]
    fn root_listing_has_no_parent_link() {
        let html = render_directory_listing("/", &entries(&[("a.txt", false)]));
        assert!(
            !html.contains(">../<"),
            "root listing must not include a parent link, got:\n{html}"
        );
    }

    #[test]
    fn subdir_listing_has_parent_link() {
        let html = render_directory_listing("/sub/", &entries(&[("a.txt", false)]));
        assert!(
            html.contains(">../<"),
            "subdir listing should include a `../` parent link, got:\n{html}"
        );
        assert!(
            html.contains("href=\"/\""),
            "subdir parent link should point to /, got:\n{html}"
        );
    }

    #[test]
    fn nested_listing_parent_points_one_level_up() {
        let html = render_directory_listing("/a/b/", &entries(&[("c.txt", false)]));
        assert!(
            html.contains("href=\"/a/\""),
            "nested parent link should point to /a/, got:\n{html}"
        );
    }

    #[test]
    fn entry_hrefs_are_absolute() {
        let html = render_directory_listing("/sub/", &entries(&[("c.txt", false)]));
        assert!(
            html.contains("href=\"/sub/c.txt\""),
            "entry href should be absolute, got:\n{html}"
        );
    }

    #[test]
    fn utf8_names_do_not_panic() {
        // Multi-byte UTF-8 names must render without any byte-index slicing panic.
        let html = render_directory_listing(
            "/",
            &entries(&[("日本語.txt", false), ("café", true), ("🎉", true)]),
        );
        assert!(html.contains("café/"), "café/ should appear, got:\n{html}");
        assert!(html.contains("🎉/"), "🎉/ should appear, got:\n{html}");
    }
}

#[cfg(test)]
mod confinement_tests {
    use super::{resolve_under_root, PathResolution};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Creates a temp dir and returns it alongside its canonicalized path.
    ///
    /// The root MUST be canonicalized: on macOS `TempDir` lives under `/var`,
    /// which is a symlink to `/private/var`, so an uncanonicalized root would make
    /// every `starts_with` check fail spuriously.
    fn canonical_root() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("temp dir");
        let root = dir.path().canonicalize().expect("canonicalize root");
        (dir, root)
    }

    #[test]
    fn root_request_resolves_to_the_root() {
        let (_dir, root) = canonical_root();
        assert_eq!(
            resolve_under_root(&root, "/"),
            PathResolution::Allowed(root.clone())
        );
    }

    #[test]
    fn in_root_file_resolves_to_its_canonical_path() {
        let (_dir, root) = canonical_root();
        let file = root.join("a.txt");
        std::fs::write(&file, b"hi").expect("write file");
        let expected = file.canonicalize().expect("canonicalize file");
        assert_eq!(
            resolve_under_root(&root, "/a.txt"),
            PathResolution::Allowed(expected)
        );
    }

    #[test]
    fn nested_in_root_file_resolves() {
        let (_dir, root) = canonical_root();
        let sub = root.join("sub");
        std::fs::create_dir(&sub).expect("create sub dir");
        let file = sub.join("b.txt");
        std::fs::write(&file, b"hi").expect("write file");
        let expected = file.canonicalize().expect("canonicalize file");
        assert_eq!(
            resolve_under_root(&root, "/sub/b.txt"),
            PathResolution::Allowed(expected)
        );
    }

    #[test]
    fn parent_traversal_is_forbidden() {
        let (_dir, root) = canonical_root();
        assert_eq!(
            resolve_under_root(&root, "/../../etc/passwd"),
            PathResolution::Forbidden
        );
    }

    #[test]
    fn dotdot_anywhere_is_forbidden() {
        let (_dir, root) = canonical_root();
        assert_eq!(
            resolve_under_root(&root, "/sub/../../x"),
            PathResolution::Forbidden
        );
    }

    #[test]
    fn missing_in_root_path_is_missing() {
        let (_dir, root) = canonical_root();
        assert_eq!(
            resolve_under_root(&root, "/does-not-exist.txt"),
            PathResolution::Missing
        );
    }

    #[test]
    fn absolute_looking_request_stays_in_root() {
        let (_dir, root) = canonical_root();
        // `/etc/passwd` must be rebuilt under the root, never resolving to the
        // real system file. Since `root/etc/passwd` does not exist, this is
        // `Missing`; it must never be an Allowed path outside the root.
        let resolution = resolve_under_root(&root, "/etc/passwd");
        assert_ne!(
            resolution,
            PathResolution::Allowed(PathBuf::from("/etc/passwd"))
        );
        if let PathResolution::Allowed(path) = &resolution {
            assert!(
                path.starts_with(&root),
                "an Allowed path must stay under the root, got {path:?}"
            );
        }
        assert_eq!(resolution, PathResolution::Missing);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_forbidden() {
        let (_dir, root) = canonical_root();
        // A secret directory living OUTSIDE the served root.
        let outside = TempDir::new().expect("outside temp dir");
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, b"top secret").expect("write secret");

        // A symlink that sits textually inside the root but points outside it.
        std::os::unix::fs::symlink(outside.path(), root.join("link")).expect("create symlink");

        // The symlink target canonicalizes outside the root, so it is rejected.
        assert_eq!(
            resolve_under_root(&root, "/link"),
            PathResolution::Forbidden
        );
        assert_eq!(
            resolve_under_root(&root, "/link/secret.txt"),
            PathResolution::Forbidden
        );
    }

    #[test]
    fn utf8_request_path_does_not_panic() {
        let (_dir, root) = canonical_root();
        // A multi-byte request path for a file that does not exist must resolve
        // to `Missing` without any byte-index slicing panic.
        assert_eq!(
            resolve_under_root(&root, "/日本語.txt"),
            PathResolution::Missing
        );
    }
}
