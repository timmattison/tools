//! `sirn` ("Serve It Right Now") — a tiny, zero-config HTTP file server.
//!
//! This library crate holds the reusable pieces of the `sirn` binary so they can
//! be exercised directly by unit tests. The first such piece is the
//! [`content_type_for`] extension → MIME lookup used to label served files.

use std::path::Path;

/// Returns the HTTP `Content-Type` for a file, based on its extension.
///
/// The lookup is case-insensitive (`.HTML`, `.Png`, and `.JSON` resolve the same
/// as their lowercase forms). Files with no extension, a non-UTF-8 extension, or
/// an unrecognized extension fall back to `application/octet-stream`. Textual
/// types carry a `; charset=utf-8` parameter; binary types do not.
///
/// This function never panics.
#[must_use]
pub fn content_type_for(_path: &Path) -> &'static str {
    // Deliberately-wrong stub: the real lookup table is implemented in the GREEN
    // step. Returning the fallback unconditionally makes the characterization
    // tests fail on their assertions rather than on a missing symbol.
    "application/octet-stream"
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
