//! `sirn` ("Serve It Right Now") — a tiny, zero-config HTTP file server.
//!
//! This library crate holds the reusable pieces of the `sirn` binary so they can
//! be exercised directly by unit tests. The first such piece is the
//! [`content_type_for`] extension → MIME lookup used to label served files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
