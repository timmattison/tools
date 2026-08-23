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

use crate::record::{SourceKind, SourceLabel};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, UdpSocket};
use std::path::{Path, PathBuf};

/// The extension of a recorded file.
const EXTENSION: &str = "jsonl";

/// The character that a file name always holds safely.
///
/// It replaces every character that a file name must not hold, and it joins the
/// two halves of a derived name.
const HYPHEN: char = '-';

/// The characters that a file name must not hold, as a list.
///
/// A colon names a drive on Windows and parts a host from a port everywhere. A
/// forward slash parts two names of a path, and a backward slash does the same
/// on Windows.
///
/// Whitespace becomes a hyphen too, and this list does not hold it.
/// `char::is_whitespace` reads the whole Unicode table, and no list of a few
/// characters holds as much.
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
pub(crate) fn output_path(named: Option<&Path>, source: IpAddr, destination: &str) -> PathBuf {
    named.map_or_else(
        || PathBuf::from(derive_name(source, destination)),
        Path::to_path_buf,
    )
}

/// The port that the socket records as its peer.
///
/// The value does not matter, because the socket sends no packet. It is not
/// zero, because some systems refuse a connect to port zero. 33434 is the port
/// that traceroute probes, so the number tells a reader what the socket is
/// for.
const PROBE_PORT: u16 = 33434_u16;

/// The port that the operating system picks for the socket.
///
/// A bind to port zero asks the operating system for a free port. The socket
/// never listens, so the number it picks is of no interest.
const ANY_PORT: u16 = 0_u16;

/// The address to bind before the socket records its peer: the unspecified
/// address of the family of the target.
///
/// A socket bound to one family refuses a connect to the other one, so the bind
/// reads the family of the target and never guesses it.
fn bind_address(target: IpAddr) -> IpAddr {
    match target {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    }
}

/// The address of the interface that reaches the target.
///
/// The socket sends no packet. A `connect` on a datagram socket only records
/// the peer, so the operating system picks the route and fills in the local
/// address, and nothing leaves the machine.
///
/// # Errors
///
/// Returns the reason when the socket does not open, when the operating system
/// finds no route to the target, and when the local address does not read.
fn egress_address(target: IpAddr) -> std::io::Result<IpAddr> {
    let socket = UdpSocket::bind((bind_address(target), ANY_PORT))?;
    socket.connect((target, PROBE_PORT))?;
    Ok(socket.local_addr()?.ip())
}

/// Finds the address that the probes leave from, and how krt found it.
///
/// The address that the user named wins, and it opens no socket. Every other
/// run reads the address of the interface that reaches the target.
///
/// The design puts one HTTPS request to a public address service between those
/// two steps. A later slice adds it.
///
/// # Errors
///
/// Returns the reason when the socket of the egress address does not open, when
/// the operating system finds no route to the target, and when the local
/// address does not read. A run that names a source raises none of these,
/// because it opens no socket.
pub(crate) fn discover(named: Option<IpAddr>, target: IpAddr) -> std::io::Result<SourceLabel> {
    match named {
        Some(addr) => Ok(SourceLabel {
            addr,
            kind: SourceKind::Override,
        }),
        None => Ok(SourceLabel {
            addr: egress_address(target)?,
            kind: SourceKind::Local,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{bind_address, derive_name, discover, egress_address, output_path};
    use crate::record::SourceKind;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::path::{Path, PathBuf};

    /// The loopback address of IP version 4.
    ///
    /// Every machine holds a route to it, so a test that reads the egress
    /// address of it reaches no network and never flakes.
    const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

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

    /// A URL that carries a query string is a plain destination to read.
    ///
    /// Windows refuses a file name that holds a question mark, so a name that
    /// keeps one opens no file there.
    #[test]
    fn a_destination_that_holds_a_query_string_derives_a_legal_name() {
        assert_eq!(
            name_of(SOURCE, "https://example.com/p?q=1"),
            "1.2.3.4-https---example.com-p-q=1.jsonl"
        );
    }

    /// Windows reserves six characters that no path of POSIX refuses.
    ///
    /// A name that keeps one of them opens no file on Windows, and the run
    /// stops at the first write.
    #[test]
    fn a_destination_that_holds_the_characters_windows_reserves_derives_a_legal_name() {
        assert_eq!(
            name_of(SOURCE, r#"a?b*c<d>e"f|g"#),
            "1.2.3.4-a-b-c-d-e-f-g.jsonl"
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

    /// The socket of the egress address sends no packet, so this test touches
    /// no network. A `connect` on a datagram socket only records the peer: the
    /// operating system picks the route and fills in the local address, and
    /// that is a system call and not a packet. The loopback route stands on
    /// every machine, so the test never flakes. Do not delete it as a network
    /// test.
    #[test]
    fn the_egress_address_of_the_loopback_of_ip_version_4_is_that_loopback() {
        let local = egress_address(LOOPBACK).expect("every machine holds a loopback route");
        assert_eq!(local, LOOPBACK);
    }

    /// The bind reads the family of the target and never guesses it.
    ///
    /// This test is pure, so it also holds on a machine that has turned IP
    /// version 6 off. A socket test of the loopback of IP version 6 would fail
    /// on such a machine for a reason that this code does not own.
    #[test]
    fn the_bind_address_of_a_target_of_ip_version_4_is_the_unspecified_address_of_that_family() {
        assert_eq!(
            bind_address(address(SOURCE)),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
    }

    /// The bind reads the family of the target and never guesses it. This test
    /// is pure, so it holds on a machine that has turned IP version 6 off.
    #[test]
    fn the_bind_address_of_a_target_of_ip_version_6_is_the_unspecified_address_of_that_family() {
        assert_eq!(
            bind_address(address(SOURCE_VERSION_6)),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED)
        );
    }

    /// A source that the user named opens no socket, and the record marks it an
    /// override.
    #[test]
    fn a_source_that_the_user_named_is_an_override() {
        let named = address(SOURCE);
        let label = discover(Some(named), LOOPBACK).expect("a named source opens no socket");
        assert_eq!(label.addr, named);
        assert_eq!(label.kind, SourceKind::Override);
    }

    /// A run that names no source reads the egress address, and the record
    /// marks it local. The target is the loopback, so the test touches no
    /// network.
    #[test]
    fn no_named_source_gives_the_local_egress_address() {
        let label = discover(None, LOOPBACK).expect("every machine holds a loopback route");
        assert_eq!(label.addr, LOOPBACK);
        assert_eq!(label.kind, SourceKind::Local);
    }
}
