//! `sirn` ("Serve It Right Now") — a tiny, zero-config HTTP file server.
//!
//! This library crate holds the reusable pieces of the `sirn` binary so they can
//! be exercised directly by unit tests. The first such piece is the
//! [`content_type_for`] extension → MIME lookup used to label served files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

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

/// Handles one request: looks up its path in `routes` and streams the file.
///
/// The lookup path is the request URL with any `?query` stripped (no
/// percent-decoding — exact match). An unregistered path or a file that cannot
/// be opened on disk yields `404`; an openable file (even empty) streams as a
/// `200` with a `Content-Length` (set by `tiny_http` from the file size) and a
/// `Content-Type` from the file's extension.
fn respond(routes: &BTreeMap<String, PathBuf>, request: tiny_http::Request) -> std::io::Result<()> {
    let path = request.url().split('?').next().unwrap_or("");

    let Some(file_path) = routes.get(path) else {
        return request.respond(tiny_http::Response::empty(404));
    };

    let Ok(file) = std::fs::File::open(file_path) else {
        return request.respond(tiny_http::Response::empty(404));
    };

    // The header name and value are compile-time-known-valid, so the only
    // `expect` on the request path can never fire.
    let content_type =
        tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type_for(file_path).as_bytes())
            .expect("static Content-Type header is always valid");

    let response = tiny_http::Response::from_file(file).with_header(content_type);
    request.respond(response)
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
        String::new()
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
        assert_eq!(
            transition.message(),
            "Warning!  File foo.txt not found..."
        );
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
