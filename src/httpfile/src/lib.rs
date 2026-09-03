//! `httpfile` — shared static-file serving primitives for `tiny_http` servers.
//!
//! `localnext` and `sirn` each grew their own copy of a content-type lookup
//! table, a path-confinement traversal guard, and the small file-streaming
//! helper that uses them — and the copies drifted: each content-type table
//! gained extensions the other lacked. This crate is the one home for all
//! three, so every static-file server in this workspace shares one
//! content-type table and one traversal guard. A fix to either reaches every
//! caller instead of having to be repeated per crate.

use std::path::{Path, PathBuf};

/// The content type of a file whose extension names nothing recognizable.
const OCTET_STREAM: &str = "application/octet-stream";

/// Returns the HTTP `Content-Type` for a file, based on its extension.
///
/// The lookup is case-insensitive (`.CSS`, `.Png`, and `.JSON` resolve the
/// same as their lowercase forms). A file with no extension, a non-UTF-8
/// extension, or an unrecognized extension falls back to
/// `application/octet-stream`. Textual types carry a `; charset=utf-8`
/// parameter; binary types do not.
///
/// `tiny_http` sends whatever header a response carries and nothing more — it
/// does no content-type sniffing the way Go's `http.ServeFile` does — so this
/// table is what fills that gap for every static-file server in this
/// workspace: markup, stylesheets, the JavaScript chunks a bundler emits and
/// their `.map` source maps, the web app manifest, fonts, images (including
/// `.avif`), Markdown and CSV documents, and the media and archive types a
/// served directory or export may otherwise hold. Without it a file goes out
/// with the wrong (or no) `Content-Type` and the browser silently refuses to
/// treat it as what it is — a `.css` file, say, renders as `text/plain` with
/// no error anywhere.
///
/// `application/json` and a `.map` source map are conventionally served
/// without a charset parameter. This function never indexes a string by
/// bytes, so a multi-byte extension is handled like any other and this
/// function never panics.
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
        "md" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
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

/// The result of resolving a request path against a confinement root.
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

/// Resolves a request path against `root`, confining the result to it.
///
/// `root` MUST already be canonicalized by the caller — the containment check
/// below compares canonical paths. `url_path` is typically a request URL path
/// with a leading `/` and any `?query` already stripped (e.g. `/`,
/// `/sub/file.txt`), but the leading `/` is only a `Component::RootDir` like
/// any other and is simply skipped, so a plain relative path with no leading
/// slash resolves identically — this crate's two callers each pass one shape.
///
/// The path is rebuilt from only its normal components: a leading `/` (root)
/// and `.` (current dir) components are skipped, while any `..` (parent) or
/// Windows prefix component is treated as an escape attempt and rejected
/// outright. The candidate is then canonicalized — which resolves symlinks —
/// and confirmed to still live under `root`, so a symlink that sits inside
/// the root textually but points outside it is rejected too. The textual
/// component check alone cannot see that link; the canonical containment
/// check alone would accept a `..` that lands back inside the root. Both are
/// needed.
///
/// Rejecting every `..` outright is stricter than Go's `filepath.Join`
/// behavior, which hands `a/../b` to `filepath.Join` and resolves it back
/// inside the root. The stricter rule is deliberate: no legitimate request
/// from a static export or a served directory carries a `..`, and a rule
/// with no exceptions is a rule with no gaps.
///
/// Returns [`PathResolution::Forbidden`] for an escape attempt, `Missing`
/// when the in-root path does not exist, and `Allowed(canonical)` otherwise.
#[must_use]
pub fn resolve_under_root(root: &Path, url_path: &str) -> PathResolution {
    use std::path::Component;

    // Rebuild the request path from only its normal components. A `..` or a
    // Windows prefix is a textual escape attempt and is rejected outright;
    // the leading `/` (root) and any `.` (current dir) are simply skipped.
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

/// Opens `path` only if it is a regular file, returning `None` otherwise.
///
/// A directory, a missing path, or any other non-regular entry yields `None`.
/// This guards the streaming path: a directory opens successfully on Unix,
/// so advertising its metadata length and then failing to produce bytes
/// would hang the client forever waiting for a body that never arrives.
fn open_regular_file(path: &Path) -> Option<std::fs::File> {
    let file = std::fs::File::open(path).ok()?;
    file.metadata().ok()?.is_file().then_some(file)
}

/// Streams `path` to `request`, or responds `404` if it is not a regular file.
///
/// `path` is opened through [`open_regular_file`], so a missing path, a
/// directory, or any other non-regular entry yields `404` — never a hung
/// stream. A regular file (even an empty one) streams as a `200` with a
/// `Content-Type` from [`content_type_for`] and a `Content-Length` that
/// `tiny_http` sets from the file size.
///
/// # Errors
///
/// Returns an error only when writing the HTTP response itself fails (e.g.
/// the client disconnected mid-response).
///
/// # Panics
///
/// Never in practice: the one `expect` on the request path builds a header
/// from a compile-time-known-valid name and value, so it can never fire.
pub fn serve_file(path: &Path, request: tiny_http::Request) -> std::io::Result<()> {
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
    use super::content_type_for;
    use std::path::Path;

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
        assert_eq!(ct("style.css"), "text/css; charset=utf-8");
    }

    #[test]
    fn javascript_is_javascript() {
        assert_eq!(ct("chunk.js"), "text/javascript; charset=utf-8");
        assert_eq!(ct("chunk.mjs"), "text/javascript; charset=utf-8");
    }

    #[test]
    fn js_and_mjs_are_javascript() {
        assert_eq!(ct("app.js"), "text/javascript; charset=utf-8");
        assert_eq!(ct("module.mjs"), "text/javascript; charset=utf-8");
    }

    #[test]
    fn json_and_source_maps_are_json_without_a_charset() {
        assert_eq!(ct("build-manifest.json"), "application/json");
        assert_eq!(ct("chunk.js.map"), "application/json");
    }

    #[test]
    fn json_has_no_charset() {
        assert_eq!(ct("data.json"), "application/json");
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
    fn svg_is_svg() {
        assert_eq!(ct("logo.svg"), "image/svg+xml");
    }

    #[test]
    fn image_types() {
        assert_eq!(ct("logo.png"), "image/png");
        assert_eq!(ct("hero.jpg"), "image/jpeg");
        assert_eq!(ct("hero.jpeg"), "image/jpeg");
        assert_eq!(ct("photo.jpg"), "image/jpeg");
        assert_eq!(ct("photo.jpeg"), "image/jpeg");
        assert_eq!(ct("spin.gif"), "image/gif");
        assert_eq!(ct("anim.gif"), "image/gif");
        assert_eq!(ct("hero.webp"), "image/webp");
        assert_eq!(ct("pic.webp"), "image/webp");
        assert_eq!(ct("hero.avif"), "image/avif");
        assert_eq!(ct("icon.svg"), "image/svg+xml");
        assert_eq!(ct("favicon.ico"), "image/x-icon");
    }

    #[test]
    fn font_types() {
        assert_eq!(ct("inter.woff"), "font/woff");
        assert_eq!(ct("inter.woff2"), "font/woff2");
        assert_eq!(ct("inter.ttf"), "font/ttf");
        assert_eq!(ct("font.woff"), "font/woff");
        assert_eq!(ct("font.woff2"), "font/woff2");
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
    fn document_and_binary_types() {
        assert_eq!(ct("doc.pdf"), "application/pdf");
        assert_eq!(ct("mod.wasm"), "application/wasm");
        assert_eq!(ct("archive.zip"), "application/zip");
        assert_eq!(ct("blob.gz"), "application/gzip");
    }

    #[test]
    fn media_types() {
        assert_eq!(ct("clip.mp4"), "video/mp4");
        assert_eq!(ct("song.mp3"), "audio/mpeg");
        assert_eq!(ct("sound.wav"), "audio/wav");
    }

    #[test]
    fn the_extension_match_is_case_insensitive() {
        assert_eq!(ct("APP.CSS"), "text/css; charset=utf-8");
        assert_eq!(ct("Index.HTML"), "text/html; charset=utf-8");
        assert_eq!(ct("LOGO.PnG"), "image/png");
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
    fn a_file_with_no_extension_is_an_opaque_byte_stream() {
        assert_eq!(ct("LICENSE"), "application/octet-stream");
        assert_eq!(ct(".gitignore"), "application/octet-stream");
    }

    #[test]
    fn an_unknown_extension_is_an_opaque_byte_stream() {
        assert_eq!(ct("archive.tar"), "application/octet-stream");
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

    #[test]
    fn a_multi_byte_extension_never_panics() {
        assert_eq!(ct("notes.日本語"), "application/octet-stream");
        assert_eq!(ct("café.html"), "text/html; charset=utf-8");
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
    /// which is a symlink to `/private/var`, so an uncanonicalized root would
    /// make every `starts_with` check fail spuriously.
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
mod range_tests {
    use super::{parse_range, RangeOutcome};

    #[test]
    fn exact_range_is_served() {
        assert_eq!(
            parse_range("bytes=2-5", 10),
            RangeOutcome::Partial { first: 2, last: 5 }
        );
    }

    #[test]
    fn a_single_byte_range_is_inclusive() {
        assert_eq!(
            parse_range("bytes=0-0", 10),
            RangeOutcome::Partial { first: 0, last: 0 }
        );
    }

    #[test]
    fn open_ended_range_runs_to_the_end() {
        assert_eq!(
            parse_range("bytes=2-", 10),
            RangeOutcome::Partial { first: 2, last: 9 }
        );
    }

    #[test]
    fn suffix_range_is_the_last_n_bytes() {
        assert_eq!(
            parse_range("bytes=-3", 10),
            RangeOutcome::Partial { first: 7, last: 9 }
        );
    }

    #[test]
    fn a_last_past_the_end_is_clamped() {
        assert_eq!(
            parse_range("bytes=0-999999", 10),
            RangeOutcome::Partial { first: 0, last: 9 }
        );
    }

    #[test]
    fn a_suffix_larger_than_the_file_is_clamped_to_the_whole_file() {
        assert_eq!(
            parse_range("bytes=-999999", 10),
            RangeOutcome::Partial { first: 0, last: 9 }
        );
    }

    #[test]
    fn first_at_or_past_the_end_is_unsatisfiable() {
        assert_eq!(parse_range("bytes=10-20", 10), RangeOutcome::Unsatisfiable);
        assert_eq!(parse_range("bytes=10-", 10), RangeOutcome::Unsatisfiable);
    }

    #[test]
    fn a_zero_length_suffix_is_unsatisfiable() {
        assert_eq!(parse_range("bytes=-0", 10), RangeOutcome::Unsatisfiable);
    }

    #[test]
    fn any_range_on_an_empty_file_is_unsatisfiable() {
        assert_eq!(parse_range("bytes=0-0", 0), RangeOutcome::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-", 0), RangeOutcome::Unsatisfiable);
        assert_eq!(parse_range("bytes=-5", 0), RangeOutcome::Unsatisfiable);
    }

    #[test]
    fn a_multi_range_request_is_ignored() {
        assert_eq!(parse_range("bytes=0-1,2-3", 10), RangeOutcome::Ignore);
    }

    #[test]
    fn a_non_bytes_unit_is_ignored() {
        assert_eq!(parse_range("items=0-1", 10), RangeOutcome::Ignore);
    }

    #[test]
    fn unparsable_text_is_ignored() {
        assert_eq!(parse_range("this makes no sense", 10), RangeOutcome::Ignore);
        assert_eq!(parse_range("bytes=", 10), RangeOutcome::Ignore);
        assert_eq!(parse_range("bytes=-", 10), RangeOutcome::Ignore);
    }

    #[test]
    fn a_reversed_range_is_ignored() {
        assert_eq!(parse_range("bytes=5-2", 10), RangeOutcome::Ignore);
    }

    #[test]
    fn a_multi_byte_header_never_panics_and_is_ignored() {
        assert_eq!(parse_range("byt\u{e9}s=0-1", 10), RangeOutcome::Ignore);
        assert_eq!(parse_range("bytes=\u{65e5}\u{672c}\u{8a9e}", 10), RangeOutcome::Ignore);
    }

    #[test]
    fn u64_max_first_is_unsatisfiable_without_overflow() {
        assert_eq!(
            parse_range("bytes=18446744073709551615-", 10),
            RangeOutcome::Unsatisfiable
        );
    }

    #[test]
    fn u64_max_suffix_is_clamped_without_overflow() {
        assert_eq!(
            parse_range("bytes=-18446744073709551615", 10),
            RangeOutcome::Partial { first: 0, last: 9 }
        );
    }
}
