//! Where the probes leave from, and what the recorded file is called.
//!
//! The search for the source holds three steps, and the first one that gives an
//! address wins: the address that the user named, the address that a public
//! service answers with, and the address of the local interface that reaches
//! the target. The public service is the one of the family of the target, and
//! an answer of the other family counts as no answer. The last step reaches no
//! network, so a machine on a captive network still records.
//!
//! The name of a recorded file holds the source address and the destination, so
//! one source and one destination keep one file across many runs. Both halves
//! of the name lose every character that a file name must not hold on macOS, on
//! Linux, or on Windows. The `--output` flag names a file of its own, and it
//! wins over the derived name.
//!
//! The derived name carries the address that the probes leave from, and that is
//! the public address of the machine on most runs. A file that the user gives
//! to another person therefore names the network of the user, and not only an
//! address of a private range that every home shares. `--output` avoids this.

use crate::record::{SourceKind, SourceLabel};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::Duration;

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
/// Windows reserves six more characters. A question mark and a star are
/// wildcards. An angle bracket of each side redirects the input or the output
/// of a command. A quotation mark encloses a name that holds a space. A
/// vertical bar joins two commands.
///
/// The list holds the characters of every platform, and not the characters of
/// one. A destination such as `https://example.com/p?q=1` therefore derives a
/// name that opens a file on Windows as it does on macOS and on Linux.
///
/// Whitespace becomes a hyphen too, and this list does not hold it.
/// `char::is_whitespace` reads the whole Unicode table, and no list of a few
/// characters holds as much.
const FORBIDDEN: [char; 9] = [':', '/', '\\', '?', '*', '<', '>', '"', '|'];

/// Replaces every character that a file name must not hold.
///
/// Each character of `FORBIDDEN` becomes one hyphen, and a space does the same.
/// Every other character stays, so a destination that holds Japanese characters
/// keeps them.
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
/// name such as `2001-db8--1-example.com.jsonl`, and a destination that holds a
/// URL with a query string gives a name such as
/// `1.2.3.4-https---example.com-p-q=1.jsonl`.
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

/// The service that answers with the address that the internet sees, for a
/// target of IP version 4.
///
/// The service answers a plain GET with the address as text and nothing else,
/// so the answer needs no parser of a format and the request needs no key of an
/// account. A service that goes away, or that starts to limit the rate of a
/// caller, costs this one line to change.
///
/// The host holds records of type A only, so the name resolves to an address of
/// IP version 4 and the request leaves on that family.
const PUBLIC_SERVICE_V4: &str = "https://api.ipify.org";

/// The service that answers with the address that the internet sees, for a
/// target of IP version 6.
///
/// The host holds records of type AAAA only, so the name resolves to an address
/// of IP version 6 and the request leaves on that family. It answers the same
/// plain GET with the address as text, as the service of IP version 4 does.
const PUBLIC_SERVICE_V6: &str = "https://api6.ipify.org";

/// The service that answers with the address of the family of the target.
///
/// A record of one family that carries a source of the other reads as a fault
/// of the tool, and it derives the file name that the run of the other family
/// derives, so the pick reads the family of the target and never guesses it.
///
/// The pick is a best effort, because it holds only while each host keeps the
/// records of one family. The check that [`public_address`] makes on the answer
/// is the guarantee.
fn public_service(target: IpAddr) -> &'static str {
    match target {
        IpAddr::V4(_) => PUBLIC_SERVICE_V4,
        IpAddr::V6(_) => PUBLIC_SERVICE_V6,
    }
}

/// How long the lookup of the public address waits before it gives up.
///
/// The lookup stands between the source that the user named and the local
/// egress address, so every run that names no source pays it. Three seconds is
/// long enough for a service on the other side of an ocean, and short enough
/// that a service that has broken does not hold the run for long.
///
/// The client gives this much time to the request and this much again to the
/// read of the answer, so a service that answers the headers and then stops
/// costs twice this at the most.
const PUBLIC_TIMEOUT: Duration = Duration::from_secs(3);

/// How many characters of an answer that is not an address the message holds.
///
/// A service that answers with a page of HTML, and not with an address, writes
/// that whole page to the warning line of the run without this limit. Sixty
/// characters name the service and the trouble.
///
/// These sixty characters are the characters of one line, because [`shorten`]
/// makes the answer one line before it counts them. A count of the characters
/// of the raw answer bounds no line: the first sixty characters of a page of
/// HTML hold three line breaks, so the warning breaks across lines and
/// [`PUBLIC_FALLBACK`] stands on a line of its own, away from the reason.
const ANSWER_LIMIT: usize = 60;

/// The character that marks an answer that the message cut.
const ELLIPSIS: &str = "…";

/// The character that stands in the place of every run of whitespace.
const SPACE: char = ' ';

/// The answer as one line, with no character that paints a terminal.
///
/// The answer is the text of a remote service, and the warning line goes
/// straight to the standard error of the user. Two kinds of character
/// therefore leave the answer here.
///
/// Every run of whitespace becomes one space. The line break is the character
/// that matters most: a page of HTML holds three of them inside its first
/// sixty characters, so an answer that keeps them breaks the warning across
/// lines. One space also spends less of [`ANSWER_LIMIT`] than a run of padding
/// does, so the limit buys words and not whitespace.
///
/// Every other control character goes. A line break is a control character and
/// a whitespace character both, and the rule of the whitespace wins, so a line
/// break becomes a space and not nothing. The escape character starts every
/// sequence that paints a terminal, and this walk drops that character alone.
/// The parameter letters of such a sequence stay as ordinary text. The warning
/// therefore paints nothing, and the reader still sees what arrived.
///
/// The result starts with no space and ends with no space.
///
/// The walk reads characters and never bytes, so a character of more than one
/// byte survives whole. `char::is_whitespace` reads the whole Unicode table,
/// as [`sanitize`] in this file does, so a space such as U+3000 IDEOGRAPHIC
/// SPACE collapses as an ASCII space does.
fn one_line(answer: &str) -> String {
    let mut cleaned = String::new();
    let mut space_is_due = false;
    for character in answer.chars() {
        if character.is_whitespace() {
            space_is_due = !cleaned.is_empty();
        } else if !character.is_control() {
            if space_is_due {
                cleaned.push(SPACE);
                space_is_due = false;
            }
            cleaned.push(character);
        }
    }
    cleaned
}

/// The start of an answer, as one line of a warning.
///
/// The answer becomes one line first, and the cut comes second, so
/// [`ANSWER_LIMIT`] counts the characters of one line and the ellipsis marks a
/// cut of the cleaned text. [`one_line`] says what the clean takes out.
///
/// The walk reads characters and never bytes, so a character of more than one
/// byte survives whole. An answer that the walk cut ends with an ellipsis, so a
/// reader sees that more of it exists.
fn shorten(answer: &str) -> String {
    let cleaned = one_line(answer);
    let mut characters = cleaned.chars();
    let start: String = characters.by_ref().take(ANSWER_LIMIT).collect();
    if characters.next().is_some() {
        format!("{start}{ELLIPSIS}")
    } else {
        start
    }
}

/// Why the lookup of the public address gives no address.
///
/// The text of each one becomes part of the warning line that the run prints
/// before it falls back to the local egress address.
#[derive(Debug, thiserror::Error)]
enum PublicError {
    /// The request did not complete.
    ///
    /// The client did not build, the name did not resolve, the connection did
    /// not open, the service answered with the status of an error, or the
    /// answer did not arrive inside the timeout.
    #[error("the request to the public address service did not complete: {reason}")]
    Request {
        /// The reason that the client gave.
        reason: String,
    },
    /// The answer is not an address.
    #[error("the public address service answered with text that is not an address: {answer}")]
    Answer {
        /// The start of the text that the service answered, as one line.
        ///
        /// [`shorten`] makes that one line, so the message of this variant
        /// stays on the one warning line of the run and paints no terminal.
        answer: String,
    },
    /// The answer is an address of the family that the target is not of.
    #[error(
        "the public address service answered with {answer}, which is not of the family of {target}"
    )]
    Family {
        /// The address that the service answered with.
        answer: IpAddr,
        /// The address of the target of the run.
        target: IpAddr,
    },
}

/// The address that the internet sees, from one GET of a public service.
///
/// The client of `reqwest` that blocks holds its `tokio` runtime inside itself
/// and drops that runtime when the call returns. `krt` therefore starts no
/// runtime of its own and holds no async code.
///
/// The caller names the timeout, so a test names a short one and runs fast. The
/// client gives that time to the request and that time again to the read of the
/// answer.
///
/// A status of an error is a failure. A service that answers `503` answers with
/// a page of its own, and that page is not an address.
///
/// The answer loses the whitespace of both ends before it parses, because a
/// service that ends its answer with a newline is a common one.
///
/// An address of the family that the target is not of is a failure. The caller
/// picks a host of the family of the target, and that pick holds only while the
/// host keeps the records of one family, so this check is the guarantee. A
/// record of one family that carries a source of the other reads as a fault of
/// the tool, and it derives the file name that the run of the other family
/// derives, so two traces of two families then append to one file.
///
/// # Errors
///
/// Returns [`PublicError::Request`] when the client does not build, when the
/// request does not complete inside the timeout, when the service answers with
/// the status of an error, and when the answer does not read as text. Returns
/// [`PublicError::Answer`] when the answer is not an address. Returns
/// [`PublicError::Family`] when the answer is an address of the family that the
/// target is not of.
fn public_address(service: &str, target: IpAddr, timeout: Duration) -> Result<IpAddr, PublicError> {
    let answer = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .and_then(|client| client.get(service).send())
        .and_then(reqwest::blocking::Response::error_for_status)
        .and_then(reqwest::blocking::Response::text)
        .map_err(|error| PublicError::Request {
            reason: error.to_string(),
        })?;
    let trimmed = answer.trim();
    let found: IpAddr = trimmed.parse().map_err(|_| PublicError::Answer {
        answer: shorten(trimmed),
    })?;
    if found.is_ipv4() == target.is_ipv4() {
        Ok(found)
    } else {
        Err(PublicError::Family {
            answer: found,
            target,
        })
    }
}

/// What the warning of an unread public address says after the reason.
const PUBLIC_FALLBACK: &str = "The run records the local egress address in its place.";

/// What the search for the source address found.
#[derive(Debug)]
pub(crate) struct Discovery {
    /// The address that the probes leave from, and how `krt` found it.
    pub(crate) label: SourceLabel,
    /// The one line that names why the public service gave no address.
    ///
    /// `main` writes it to standard error before the display starts. A search
    /// that needed no fallback carries none.
    pub(crate) note: Option<String>,
}

/// Finds the address that the probes leave from, and how krt found it.
///
/// The search holds three steps, and the first one that gives an address wins.
/// The address that the user named wins over both of the others, and it asks no
/// service and opens no socket. A run that names no source asks the public
/// address service of the family of the target once, and an answer of the other
/// family counts as no answer. A run that reads no address there records the
/// address of the interface that reaches the target, and it carries the note
/// that says why.
///
/// The last step is what keeps a run recording: a machine on a captive network,
/// and a machine on a network with no route out, both reach the local egress
/// address.
///
/// # Errors
///
/// Returns the reason when the public lookup gives no address **and** the
/// socket of the egress address then also fails: the socket does not open, the
/// operating system finds no route to the target, or the local address does not
/// read. A run that names a source raises none of these, and neither does a run
/// that reads a public address, because neither one opens a socket.
pub(crate) fn discover(named: Option<IpAddr>, target: IpAddr) -> std::io::Result<Discovery> {
    discover_at(named, target, public_service(target), PUBLIC_TIMEOUT)
}

/// Finds the source address against the service and the timeout of the caller.
///
/// [`discover`] names the service and the timeout of a run. A test names a
/// service of its own, which runs on this machine, so no test of this module
/// reaches a public address service of the internet.
///
/// # Errors
///
/// Returns the reason when the public lookup gives no address and the socket of
/// the egress address then also fails.
fn discover_at(
    named: Option<IpAddr>,
    target: IpAddr,
    service: &str,
    timeout: Duration,
) -> std::io::Result<Discovery> {
    if let Some(addr) = named {
        return Ok(Discovery {
            label: SourceLabel {
                addr,
                kind: SourceKind::Override,
            },
            note: None,
        });
    }
    match public_address(service, target, timeout) {
        Ok(addr) => Ok(Discovery {
            label: SourceLabel {
                addr,
                kind: SourceKind::Public,
            },
            note: None,
        }),
        Err(error) => Ok(Discovery {
            label: SourceLabel {
                addr: egress_address(target)?,
                kind: SourceKind::Local,
            },
            note: Some(format!("{error}. {PUBLIC_FALLBACK}")),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bind_address, derive_name, discover_at, egress_address, output_path, public_address,
        public_service, Discovery, PublicError, ANSWER_LIMIT, ELLIPSIS, PUBLIC_FALLBACK,
        PUBLIC_SERVICE_V4, PUBLIC_SERVICE_V6,
    };
    use crate::record::SourceKind;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

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

    /// The lookup reads the family of the target and never guesses it.
    ///
    /// The host of the service of IP version 4 holds records of type A only, so
    /// a request to it leaves on IP version 4. A run of IP version 6 that asks
    /// that host reads the address of IP version 4 of the machine, and it
    /// writes that address into a record that names IP version 6.
    #[test]
    fn a_target_of_ip_version_4_asks_the_service_of_that_family() {
        assert_eq!(public_service(address(SOURCE)), PUBLIC_SERVICE_V4);
    }

    /// The lookup reads the family of the target and never guesses it.
    ///
    /// The host of the service of IP version 6 holds records of type AAAA only,
    /// so a request to it leaves on IP version 6. This test is pure, so it
    /// holds on a machine that has turned IP version 6 off.
    #[test]
    fn a_target_of_ip_version_6_asks_the_service_of_that_family() {
        assert_eq!(public_service(address(SOURCE_VERSION_6)), PUBLIC_SERVICE_V6);
    }

    /// The two families ask two hosts.
    ///
    /// One host for both families reads as a pick and is none, and a test of
    /// each family alone passes when the two constants hold one host. The two
    /// tests above also pass when the two constants swap, and this one fails
    /// with them.
    #[test]
    fn the_service_of_each_family_is_a_service_of_its_own() {
        assert_ne!(PUBLIC_SERVICE_V4, PUBLIC_SERVICE_V6);
    }

    /// The address that a mock service answers with.
    ///
    /// 203.0.113.0/24 is TEST-NET-3, which the registries hold for
    /// documentation, so no machine of the internet carries this address.
    const PUBLIC_ADDRESS: &str = "203.0.113.7";

    /// The address of IP version 6 that a mock service answers with.
    ///
    /// `2001:db8::/32` is the range that the registries hold for documentation,
    /// so no machine of the internet carries this address. Every test that
    /// reads it names a target of IP version 4, so the two families disagree.
    const PUBLIC_ADDRESS_VERSION_6: &str = "2001:db8::7";

    /// The target of every test that reads the lookup on its own.
    ///
    /// The lookup opens no socket, so this address only names a family.
    /// 198.51.100.0/24 is TEST-NET-2, which the registries hold for
    /// documentation, so the number tells a reader that it names no machine of
    /// the internet, and it stands apart from the answers of TEST-NET-3.
    const TARGET: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1));

    /// The name that the public address and the plain destination derive.
    const PUBLIC_NAME: &str = "203.0.113.7-example.com.jsonl";

    /// The method of the one request that the lookup makes.
    const GET: &str = "GET";

    /// The path that the mock service answers on.
    ///
    /// `Server::url` gives a URL that carries no path, and a URL that carries
    /// no path asks for the root.
    const ROOT: &str = "/";

    /// The status of an answer that carries an address.
    const OK: usize = 200;

    /// The status of a service that has broken.
    const SERVER_ERROR: usize = 500;

    /// How many requests one lookup makes. One lookup asks once.
    const ONE_REQUEST: usize = 1;

    /// How many requests a search that names its source makes.
    ///
    /// Such a search reads no service, so it makes none.
    const NO_REQUEST: usize = 0;

    /// The timeout of a test that reads a service that answers at once.
    ///
    /// The service runs on the loopback of this machine, so it answers in a
    /// millisecond or two. Five seconds is large enough that a machine under
    /// load does not fail the test for a reason that this code does not own.
    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    /// The timeout of the test that reads a service that answers too late.
    const SHORT_TIMEOUT: Duration = Duration::from_millis(50);

    /// How long the slow service waits before it writes the first byte of a
    /// body.
    ///
    /// The value is ten times `SHORT_TIMEOUT`, and the two numbers stand far
    /// apart on purpose. The service starts to wait when the request arrives,
    /// and the client starts its clock before it sends that request, so the
    /// answer cannot reach the client earlier than this. The order of the two
    /// is therefore fixed and not a race, and the gap makes that plain to a
    /// reader and leaves room on a machine under load.
    const SLOW_ANSWER: Duration = Duration::from_millis(500);

    /// An answer that is not an address.
    const NOT_AN_ADDRESS: &str = "the service moved";

    /// The character that the long answer repeats.
    ///
    /// It holds three bytes and one character, so a cut that reads bytes lands
    /// inside it and panics.
    const LONG_ANSWER_CHARACTER: char = '日';

    /// How many characters the long answer holds.
    ///
    /// The count is more than `ANSWER_LIMIT`, so the message holds a part of
    /// the answer and not all of it.
    const LONG_ANSWER_LENGTH: usize = ANSWER_LIMIT * 4;

    /// An answer shaped like the start of a page of HTML.
    ///
    /// A service that has broken answers such a page. The page holds three
    /// line breaks inside its first sixty characters, so a message that counts
    /// characters and keeps the line breaks writes a warning of four lines,
    /// and `PUBLIC_FALLBACK` then stands on a line of its own, away from the
    /// reason that it belongs to.
    const HTML_ANSWER: &str = "<!DOCTYPE html>\n<html>\n<head>\n<title>Gateway</title>";

    /// The line break that the answer of HTML holds.
    const LINE_BREAK: char = '\n';

    /// The first two lines of the answer of HTML, joined by one space.
    ///
    /// The collapse of the whitespace joins the lines of the answer and drops
    /// none of them, so the message holds the words of more than one line.
    const HTML_JOINED: &str = "<!DOCTYPE html> <html>";

    /// The escape character, which starts every sequence that paints a
    /// terminal.
    const ESCAPE: char = '\u{1b}';

    /// An answer that holds a sequence which clears the screen of a reader.
    ///
    /// The warning line goes to standard error, so a sequence that arrives in
    /// the answer paints the terminal of the user. `ESC [ 2 J` clears the
    /// screen.
    const ANSWER_WITH_A_SEQUENCE: &str = "\u{1b}[2Jgone";

    /// What the answer with a sequence leaves in the message.
    ///
    /// The clean drops the escape character alone. The parameter letters of
    /// the sequence stay as ordinary text, so the terminal keeps its screen
    /// and the reader still sees what arrived.
    const SEQUENCE_AS_TEXT: &str = "[2Jgone";

    /// An answer that holds a run of several whitespace characters.
    ///
    /// The run holds a line break, a tab, and three spaces, and it stands
    /// between the two halves of `NOT_AN_ADDRESS`, so the cleaned answer is
    /// that text.
    const ANSWER_WITH_A_RUN: &str = "the service\n\t   moved";

    /// Reads a mock service that answers one GET of its root with a status and
    /// a body, for a target that the caller names.
    ///
    /// The target names the family that the answer must be of. A caller that
    /// names a target of one family and a body of the other reads the check of
    /// the family.
    ///
    /// `mockito::Server` binds the loopback and asks the operating system for a
    /// port, so two copies of one test that run at the same time take two ports
    /// and never collide. The service is a local one, so no test of the lookup
    /// reaches a public address service of the internet.
    ///
    /// The guard of the server stays alive until the lookup has its answer, and
    /// the server stops when the guard drops. `Mock::assert` then reads the
    /// count of requests, which proves that the lookup asked the mock service
    /// and asked it once.
    fn answer_of(status: usize, body: &str, target: IpAddr) -> Result<IpAddr, PublicError> {
        let mut server = mockito::Server::new();
        let mock = server
            .mock(GET, ROOT)
            .with_status(status)
            .with_body(body)
            .expect(ONE_REQUEST)
            .create();
        let found = public_address(&server.url(), target, TEST_TIMEOUT);
        mock.assert();
        found
    }

    /// The address that a service which answers at once gives.
    #[test]
    fn a_service_that_answers_with_an_address_gives_that_address() {
        let found =
            answer_of(OK, PUBLIC_ADDRESS, TARGET).expect("the mock service answers an address");
        assert_eq!(found, address(PUBLIC_ADDRESS));
    }

    /// A service that ends its answer with a newline is a common one, and a
    /// lookup that keeps that newline parses no address at all.
    #[test]
    fn a_service_that_answers_with_an_address_and_whitespace_gives_that_address() {
        let body = format!("  {PUBLIC_ADDRESS}\r\n");
        let found =
            answer_of(OK, &body, TARGET).expect("the answer loses the whitespace of both ends");
        assert_eq!(found, address(PUBLIC_ADDRESS));
    }

    /// A service that answers `500` answers with a page of its own, and that
    /// page is not an address. A lookup that reads the body of every status
    /// takes that page for an answer.
    #[test]
    fn a_service_that_answers_with_the_status_of_an_error_gives_no_address() {
        let error = answer_of(SERVER_ERROR, PUBLIC_ADDRESS, TARGET)
            .expect_err("the status of an error gives no address");
        assert!(
            matches!(error, PublicError::Request { .. }),
            "the status stops the request before the body parses: {error}"
        );
    }

    /// The timeout holds the run to a known cost.
    ///
    /// The service writes the first byte of the body after `SLOW_ANSWER`, which
    /// is ten times the timeout that the lookup takes, so the lookup gives up
    /// first. Without the timeout the run waits as long as the service does.
    ///
    /// This test names no `Mock::assert`. The client drops the connection when
    /// it gives up, and the count of requests is of no interest here: the URL
    /// is the URL of the mock service, so the lookup reached no other one.
    #[test]
    fn a_service_that_answers_after_the_timeout_gives_no_address() {
        let mut server = mockito::Server::new();
        let _mock = server
            .mock(GET, ROOT)
            .with_status(OK)
            .with_chunked_body(|writer| {
                std::thread::sleep(SLOW_ANSWER);
                writer.write_all(PUBLIC_ADDRESS.as_bytes())
            })
            .create();
        let error = public_address(&server.url(), TARGET, SHORT_TIMEOUT)
            .expect_err("the lookup gives up before the service answers");
        assert!(
            matches!(error, PublicError::Request { .. }),
            "a request that runs out of time did not complete: {error}"
        );
    }

    /// A service that answers `200` with a page of its own, and not with an
    /// address, gives no address. The message names the text, so a reader of
    /// the warning line sees what arrived.
    #[test]
    fn a_service_that_answers_with_text_that_is_not_an_address_gives_no_address() {
        let error = answer_of(OK, NOT_AN_ADDRESS, TARGET)
            .expect_err("the text of the answer is not an address");
        assert!(
            matches!(error, PublicError::Answer { .. }),
            "the request completed and the answer did not parse: {error}"
        );
        assert!(
            error.to_string().contains(NOT_AN_ADDRESS),
            "the message names the text that arrived: {error}"
        );
    }

    /// A service that answers with a whole page writes that whole page to the
    /// warning line of the run, unless the message cuts it.
    ///
    /// The answer repeats a character of three bytes, so a cut that reads bytes
    /// lands inside a character and panics. The cut reads characters, so the
    /// message ends with the ellipsis and the test does not panic.
    #[test]
    fn an_answer_that_is_not_an_address_and_holds_many_characters_gives_a_short_message() {
        let body = LONG_ANSWER_CHARACTER.to_string().repeat(LONG_ANSWER_LENGTH);
        let error = answer_of(OK, &body, TARGET).expect_err("a page of text is not an address");
        let message = error.to_string();
        assert!(
            message.ends_with(ELLIPSIS),
            "the message cut the answer and said so: {message}"
        );
        assert!(
            message.chars().count() < body.chars().count(),
            "the message is shorter than the answer: {message}"
        );
    }

    /// A service that answers with a page of HTML gives a message of one line.
    ///
    /// The message becomes the warning line that the run writes to standard
    /// error. A message that keeps the line breaks of the page breaks that
    /// warning across lines, and `PUBLIC_FALLBACK` then stands on a line of
    /// its own, away from the reason that it belongs to. A limit that counts
    /// characters bounds no line, because the first sixty characters of such a
    /// page hold three line breaks.
    ///
    /// The collapse of the whitespace joins the lines and drops none of them,
    /// so the message still holds the words of more than one line of the page.
    #[test]
    fn an_answer_that_holds_line_breaks_gives_a_message_of_one_line() {
        let error =
            answer_of(OK, HTML_ANSWER, TARGET).expect_err("a page of HTML is not an address");
        let message = error.to_string();
        assert!(
            !message.contains(LINE_BREAK),
            "the message holds no line break: {message:?}"
        );
        assert!(
            message.contains(HTML_JOINED),
            "the message holds the words of more than one line: {message:?}"
        );
    }

    /// A service that answers with a sequence which paints a terminal gives a
    /// message that holds no escape character.
    ///
    /// The warning line goes straight to standard error, and the answer is the
    /// text of a remote service. A message that keeps the escape character
    /// lets that service clear the screen of the user, move the cursor, and
    /// hide what the run wrote.
    #[test]
    fn an_answer_that_holds_the_escape_character_gives_a_message_without_it() {
        let error = answer_of(OK, ANSWER_WITH_A_SEQUENCE, TARGET)
            .expect_err("a sequence that paints a terminal is not an address");
        let message = error.to_string();
        assert!(
            !message.contains(ESCAPE),
            "the message holds no escape character: {message:?}"
        );
        assert!(
            message.ends_with(SEQUENCE_AS_TEXT),
            "the message keeps the letters of the sequence as text: {message:?}"
        );
    }

    /// A service that answers with a run of whitespace gives a message that
    /// holds one space in its place.
    ///
    /// `ANSWER_LIMIT` is the whole budget of the warning line. A message that
    /// keeps a run of whitespace spends that budget on padding, and not on the
    /// words that name the trouble.
    #[test]
    fn an_answer_that_holds_a_run_of_whitespace_gives_a_message_with_one_space() {
        let error = answer_of(OK, ANSWER_WITH_A_RUN, TARGET)
            .expect_err("the text of the answer is not an address");
        let message = error.to_string();
        assert!(
            message.ends_with(NOT_AN_ADDRESS),
            "the run of whitespace became one space: {message:?}"
        );
    }

    /// A service that answers with an address of the other family gives no
    /// address.
    ///
    /// The pick of the host by the family of the target is a best effort, and
    /// this check is the guarantee. A record of one family that carries a
    /// source of the other reads as a fault of the tool, and it derives the
    /// file name that the run of the other family derives, so two traces of two
    /// families append to one file.
    ///
    /// The message names both addresses, because a reader of the warning line
    /// needs to see which two families disagreed.
    #[test]
    fn a_service_that_answers_with_an_address_of_the_other_family_gives_no_address() {
        let error = answer_of(OK, PUBLIC_ADDRESS_VERSION_6, TARGET)
            .expect_err("an address of the other family is not the source of this run");
        assert!(
            matches!(error, PublicError::Family { .. }),
            "the answer parses and its family disagrees with the target: {error}"
        );
        let message = error.to_string();
        assert!(
            message.contains(PUBLIC_ADDRESS_VERSION_6),
            "the message names the address that arrived: {message}"
        );
        assert!(
            message.contains(&TARGET.to_string()),
            "the message names the target of the run: {message}"
        );
    }

    /// Runs the search for the source against a mock service that answers one
    /// GET of its root with a status and a body.
    ///
    /// `requests` is how many requests the search must make. `Mock::assert`
    /// reads that count when the search returns, so a search that asks the
    /// service twice, or that asks it at all when the count is zero, fails the
    /// test. The count is what proves the order of the three steps, because a
    /// search that asks the service and then throws the answer away gives the
    /// same label as one that never asks.
    ///
    /// The target is the loopback, so the step that reads the local egress
    /// address opens a socket of the loopback and touches no network. The mock
    /// service binds the loopback too, and it asks the operating system for a
    /// port, so two copies of one test that run at the same time take two ports
    /// and never collide. No test of the search reaches a public address
    /// service of the internet.
    fn search_of(
        named: Option<IpAddr>,
        status: usize,
        body: &str,
        requests: usize,
    ) -> std::io::Result<Discovery> {
        let mut server = mockito::Server::new();
        let mock = server
            .mock(GET, ROOT)
            .with_status(status)
            .with_body(body)
            .expect(requests)
            .create();
        let found = discover_at(named, LOOPBACK, &server.url(), TEST_TIMEOUT);
        mock.assert();
        found
    }

    /// A source that the user named wins over both lookups. The record marks it
    /// an override, the search asks no service, and it opens no socket.
    ///
    /// The mock service of this test answers an address, and the result holds
    /// no part of that answer. A search that asks the service first pays a
    /// request, and a timeout, for an address that it then throws away.
    #[test]
    fn a_source_that_the_user_named_is_an_override() {
        let named = address(SOURCE);
        let found = search_of(Some(named), OK, PUBLIC_ADDRESS, NO_REQUEST)
            .expect("a named source opens no socket");
        assert_eq!(found.label.addr, named);
        assert_eq!(found.label.kind, SourceKind::Override);
        assert_eq!(
            found.note, None,
            "a search that needed no fallback carries no warning"
        );
    }

    /// A service that answers an address makes that address the source, and the
    /// record marks it public.
    ///
    /// Without this step the record names an address of a local interface,
    /// which every machine behind one router shares with the others, so two
    /// machines of one house record to one file.
    #[test]
    fn a_service_that_answers_an_address_gives_the_public_source() {
        let found = search_of(None, OK, PUBLIC_ADDRESS, ONE_REQUEST)
            .expect("the mock service answers an address");
        assert_eq!(found.label.addr, address(PUBLIC_ADDRESS));
        assert_eq!(found.label.kind, SourceKind::Public);
        assert_eq!(
            found.note, None,
            "a search that needed no fallback carries no warning"
        );
    }

    /// A run that names no source, and that reads no public address, records
    /// the local egress address, and the record marks it local.
    ///
    /// The service of this test answers the status of an error, and the target
    /// is the loopback, so the test touches no network. Without this step a
    /// machine on a captive network, or on a network with no route out, records
    /// nothing at all.
    #[test]
    fn no_named_source_gives_the_local_egress_address() {
        let found = search_of(None, SERVER_ERROR, PUBLIC_ADDRESS, ONE_REQUEST)
            .expect("every machine holds a loopback route");
        assert_eq!(found.label.addr, LOOPBACK);
        assert_eq!(found.label.kind, SourceKind::Local);
    }

    /// A search that fell back to the local egress address names the reason and
    /// names what it did.
    ///
    /// The service of this test answers text that is not an address, so the
    /// note carries that text. A note that names only the reason leaves a
    /// reader to guess which address the file then holds.
    #[test]
    fn a_service_that_answers_no_address_falls_back_and_says_why() {
        let found = search_of(None, OK, NOT_AN_ADDRESS, ONE_REQUEST)
            .expect("every machine holds a loopback route");
        assert_eq!(found.label.addr, LOOPBACK);
        assert_eq!(found.label.kind, SourceKind::Local);
        let note = found.note.expect("a search that fell back carries a note");
        assert!(
            note.contains(NOT_AN_ADDRESS),
            "the note names the reason: {note}"
        );
        assert!(
            note.contains(PUBLIC_FALLBACK),
            "the note names what the run recorded in its place: {note}"
        );
    }

    /// A run that reads an address of the other family falls back to the local
    /// egress address, and it says why.
    ///
    /// The target of this test is the loopback of IP version 4, and the mock
    /// service answers an address of IP version 6, so the two families
    /// disagree. The mock service binds the loopback of IP version 4, and the
    /// check reads the address that arrived and not the address of the
    /// transport, so the test needs no route of IP version 6.
    ///
    /// Without the fallback the run writes an address of one family into a
    /// record that names the other, and it derives the file name that the run
    /// of that other family derives.
    #[test]
    fn a_service_that_answers_an_address_of_the_other_family_falls_back_and_says_why() {
        let found = search_of(None, OK, PUBLIC_ADDRESS_VERSION_6, ONE_REQUEST)
            .expect("every machine holds a loopback route");
        assert_eq!(found.label.addr, LOOPBACK);
        assert_eq!(found.label.kind, SourceKind::Local);
        let note = found.note.expect("a search that fell back carries a note");
        assert!(
            note.contains(PUBLIC_ADDRESS_VERSION_6),
            "the note names the address that arrived: {note}"
        );
        assert!(
            note.contains(&LOOPBACK.to_string()),
            "the note names the target of the run: {note}"
        );
        assert!(
            note.contains(PUBLIC_FALLBACK),
            "the note names what the run recorded in its place: {note}"
        );
    }

    /// One run asks the public service once, and one run therefore writes one
    /// warning.
    ///
    /// The service of this test fails, because the warning belongs to the run
    /// that falls back. A search that asks once a round pays a request, and a
    /// timeout, for every round of a run that lasts for hours, and it writes
    /// that warning once a round. The count of requests that `search_of` reads
    /// is the proof.
    #[test]
    fn one_search_asks_the_public_service_once() {
        let found = search_of(None, SERVER_ERROR, PUBLIC_ADDRESS, ONE_REQUEST)
            .expect("every machine holds a loopback route");
        assert!(
            found.note.is_some(),
            "the search fell back, so it carries the one warning of the run"
        );
    }

    /// The derived name carries the label that won.
    ///
    /// A run that reads a public address keeps one file for that address across
    /// many runs, and the name loses the characters that a file name must not
    /// hold, as every derived name does.
    #[test]
    fn the_derived_name_carries_the_public_source_that_won() {
        let found = search_of(None, OK, PUBLIC_ADDRESS, ONE_REQUEST)
            .expect("the mock service answers an address");
        assert_eq!(
            output_path(None, found.label.addr, DESTINATION),
            PathBuf::from(PUBLIC_NAME)
        );
    }
}
