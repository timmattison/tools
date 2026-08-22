//! Where the probes leave from, and what the recorded file is called.
//!
//! The name of a recorded file holds the source address and the destination, so
//! one source and one destination keep one file across many runs. Both halves
//! of the name lose every character that a file name must not hold. The
//! `--output` flag names a file of its own, and it wins over the derived name.
//!
//! The derived name carries the address that the probes leave from. A file that
//! the user gives to another person carries that address too. `--output`
//! avoids this.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// The extension of a recorded file.
#[allow(
    dead_code,
    reason = "main derives the file name and the source beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
const EXTENSION: &str = "jsonl";

/// The character that a file name always holds safely.
///
/// It replaces every character that a file name must not hold, and it joins the
/// two halves of a derived name.
#[allow(
    dead_code,
    reason = "main derives the file name and the source beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
const HYPHEN: char = '-';

/// Every character that a file name must not hold.
///
/// A colon names a drive on Windows and parts a host from a port everywhere. A
/// forward slash parts two names of a path, and a backward slash does the same
/// on Windows. Whitespace is not forbidden, but a name that holds it needs a
/// quote at every command line that reads it.
#[allow(
    dead_code,
    reason = "main derives the file name and the source beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
const FORBIDDEN: [char; 3] = [':', '/', '\\'];

/// Replaces every character that a file name must not hold.
///
/// A colon, a forward slash, a backward slash, and a space each become one
/// hyphen. Every other character stays, so a destination that holds Japanese
/// characters keeps them.
///
/// The walk reads characters and never bytes, so a character of more than one
/// byte survives whole. `char::is_whitespace` reads the Unicode table, so a
/// space such as U+3000 IDEOGRAPHIC SPACE becomes a hyphen as an ASCII space
/// does.
#[allow(
    dead_code,
    reason = "main derives the file name and the source beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|character| {
            if FORBIDDEN.contains(&character) || character.is_whitespace() {
                HYPHEN
            } else {
                character
            }
        })
        .collect()
}

/// Builds the name of the recorded file from the source and the destination.
///
/// The name is `SOURCE-DESTINATION.jsonl`, and both halves lose the characters
/// that a file name must not hold. A source of IP version 6 therefore gives a
/// name such as `2001-db8--1-example.com.jsonl`.
#[allow(
    dead_code,
    reason = "main derives the file name and the source beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
fn derive_name(source: IpAddr, destination: &str) -> String {
    format!(
        "{}{HYPHEN}{}.{EXTENSION}",
        sanitize(&source.to_string()),
        sanitize(destination)
    )
}

/// The file that a run records to.
///
/// The file that the user named wins over the derived name. The derived name
/// carries no directory, so it lands in the working directory.
#[allow(
    dead_code,
    reason = "main derives the file name and the source beside the run loop, and the run loop arrives in a later slice of issue #367"
)]
pub(crate) fn output_path(named: Option<&Path>, source: IpAddr, destination: &str) -> PathBuf {
    named.map_or_else(
        || PathBuf::from(derive_name(source, destination)),
        Path::to_path_buf,
    )
}

#[cfg(test)]
mod tests {
    use super::{derive_name, output_path};
    use std::net::IpAddr;
    use std::path::{Path, PathBuf};

    /// The source address of every test that names a plain one.
    const SOURCE: &str = "1.2.3.4";

    /// The destination of every test that names a plain one.
    const DESTINATION: &str = "example.com";

    /// The name that the plain source and the plain destination derive.
    const PLAIN_NAME: &str = "1.2.3.4-example.com.jsonl";

    /// A source address of IP version 6. Its text holds four colons.
    const SOURCE_VERSION_6: &str = "2001:db8::1";

    /// The file that a test names on the command line.
    const NAMED_FILE: &str = "elsewhere/trace.jsonl";

    /// Reads an address that a test names.
    fn address(text: &str) -> IpAddr {
        text.parse().expect("the test address must parse")
    }

    /// The name that a source and a destination of a test derive.
    fn name_of(source: &str, destination: &str) -> String {
        derive_name(address(source), destination)
    }

    #[test]
    fn a_plain_source_and_a_plain_destination_derive_the_plain_name() {
        assert_eq!(name_of(SOURCE, DESTINATION), PLAIN_NAME);
    }

    #[test]
    fn a_source_of_ip_version_6_derives_a_name_that_holds_no_colon() {
        assert_eq!(
            name_of(SOURCE_VERSION_6, DESTINATION),
            "2001-db8--1-example.com.jsonl"
        );
    }

    #[test]
    fn a_destination_that_holds_a_url_derives_a_legal_name() {
        assert_eq!(
            name_of(SOURCE, "https://example.com/path"),
            "1.2.3.4-https---example.com-path.jsonl"
        );
    }

    #[test]
    fn a_destination_that_holds_a_backward_slash_derives_a_legal_name() {
        assert_eq!(
            name_of(SOURCE, r"example.com\share"),
            "1.2.3.4-example.com-share.jsonl"
        );
    }

    #[test]
    fn a_destination_that_holds_a_space_derives_a_legal_name() {
        assert_eq!(name_of(SOURCE, "example com"), "1.2.3.4-example-com.jsonl");
    }

    /// U+3000 IDEOGRAPHIC SPACE is a space to Unicode and no space to ASCII.
    ///
    /// A rule that reads bytes, or that asks `is_ascii_whitespace`, keeps this
    /// character and writes a name that a command line then needs a quote for.
    #[test]
    fn a_destination_that_holds_an_ideographic_space_derives_a_legal_name() {
        assert_eq!(
            name_of(SOURCE, "example\u{3000}com"),
            "1.2.3.4-example-com.jsonl"
        );
    }

    /// A character of more than one byte survives the walk whole.
    ///
    /// The Japanese characters hold three bytes each, and the emoji holds four.
    /// A walk that reads bytes cuts such a character in half and panics.
    #[test]
    fn a_destination_that_holds_multi_byte_characters_keeps_them() {
        assert_eq!(
            name_of(SOURCE, "日本語.example.com"),
            "1.2.3.4-日本語.example.com.jsonl"
        );
        assert_eq!(
            name_of(SOURCE, "🎉.example.com"),
            "1.2.3.4-🎉.example.com.jsonl"
        );
    }

    /// The file that the user named wins, and the derivation never runs.
    ///
    /// The source and the destination of this test derive a name of their own,
    /// and the result holds no part of it. `output_path` builds the derived
    /// name inside a closure, so an argument that the user named leaves that
    /// closure unread.
    #[test]
    fn a_named_file_wins_over_the_derived_name() {
        let named = PathBuf::from(NAMED_FILE);
        let path = output_path(
            Some(&named),
            address(SOURCE_VERSION_6),
            "https://example.com/path",
        );
        assert_eq!(path, named);
    }

    #[test]
    fn no_named_file_gives_the_derived_name_in_the_working_directory() {
        let path = output_path(None, address(SOURCE), DESTINATION);
        assert_eq!(path, PathBuf::from(PLAIN_NAME));
        assert!(
            path.is_relative(),
            "the derived name is a bare relative path: {}",
            path.display()
        );
        assert_eq!(
            path.parent(),
            Some(Path::new("")),
            "the derived name carries no directory: {}",
            path.display()
        );
    }
}
