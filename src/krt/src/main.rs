//! `krt` (Knights of the Round Trip) records the network path to a
//! destination, hop by hop.
//!
//! This slice builds the crate and the build string. Later slices add the
//! command line flags, the tracer, the file writer, and the table.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use buildinfo::version_string;
use clap::{Parser, ValueEnum};
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

/// The accepted units of a duration.
const DURATION_UNITS: &str = "the unit must be `ms`, `s`, `m`, or `h`";

/// Examples of a duration, for the end of an error message.
const DURATION_FORMS: &str = "as in `500ms`, `1s`, or `2m`";

/// The number of seconds in one minute.
const SECONDS_PER_MINUTE: u64 = 60;

/// The number of seconds in one hour.
const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;

/// The protocol of a probe.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Protocol {
    /// Send ICMP echo requests.
    Icmp,
    /// Send UDP datagrams.
    Udp,
    /// Send TCP packets.
    Tcp,
}

/// The way a probe keeps or varies the flow of a packet.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Multipath {
    /// Let each probe take its own flow, as traceroute always did.
    Classic,
    /// Hold one flow for every probe, as Paris traceroute does.
    Paris,
    /// Walk the flows to find every path, as Dublin traceroute does.
    Dublin,
}

/// The command line of `krt`.
///
/// `--version` and `-V` print the build string that `buildinfo` made at compile
/// time.
#[derive(Parser, Debug)]
#[command(
    name = "krt",
    version = version_string!(),
    about = "Knights of the Round Trip: record the network path to a destination"
)]
struct Cli {
    /// The host or the address to trace.
    destination: Option<String>,

    /// The JSONL path. Overrides the derived name.
    #[arg(long)]
    output: Option<PathBuf>,

    /// The round period. Accepts `500ms`, `1s`, `2m`.
    #[arg(long, value_parser = parse_duration)]
    interval: Duration,

    /// The first TTL to probe.
    #[arg(long)]
    first_ttl: u8,

    /// The last TTL to probe.
    #[arg(long)]
    max_ttl: u8,

    /// The protocol of a probe.
    #[arg(long)]
    protocol: Protocol,

    /// The multipath mode. UDP and TCP only.
    #[arg(long)]
    multipath: Multipath,

    /// Force IP version 4.
    #[arg(long)]
    ipv4: bool,

    /// Force IP version 6.
    #[arg(long)]
    ipv6: bool,

    /// Skip reverse DNS. Show addresses only.
    #[arg(long)]
    no_dns: bool,

    /// Override the source label in the derived filename.
    #[arg(long)]
    source: Option<IpAddr>,

    /// No table. Print one status line per minute.
    #[arg(long)]
    headless: bool,

    /// Stop after this much time.
    #[arg(long, value_parser = parse_duration)]
    duration: Option<Duration>,

    /// Stop after this many rounds.
    #[arg(long)]
    rounds: Option<u64>,

    /// Fold a recorded file and print the table. Then exit.
    #[arg(long)]
    replay: Option<PathBuf>,

    /// With `--replay`, pick which run in the file to fold.
    #[arg(long)]
    run: Option<String>,
}

/// Reads a duration from the text of a command line flag.
///
/// The text holds a whole number and one unit, with no space between them. The
/// units are `ms` for milliseconds, `s` for seconds, `m` for minutes, and `h`
/// for hours. `500ms`, `1s`, `2m`, and `3h` are examples.
///
/// # Errors
///
/// Returns the reason as text when the number is absent, the unit is absent or
/// unknown, the text carries a sign, the duration is zero, or the number is too
/// large. Each message names the fault and the accepted forms, so the user
/// reads one line and corrects the flag.
#[allow(
    dead_code,
    reason = "slice 3 attaches this parser to `--interval` and `--duration`"
)]
fn parse_duration(text: &str) -> Result<Duration, String> {
    if text.is_empty() {
        return Err(format!(
            "a duration is empty: write a whole number and a unit, {DURATION_FORMS}"
        ));
    }
    if text.starts_with('-') {
        return Err(format!(
            "`{text}` is negative: a duration is never negative, {DURATION_FORMS}"
        ));
    }

    // The number is the run of digits at the front. The unit is the rest. The
    // split point comes from `char_indices`, so it is on a character boundary.
    let unit_start = text
        .char_indices()
        .find(|(_, character)| !character.is_ascii_digit())
        .map_or(text.len(), |(index, _)| index);
    let (number, unit) = text.split_at(unit_start);

    if number.is_empty() {
        return Err(format!(
            "`{text}` has no number: write a whole number before the unit, {DURATION_FORMS}"
        ));
    }
    if unit.is_empty() {
        return Err(format!(
            "`{text}` has no unit: {DURATION_UNITS}, {DURATION_FORMS}"
        ));
    }

    let too_large = || format!("`{text}` is too large: use a smaller number, {DURATION_FORMS}");
    let count: u64 = number.parse().map_err(|_| too_large())?;
    let seconds = |per_unit: u64| {
        count
            .checked_mul(per_unit)
            .map(Duration::from_secs)
            .ok_or_else(too_large)
    };
    let duration = match unit {
        "ms" => Duration::from_millis(count),
        "s" => Duration::from_secs(count),
        "m" => seconds(SECONDS_PER_MINUTE)?,
        "h" => seconds(SECONDS_PER_HOUR)?,
        _ => {
            return Err(format!(
                "`{text}` is not a duration: {DURATION_UNITS}, {DURATION_FORMS}"
            ));
        }
    };

    if duration.is_zero() {
        return Err(format!(
            "`{text}` is zero: a duration is more than zero, {DURATION_FORMS}"
        ));
    }
    Ok(duration)
}

/// Writes the shortest exact text of a duration.
///
/// A duration that carries milliseconds becomes milliseconds. A whole number of
/// hours becomes hours. A whole number of minutes becomes minutes. Every other
/// duration becomes seconds. The text reads like the text a user types, so
/// `Duration::from_secs(3600)` becomes `1h`.
#[allow(
    dead_code,
    reason = "slice 4 prints the resolved configuration with this renderer"
)]
fn render_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if duration.subsec_millis() != 0 || seconds == 0 {
        return format!("{}ms", duration.as_millis());
    }
    if seconds.is_multiple_of(SECONDS_PER_HOUR) {
        return format!("{}h", seconds / SECONDS_PER_HOUR);
    }
    if seconds.is_multiple_of(SECONDS_PER_MINUTE) {
        return format!("{}m", seconds / SECONDS_PER_MINUTE);
    }
    format!("{seconds}s")
}

fn main() {
    // The parse handles `--version`, `-V`, and `--help` on its own. A later
    // slice prints the resolved configuration.
    Cli::parse();
}

#[cfg(test)]
mod tests {
    use super::{Cli, Multipath, Protocol, parse_duration, render_duration};
    use clap::error::ErrorKind;
    use clap::{CommandFactory, Parser};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::path::PathBuf;
    use std::time::Duration;

    /// The accepted units, as `parse_duration` names them for an unknown unit.
    const UNITS_IN_THE_MESSAGE: &str = "the unit must be `ms`, `s`, `m`, or `h`";

    /// The accepted forms, as `parse_duration` names them in every message.
    const FORMS_IN_THE_MESSAGE: &str = "as in `500ms`, `1s`, or `2m`";

    /// Reads a command line that the definition accepts.
    fn parse(arguments: &[&str]) -> Cli {
        Cli::try_parse_from(arguments.iter().copied()).expect("the command line must parse")
    }

    /// Reads the error of a command line that the definition rejects.
    fn rejection(arguments: &[&str]) -> clap::Error {
        Cli::try_parse_from(arguments.iter().copied()).expect_err("the command line must fail")
    }

    /// Every text that the parser rejects, for the message tests.
    const BAD_TEXTS: [&str; 10] = [
        "",
        "1",
        "1sec",
        "5x",
        "ms",
        "abc",
        "-1s",
        "0s",
        "0ms",
        "99999999999999999999m",
    ];

    fn error_of(text: &str) -> String {
        parse_duration(text).expect_err("the parser must reject this text")
    }

    #[test]
    fn parses_milliseconds() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
    }

    #[test]
    fn parses_many_milliseconds() {
        assert_eq!(
            parse_duration("1500ms").unwrap(),
            Duration::from_millis(1500)
        );
    }

    #[test]
    fn parses_seconds() {
        assert_eq!(parse_duration("1s").unwrap(), Duration::from_secs(1));
    }

    #[test]
    fn parses_many_seconds() {
        assert_eq!(parse_duration("90s").unwrap(), Duration::from_secs(90));
    }

    #[test]
    fn parses_minutes() {
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_mins(2));
    }

    #[test]
    fn parses_hours() {
        assert_eq!(parse_duration("3h").unwrap(), Duration::from_hours(3));
    }

    #[test]
    fn rejects_empty_text() {
        assert!(error_of("").contains("empty"));
    }

    #[test]
    fn rejects_text_without_a_unit() {
        let message = error_of("1");
        assert!(
            message.contains("`1`"),
            "the message names the text: {message}"
        );
        assert!(
            message.contains("no unit"),
            "the message names the fault: {message}"
        );
        assert!(
            message.contains("the unit must be `ms`, `s`, `m`, or `h`"),
            "the message names the accepted units: {message}"
        );
    }

    #[test]
    fn rejects_an_unknown_unit() {
        for text in ["1sec", "5x"] {
            let message = error_of(text);
            assert!(
                message.contains(text),
                "the message names the text: {message}"
            );
            assert!(
                message.contains("the unit must be `ms`, `s`, `m`, or `h`"),
                "the message names the accepted units: {message}"
            );
        }
    }

    #[test]
    fn rejects_text_without_a_number() {
        for text in ["ms", "abc"] {
            let message = error_of(text);
            assert!(
                message.contains("no number"),
                "the message names the fault: {message}"
            );
        }
    }

    #[test]
    fn rejects_a_negative_duration() {
        let message = error_of("-1s");
        assert!(
            message.contains("never negative"),
            "the message names the fault: {message}"
        );
    }

    #[test]
    fn rejects_a_zero_duration() {
        for text in ["0s", "0ms"] {
            let message = error_of(text);
            assert!(
                message.contains("zero"),
                "the message names the fault: {message}"
            );
        }
    }

    #[test]
    fn rejects_a_number_that_overflows() {
        let message = error_of("99999999999999999999m");
        assert!(
            message.contains("too large"),
            "the message names the fault: {message}"
        );
    }

    #[test]
    fn every_error_names_the_accepted_forms() {
        for text in BAD_TEXTS {
            let message = error_of(text);
            assert!(
                message.contains("as in `500ms`, `1s`, or `2m`"),
                "the message names the accepted forms: {message}"
            );
        }
    }

    #[test]
    fn renders_milliseconds() {
        assert_eq!(render_duration(Duration::from_millis(500)), "500ms");
    }

    #[test]
    fn renders_one_second() {
        assert_eq!(render_duration(Duration::from_secs(1)), "1s");
    }

    #[test]
    fn renders_many_seconds() {
        assert_eq!(render_duration(Duration::from_secs(90)), "90s");
    }

    #[test]
    fn renders_minutes() {
        assert_eq!(render_duration(Duration::from_mins(2)), "2m");
    }

    #[test]
    fn renders_one_hour() {
        assert_eq!(render_duration(Duration::from_hours(1)), "1h");
    }

    #[test]
    #[allow(
        clippy::duration_suboptimal_units,
        reason = "the seconds are the behavior: a duration of 3600 seconds renders as hours"
    )]
    fn renders_seconds_that_make_a_whole_hour_as_hours() {
        assert_eq!(render_duration(Duration::from_secs(3600)), "1h");
    }

    #[test]
    fn renders_whole_hours_as_hours() {
        assert_eq!(render_duration(Duration::from_hours(2)), "2h");
    }

    #[test]
    fn renders_a_duration_that_carries_milliseconds_as_milliseconds() {
        assert_eq!(render_duration(Duration::from_millis(1500)), "1500ms");
    }

    #[test]
    fn a_parsed_duration_renders_as_the_text_of_the_parse() {
        for text in ["500ms", "1s", "90s", "2m", "1h"] {
            assert_eq!(render_duration(parse_duration(text).unwrap()), text);
        }
    }

    #[test]
    fn the_command_line_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_flag_has_its_documented_default() {
        let cli = parse(&["krt", "example.com"]);
        assert_eq!(cli.destination.as_deref(), Some("example.com"));
        assert_eq!(cli.output, None);
        assert_eq!(cli.interval, Duration::from_secs(1));
        assert_eq!(cli.first_ttl, 1);
        assert_eq!(cli.max_ttl, 30);
        assert_eq!(cli.protocol, Protocol::Icmp);
        assert_eq!(cli.multipath, Multipath::Classic);
        assert!(!cli.ipv4, "the address family is automatic by default");
        assert!(!cli.ipv6, "the address family is automatic by default");
        assert!(!cli.no_dns, "reverse DNS is on by default");
        assert_eq!(cli.source, None);
        assert!(!cli.headless, "the table is on by default");
        assert_eq!(cli.duration, None);
        assert_eq!(cli.rounds, None);
        assert_eq!(cli.replay, None);
        assert_eq!(cli.run, None);
    }

    #[test]
    fn parses_every_protocol() {
        for (text, expected) in [
            ("icmp", Protocol::Icmp),
            ("udp", Protocol::Udp),
            ("tcp", Protocol::Tcp),
        ] {
            let cli = parse(&["krt", "example.com", "--protocol", text]);
            assert_eq!(cli.protocol, expected, "`--protocol {text}`");
        }
    }

    #[test]
    fn rejects_an_unknown_protocol() {
        let error = rejection(&["krt", "example.com", "--protocol", "quic"]);
        assert_eq!(error.kind(), ErrorKind::InvalidValue);
        let message = error.to_string();
        assert!(
            message.contains("icmp"),
            "the message names the accepted values: {message}"
        );
    }

    #[test]
    fn parses_every_multipath_mode() {
        for (text, expected) in [
            ("classic", Multipath::Classic),
            ("paris", Multipath::Paris),
            ("dublin", Multipath::Dublin),
        ] {
            let cli = parse(&["krt", "example.com", "--multipath", text]);
            assert_eq!(cli.multipath, expected, "`--multipath {text}`");
        }
    }

    #[test]
    fn rejects_an_unknown_multipath_mode() {
        let error = rejection(&["krt", "example.com", "--multipath", "tokyo"]);
        assert_eq!(error.kind(), ErrorKind::InvalidValue);
        let message = error.to_string();
        assert!(
            message.contains("classic"),
            "the message names the accepted values: {message}"
        );
    }

    #[test]
    fn parses_the_flag_of_ip_version_4() {
        let cli = parse(&["krt", "example.com", "-4"]);
        assert!(cli.ipv4);
        assert!(!cli.ipv6);
    }

    #[test]
    fn parses_the_flag_of_ip_version_6() {
        let cli = parse(&["krt", "example.com", "-6"]);
        assert!(cli.ipv6);
        assert!(!cli.ipv4);
    }

    #[test]
    fn rejects_both_address_family_flags() {
        let error = rejection(&["krt", "example.com", "-4", "-6"]);
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
        let message = error.to_string();
        assert!(
            message.contains("-4"),
            "the message names the first flag: {message}"
        );
        assert!(
            message.contains("-6"),
            "the message names the second flag: {message}"
        );
    }

    #[test]
    fn parses_an_interval_in_milliseconds() {
        let cli = parse(&["krt", "example.com", "--interval", "500ms"]);
        assert_eq!(cli.interval, Duration::from_millis(500));
    }

    #[test]
    fn parses_an_interval_in_minutes() {
        let cli = parse(&["krt", "example.com", "--interval", "2m"]);
        assert_eq!(cli.interval, Duration::from_mins(2));
    }

    #[test]
    fn rejects_an_interval_that_is_not_a_duration() {
        for text in ["bogus", "5x"] {
            let error = rejection(&["krt", "example.com", "--interval", text]);
            assert_eq!(error.kind(), ErrorKind::ValueValidation, "`{text}`");
            let message = error.to_string();
            assert!(
                message.contains(FORMS_IN_THE_MESSAGE),
                "the message names the accepted forms: {message}"
            );
        }
        let message = rejection(&["krt", "example.com", "--interval", "5x"]).to_string();
        assert!(
            message.contains(UNITS_IN_THE_MESSAGE),
            "the message names the accepted units: {message}"
        );
    }

    #[test]
    fn parses_a_duration_that_stops_the_run() {
        let cli = parse(&["krt", "example.com", "--duration", "2m"]);
        assert_eq!(cli.duration, Some(Duration::from_mins(2)));
    }

    #[test]
    fn rejects_a_command_line_without_a_destination() {
        let error = rejection(&["krt"]);
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn a_replay_needs_no_destination() {
        let cli = parse(&["krt", "--replay", "path.jsonl"]);
        assert_eq!(cli.destination, None);
        assert_eq!(cli.replay, Some(PathBuf::from("path.jsonl")));
    }

    #[test]
    fn rejects_a_run_without_a_replay() {
        let error = rejection(&["krt", "example.com", "--run", "2026-08-19T12:00:00Z"]);
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn parses_a_run_of_a_replay() {
        let cli = parse(&[
            "krt",
            "--replay",
            "path.jsonl",
            "--run",
            "2026-08-19T12:00:00Z",
        ]);
        assert_eq!(cli.run.as_deref(), Some("2026-08-19T12:00:00Z"));
    }

    #[test]
    fn rejects_a_first_ttl_of_zero() {
        let error = rejection(&["krt", "example.com", "--first-ttl", "0"]);
        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }

    #[test]
    fn parses_the_largest_max_ttl() {
        let cli = parse(&["krt", "example.com", "--max-ttl", "255"]);
        assert_eq!(cli.max_ttl, 255);
    }

    #[test]
    fn the_short_forms_reach_the_same_fields_as_the_long_forms() {
        let short = parse(&[
            "krt",
            "example.com",
            "-o",
            "path.jsonl",
            "-i",
            "500ms",
            "-4",
        ]);
        let long = parse(&[
            "krt",
            "example.com",
            "--output",
            "path.jsonl",
            "--interval",
            "500ms",
        ]);
        assert_eq!(short.output, Some(PathBuf::from("path.jsonl")));
        assert_eq!(short.interval, Duration::from_millis(500));
        assert!(short.ipv4, "`-4` is the only form of the flag");
        assert_eq!(short.output, long.output);
        assert_eq!(short.interval, long.interval);
    }

    #[test]
    fn parses_a_source_of_ip_version_4() {
        let cli = parse(&["krt", "example.com", "--source", "1.2.3.4"]);
        assert_eq!(cli.source, Some(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))));
    }

    #[test]
    fn parses_a_source_of_ip_version_6() {
        let cli = parse(&["krt", "example.com", "--source", "::1"]);
        assert_eq!(cli.source, Some(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn rejects_a_source_that_is_not_an_address() {
        let error = rejection(&["krt", "example.com", "--source", "nope"]);
        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }

    #[test]
    fn parses_the_rounds_that_stop_the_run() {
        let cli = parse(&["krt", "example.com", "--rounds", "10"]);
        assert_eq!(cli.rounds, Some(10));
    }

    #[test]
    fn parses_the_flags_that_hold_no_value() {
        let cli = parse(&["krt", "example.com", "--no-dns", "--headless"]);
        assert!(cli.no_dns);
        assert!(cli.headless);
    }
}
