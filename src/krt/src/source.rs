//! Where the probes leave from, and what the recorded file is called.
//!
//! The name of a recorded file holds the source address and the destination, so
//! one source and one destination keep one file across many runs. Both halves
//! of the name lose every character that a file name must not hold on macOS, on
//! Linux, or on Windows. The `--output` flag names a file of its own, and it
//! wins over the derived name.
//!
//! The derived name carries the address that the probes leave from. A file that
//! the user gives to another person carries that address too. `--output`
//! avoids this.

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

/// The service that answers with the address that the internet sees.
///
/// The service answers a plain GET with the address as text and nothing else,
/// so the answer needs no parser of a format and the request needs no key of an
/// account. A service that goes away, or that starts to limit the rate of a
/// caller, costs this one line to change.
#[allow(
    dead_code,
    reason = "discover() reads this in the next slice of issue #368"
)]
const PUBLIC_SERVICE: &str = "https://api.ipify.org";

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
#[allow(
    dead_code,
    reason = "discover() reads this in the next slice of issue #368"
)]
const PUBLIC_TIMEOUT: Duration = Duration::from_secs(3);

/// How many characters of an answer that is not an address the message holds.
///
/// A service that answers with a page of HTML, and not with an address, writes
/// that whole page to the warning line of the run without this limit. Sixty
/// characters name the service and the trouble, and they fit one line.
const ANSWER_LIMIT: usize = 60;

/// The character that marks an answer that the message cut.
const ELLIPSIS: &str = "…";

/// The start of an answer, short enough for one line of a warning.
///
/// The walk reads characters and never bytes, so a character of more than one
/// byte survives whole. An answer that the walk cut ends with an ellipsis, so a
/// reader sees that more of it exists.
fn shorten(answer: &str) -> String {
    let mut characters = answer.chars();
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
        /// The start of the text that the service answered.
        answer: String,
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
/// # Errors
///
/// Returns [`PublicError::Request`] when the client does not build, when the
/// request does not complete inside the timeout, when the service answers with
/// the status of an error, and when the answer does not read as text. Returns
/// [`PublicError::Answer`] when the answer is not an address.
#[allow(
    dead_code,
    reason = "discover() calls this in the next slice of issue #368; the tests of this module cover it now"
)]
fn public_address(service: &str, timeout: Duration) -> Result<IpAddr, PublicError> {
    let _ = (service, timeout);
    Err(PublicError::Answer {
        answer: String::new(),
    })
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
    use super::{
        ANSWER_LIMIT, ELLIPSIS, PublicError, bind_address, derive_name, discover, egress_address,
        output_path, public_address,
    };
    use crate::record::SourceKind;
    use std::io::Write;
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

    /// The address that a mock service answers with.
    ///
    /// 203.0.113.0/24 is TEST-NET-3, which the registries hold for
    /// documentation, so no machine of the internet carries this address.
    const PUBLIC_ADDRESS: &str = "203.0.113.7";

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

    /// Reads a mock service that answers one GET of its root with a status and
    /// a body.
    ///
    /// `mockito::Server` binds the loopback and asks the operating system for a
    /// port, so two copies of one test that run at the same time take two ports
    /// and never collide. The service is a local one, so no test of the lookup
    /// reaches the service that `PUBLIC_SERVICE` names.
    ///
    /// The guard of the server stays alive until the lookup has its answer, and
    /// the server stops when the guard drops. `Mock::assert` then reads the
    /// count of requests, which proves that the lookup asked the mock service
    /// and asked it once.
    fn answer_of(status: usize, body: &str) -> Result<IpAddr, PublicError> {
        let mut server = mockito::Server::new();
        let mock = server
            .mock(GET, ROOT)
            .with_status(status)
            .with_body(body)
            .expect(ONE_REQUEST)
            .create();
        let found = public_address(&server.url(), TEST_TIMEOUT);
        mock.assert();
        found
    }

    /// The address that a service which answers at once gives.
    #[test]
    fn a_service_that_answers_with_an_address_gives_that_address() {
        let found = answer_of(OK, PUBLIC_ADDRESS).expect("the mock service answers an address");
        assert_eq!(found, address(PUBLIC_ADDRESS));
    }

    /// A service that ends its answer with a newline is a common one, and a
    /// lookup that keeps that newline parses no address at all.
    #[test]
    fn a_service_that_answers_with_an_address_and_whitespace_gives_that_address() {
        let body = format!("  {PUBLIC_ADDRESS}\r\n");
        let found = answer_of(OK, &body).expect("the answer loses the whitespace of both ends");
        assert_eq!(found, address(PUBLIC_ADDRESS));
    }

    /// A service that answers `500` answers with a page of its own, and that
    /// page is not an address. A lookup that reads the body of every status
    /// takes that page for an answer.
    #[test]
    fn a_service_that_answers_with_the_status_of_an_error_gives_no_address() {
        let error = answer_of(SERVER_ERROR, PUBLIC_ADDRESS)
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
        let error = public_address(&server.url(), SHORT_TIMEOUT)
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
        let error =
            answer_of(OK, NOT_AN_ADDRESS).expect_err("the text of the answer is not an address");
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
        let body = LONG_ANSWER_CHARACTER
            .to_string()
            .repeat(LONG_ANSWER_LENGTH);
        let error = answer_of(OK, &body).expect_err("a page of text is not an address");
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
}
