//! Byte work on the string a symbolic link holds.
//!
//! A link target is a sequence of bytes that the operating system never reads
//! as text, so this crate carries one in an [`OsString`] and never in a
//! [`String`]. `OsStr` offers no byte view on every platform, thus the one
//! operation this tool needs on those bytes lives here, with one implementation
//! for each platform and one call site.

use std::ffi::{OsStr, OsString};

/// Removes `prefix` from the front of `target`, when `target` starts with it.
///
/// This is a raw byte prefix, as `strings.HasPrefix` is in the tool this port
/// replaces, and not a path-component prefix.
/// [`Path::strip_prefix`](std::path::Path::strip_prefix) answers a different
/// question: it compares whole components, so it would refuse
/// `--remove-to-fix /old/pa` on the target `/old/path/foo`, which the tool this
/// port replaces accepts.
///
/// `None` says the target does not start with the prefix. There is then no
/// candidate to try, and the remove strategy makes no repair.
#[cfg(unix)]
#[must_use]
pub fn strip_prefix(target: &OsStr, prefix: &OsStr) -> Option<OsString> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    // `<[u8]>::strip_prefix` reads the bytes and never indexes them, so a
    // prefix that ends inside a multi-byte character takes the bytes it names
    // and leaves the rest whole, rather than panicking on a boundary.
    target
        .as_bytes()
        .strip_prefix(prefix.as_bytes())
        .map(|rest| OsString::from_vec(rest.to_vec()))
}

/// The same, on a platform with no byte view of an `OsStr`.
///
/// A target that is not UTF-8 gives `None` here, thus the prefix does not match
/// and the tool makes no repair. It never guesses at bytes it cannot read.
#[cfg(not(unix))]
#[must_use]
pub fn strip_prefix(target: &OsStr, prefix: &OsStr) -> Option<OsString> {
    // `str::strip_prefix` also reads without indexing, so this half carries the
    // same guarantee on the text it can read.
    let (target, prefix) = (target.to_str()?, prefix.to_str()?);
    target.strip_prefix(prefix).map(OsString::from)
}

#[cfg(test)]
mod tests {
    use super::strip_prefix;
    use std::ffi::{OsStr, OsString};

    #[test]
    fn a_target_that_starts_with_the_prefix_loses_it() {
        assert_eq!(
            strip_prefix(OsStr::new("/old/path/foo"), OsStr::new("/old")),
            Some(OsString::from("/path/foo"))
        );
    }

    #[test]
    fn a_target_that_does_not_start_with_the_prefix_gives_nothing() {
        assert_eq!(
            strip_prefix(OsStr::new("/new/path/foo"), OsStr::new("/old")),
            None
        );
    }

    #[test]
    fn an_empty_prefix_leaves_the_target_whole() {
        assert_eq!(
            strip_prefix(OsStr::new("/old/path/foo"), OsStr::new("")),
            Some(OsString::from("/old/path/foo"))
        );
    }

    #[test]
    fn a_prefix_that_is_the_whole_target_leaves_nothing() {
        assert_eq!(
            strip_prefix(OsStr::new("/old"), OsStr::new("/old")),
            Some(OsString::new())
        );
    }

    #[test]
    fn the_prefix_comes_off_by_bytes_and_not_by_path_components() {
        // `Path::strip_prefix` compares whole components, so it would refuse
        // this pair and give back nothing. The tool this port replaces accepts
        // it, and so does this function.
        assert_eq!(
            strip_prefix(OsStr::new("/old/path/foo"), OsStr::new("/old/pa")),
            Some(OsString::from("th/foo"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_target_that_is_not_utf8_keeps_its_bytes() {
        // The byte `0x80` never begins a UTF-8 sequence, so this target is not
        // text and a strip that went through `str` would refuse it. The bytes
        // that follow the prefix come back exactly as they went in.
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let target = OsString::from_vec(b"junk\x80/target.txt".to_vec());

        let stripped = strip_prefix(&target, OsStr::new("junk")).unwrap();

        assert_eq!(stripped.as_bytes(), b"\x80/target.txt");
    }
}
