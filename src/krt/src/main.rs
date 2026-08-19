//! `krt` (Knights of the Round Trip) records the network path to a
//! destination, hop by hop.
//!
//! This slice resolves the command line and prints the configuration of the
//! run. Later slices add the tracer, the file writer, and the table.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use buildinfo::version_string;
use clap::{CommandFactory, Parser, ValueEnum};
use std::fmt;
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

/// The lowest TTL that a probe carries. A TTL of zero leaves no hop to reach.
///
/// The type is the type that `clap` takes for the bound of a range.
const TTL_LOWEST: i64 = 1;

/// The highest TTL that a probe carries. The field of the packet holds one byte.
///
/// The type is the type that `clap` takes for the bound of a range.
const TTL_HIGHEST: i64 = 255;

/// The first TTL of a run, when the user names none.
const FIRST_TTL_DEFAULT: u8 = 1;

/// The last TTL of a run, when the user names none.
const MAX_TTL_DEFAULT: u8 = 30;

/// The lowest number of rounds that stops a run. A run of zero rounds records
/// nothing.
///
/// The type is the type that `clap` takes for the bound of a range.
const ROUNDS_LOWEST: u64 = 1;

/// The width of the key field of the resolved configuration block.
///
/// The longest key is `address family:`, and one space follows it.
const CONFIG_KEY_WIDTH: usize = 16;

/// The value of a flag that holds no limit and no file.
const ABSENT: &str = "none";

/// The value of the output, when the user names no file.
const OUTPUT_DERIVED: &str = "derived at run time";

/// The value of the source, when the user names no address.
const SOURCE_DISCOVERED: &str = "discovered at run time";

/// The value of the run, when the user names no run of a replay.
const RUN_LATEST: &str = "the last run";

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

/// The IP version of a probe, after the two flags of the command line resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AddressFamily {
    /// Let the resolver pick the version.
    Auto,
    /// Force IP version 4.
    Version4,
    /// Force IP version 6.
    Version6,
}

impl fmt::Display for AddressFamily {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::Version4 => "ipv4",
            Self::Version6 => "ipv6",
        })
    }
}

/// Reads the text that the parser accepts for one value of a value enum.
///
/// The printer and the parser then read the same table, so the text of the
/// output and the text of the command line can never part company.
///
/// # Panics
///
/// Panics when the variant carries no name. Every variant of a value enum of
/// `krt` carries one, because no variant is hidden from the parser.
fn value_name<T: ValueEnum>(value: &T) -> String {
    value
        .to_possible_value()
        .expect("every variant of a value enum of `krt` carries a name")
        .get_name()
        .to_owned()
}

/// Knights of the Round Trip: record the network path to a destination.
///
/// `krt` probes every hop to the destination once per round, and it records
/// each round in a file. This build parses the command line and prints the
/// configuration it resolved.
#[derive(Parser, Debug)]
#[command(name = "krt", version = version_string!())]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag of the design is one switch of the command line"
)]
struct Cli {
    /// The host or the address to trace. A replay takes no destination.
    #[arg(
        value_name = "DESTINATION",
        required_unless_present = "replay",
        conflicts_with_all = ["replay", "run"],
    )]
    destination: Option<String>,

    /// The JSONL path. Overrides the derived name.
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// The round period. Accepts `500ms`, `1s`, `2m`.
    #[arg(
        short,
        long,
        value_name = "DUR",
        default_value = "1s",
        value_parser = parse_duration,
    )]
    interval: Duration,

    /// The first TTL to probe.
    #[arg(
        long,
        value_name = "N",
        default_value_t = FIRST_TTL_DEFAULT,
        value_parser = clap::value_parser!(u8).range(TTL_LOWEST..=TTL_HIGHEST),
    )]
    first_ttl: u8,

    /// The last TTL to probe.
    #[arg(
        long,
        value_name = "N",
        default_value_t = MAX_TTL_DEFAULT,
        value_parser = clap::value_parser!(u8).range(TTL_LOWEST..=TTL_HIGHEST),
    )]
    max_ttl: u8,

    /// The protocol of a probe.
    #[arg(long, value_name = "P", value_enum, default_value_t = Protocol::Icmp)]
    protocol: Protocol,

    /// The multipath mode. UDP and TCP only.
    #[arg(long, value_name = "M", value_enum, default_value_t = Multipath::Classic)]
    multipath: Multipath,

    /// Force IP version 4.
    #[arg(short = '4', conflicts_with = "ipv6")]
    ipv4: bool,

    /// Force IP version 6.
    #[arg(short = '6')]
    ipv6: bool,

    /// Skip reverse DNS. Show addresses only.
    #[arg(long)]
    no_dns: bool,

    /// Override the source label in the derived filename.
    #[arg(long, value_name = "IP")]
    source: Option<IpAddr>,

    /// No table. Print one status line per minute.
    #[arg(long)]
    headless: bool,

    /// Stop after this much time.
    #[arg(long, value_name = "DUR", value_parser = parse_duration)]
    duration: Option<Duration>,

    /// Stop after this many rounds.
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u64).range(ROUNDS_LOWEST..),
    )]
    rounds: Option<u64>,

    /// Fold a recorded file and print the table. Then exit. A replay takes no
    /// destination.
    #[arg(long, value_name = "FILE")]
    replay: Option<PathBuf>,

    /// With `--replay`, pick which run in the file to fold.
    #[arg(long, value_name = "ID", requires = "replay")]
    run: Option<String>,
}

/// The configuration of one run, after the command line resolves.
///
/// Every field holds a resolved value and not a flag, so a later slice reads
/// the behavior of the run and never reads the switch that made it.
#[derive(Debug)]
struct ResolvedConfig {
    /// The host or the address to trace. A replay traces nothing, so a replay
    /// takes no destination and this field holds none.
    destination: Option<String>,
    /// The JSONL path the user named. An absent path is derived at run time.
    output: Option<PathBuf>,
    /// The period of one round.
    interval: Duration,
    /// The first TTL to probe.
    first_ttl: u8,
    /// The last TTL to probe.
    max_ttl: u8,
    /// The protocol of a probe.
    protocol: Protocol,
    /// The way a probe keeps or varies the flow of a packet.
    multipath: Multipath,
    /// The IP version of a probe.
    address_family: AddressFamily,
    /// True when the tool reads the name of each hop.
    reverse_dns: bool,
    /// The source address the user named for the derived filename.
    source: Option<IpAddr>,
    /// True when the tool prints status lines and no table.
    headless: bool,
    /// The time that stops the run. An absent time runs until the user stops it.
    duration: Option<Duration>,
    /// The number of rounds that stops the run.
    rounds: Option<u64>,
    /// The recorded file to fold and print.
    replay: Option<PathBuf>,
    /// The run in the replay file to fold.
    run: Option<String>,
}

impl Cli {
    /// Resolves the command line into the configuration of one run.
    ///
    /// The two flags of the address family collapse into one value, and the
    /// `--no-dns` switch becomes the behavior it controls.
    ///
    /// # Errors
    ///
    /// Returns the reason as text when two flags contradict each other. A first
    /// TTL above the max TTL leaves no hop to probe. A multipath mode other
    /// than `classic` needs UDP or TCP, because ICMP carries no flow to vary.
    fn resolve(self) -> Result<ResolvedConfig, String> {
        if self.first_ttl > self.max_ttl {
            return Err(format!(
                "`--first-ttl {}` is above `--max-ttl {}`: the first TTL starts the probe and the max TTL ends it",
                self.first_ttl, self.max_ttl
            ));
        }

        let carries_a_flow = matches!(self.protocol, Protocol::Udp | Protocol::Tcp);
        if self.multipath != Multipath::Classic && !carries_a_flow {
            return Err(format!(
                "`--multipath {}` needs `--protocol udp` or `--protocol tcp`, but the protocol is `{}`",
                value_name(&self.multipath),
                value_name(&self.protocol)
            ));
        }

        // The parser rejects the two flags of the address family together, so
        // one flag at most is true here.
        let address_family = if self.ipv4 {
            AddressFamily::Version4
        } else if self.ipv6 {
            AddressFamily::Version6
        } else {
            AddressFamily::Auto
        };

        Ok(ResolvedConfig {
            destination: self.destination,
            output: self.output,
            interval: self.interval,
            first_ttl: self.first_ttl,
            max_ttl: self.max_ttl,
            protocol: self.protocol,
            multipath: self.multipath,
            address_family,
            reverse_dns: !self.no_dns,
            source: self.source,
            headless: self.headless,
            duration: self.duration,
            rounds: self.rounds,
            replay: self.replay,
            run: self.run,
        })
    }
}

impl fmt::Display for ResolvedConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path_or = |path: Option<&PathBuf>, absent: &str| {
            path.map_or_else(|| absent.to_owned(), |path| path.display().to_string())
        };
        let rows = [
            (
                "destination",
                self.destination
                    .clone()
                    .unwrap_or_else(|| ABSENT.to_owned()),
            ),
            ("output", path_or(self.output.as_ref(), OUTPUT_DERIVED)),
            ("interval", render_duration(self.interval)),
            ("first ttl", self.first_ttl.to_string()),
            ("max ttl", self.max_ttl.to_string()),
            ("protocol", value_name(&self.protocol)),
            ("multipath", value_name(&self.multipath)),
            ("address family", self.address_family.to_string()),
            (
                "reverse dns",
                if self.reverse_dns { "on" } else { "off" }.to_owned(),
            ),
            (
                "source",
                self.source.map_or_else(
                    || SOURCE_DISCOVERED.to_owned(),
                    |address| address.to_string(),
                ),
            ),
            (
                "display",
                if self.headless { "headless" } else { "table" }.to_owned(),
            ),
            (
                "duration limit",
                self.duration
                    .map_or_else(|| ABSENT.to_owned(), render_duration),
            ),
            (
                "round limit",
                self.rounds
                    .map_or_else(|| ABSENT.to_owned(), |rounds| rounds.to_string()),
            ),
            ("replay", path_or(self.replay.as_ref(), ABSENT)),
            (
                "run",
                self.run.clone().unwrap_or_else(|| RUN_LATEST.to_owned()),
            ),
        ];

        writeln!(formatter, "resolved configuration:")?;
        for (key, value) in rows {
            let key = format!("{key}:");
            writeln!(formatter, "  {key:<CONFIG_KEY_WIDTH$}{value}")?;
        }
        Ok(())
    }
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
    // The parse handles `--version`, `-V`, and `--help` on its own. A
    // contradiction between two flags leaves the parser, so `clap` writes it to
    // standard error in the style of every other error of a command line.
    let cli = Cli::parse();
    match cli.resolve() {
        Ok(config) => print!("{config}"),
        Err(message) => Cli::command()
            .error(clap::error::ErrorKind::ValueValidation, message)
            .exit(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_duration, render_duration, AddressFamily, Cli, Multipath, Protocol, ResolvedConfig,
    };
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

    /// Resolves a command line that holds no contradiction.
    fn resolve(arguments: &[&str]) -> ResolvedConfig {
        parse(arguments)
            .resolve()
            .expect("the command line must resolve")
    }

    /// Reads the message of a command line that contradicts itself.
    fn contradiction(arguments: &[&str]) -> String {
        parse(arguments)
            .resolve()
            .expect_err("the command line must contradict itself")
    }

    /// The block that `krt example.com` prints, with every default.
    const DEFAULT_BLOCK: &str = "\
resolved configuration:
  destination:    example.com
  output:         derived at run time
  interval:       1s
  first ttl:      1
  max ttl:        30
  protocol:       icmp
  multipath:      classic
  address family: auto
  reverse dns:    on
  source:         discovered at run time
  display:        table
  duration limit: none
  round limit:    none
  replay:         none
  run:            the last run
";

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
    fn rejects_a_destination_beside_a_replay() {
        let error = rejection(&["krt", "example.com", "--replay", "path.jsonl"]);
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
        let message = error.to_string();
        assert!(
            message.contains("DESTINATION"),
            "the message names the destination: {message}"
        );
        assert!(
            message.contains("--replay"),
            "the message names the replay: {message}"
        );
    }

    #[test]
    fn rejects_a_run_without_a_replay() {
        let error = rejection(&["krt", "--run", "2026-08-19T12:00:00Z"]);
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
        let message = error.to_string();
        assert!(
            message.contains("--replay"),
            "the message names the replay: {message}"
        );
    }

    #[test]
    fn rejects_a_run_beside_a_destination() {
        let error = rejection(&["krt", "example.com", "--run", "2026-08-19T12:00:00Z"]);
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
        let message = error.to_string();
        assert!(
            message.contains("DESTINATION"),
            "the message names the destination: {message}"
        );
        assert!(
            message.contains("--run"),
            "the message names the run: {message}"
        );
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
    fn parses_a_round_limit_of_one() {
        let cli = parse(&["krt", "example.com", "--rounds", "1"]);
        assert_eq!(cli.rounds, Some(1));
    }

    #[test]
    fn rejects_a_round_limit_of_zero() {
        let error = rejection(&["krt", "example.com", "--rounds", "0"]);
        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }

    #[test]
    fn parses_the_flags_that_hold_no_value() {
        let cli = parse(&["krt", "example.com", "--no-dns", "--headless"]);
        assert!(cli.no_dns);
        assert!(cli.headless);
    }

    #[test]
    fn a_resolved_configuration_holds_every_documented_default() {
        let config = resolve(&["krt", "example.com"]);
        assert_eq!(config.destination.as_deref(), Some("example.com"));
        assert_eq!(config.output, None);
        assert_eq!(config.interval, Duration::from_secs(1));
        assert_eq!(config.first_ttl, 1);
        assert_eq!(config.max_ttl, 30);
        assert_eq!(config.protocol, Protocol::Icmp);
        assert_eq!(config.multipath, Multipath::Classic);
        assert_eq!(config.address_family, AddressFamily::Auto);
        assert!(config.reverse_dns, "reverse DNS is on by default");
        assert_eq!(config.source, None);
        assert!(!config.headless, "the table is on by default");
        assert_eq!(config.duration, None);
        assert_eq!(config.rounds, None);
        assert_eq!(config.replay, None);
        assert_eq!(config.run, None);
    }

    #[test]
    fn the_flag_of_ip_version_4_resolves_to_version_4() {
        let config = resolve(&["krt", "example.com", "-4"]);
        assert_eq!(config.address_family, AddressFamily::Version4);
    }

    #[test]
    fn the_flag_of_ip_version_6_resolves_to_version_6() {
        let config = resolve(&["krt", "example.com", "-6"]);
        assert_eq!(config.address_family, AddressFamily::Version6);
    }

    #[test]
    fn the_no_dns_flag_turns_reverse_dns_off() {
        let config = resolve(&["krt", "example.com", "--no-dns"]);
        assert!(!config.reverse_dns);
    }

    #[test]
    fn rejects_a_first_ttl_above_the_max_ttl() {
        let message = contradiction(&["krt", "example.com", "--first-ttl", "5", "--max-ttl", "3"]);
        for part in ["--first-ttl", "5", "--max-ttl", "3"] {
            assert!(
                message.contains(part),
                "the message names `{part}`: {message}"
            );
        }
    }

    #[test]
    fn a_first_ttl_equal_to_the_max_ttl_resolves() {
        let config = resolve(&["krt", "example.com", "--first-ttl", "4", "--max-ttl", "4"]);
        assert_eq!(config.first_ttl, 4);
        assert_eq!(config.max_ttl, 4);
    }

    #[test]
    fn rejects_a_multipath_mode_that_the_protocol_cannot_carry() {
        let message = contradiction(&["krt", "example.com", "--multipath", "paris"]);
        for part in ["--multipath", "paris", "--protocol", "icmp"] {
            assert!(
                message.contains(part),
                "the message names `{part}`: {message}"
            );
        }
    }

    #[test]
    fn a_multipath_mode_resolves_with_udp_and_with_tcp() {
        for (mode, protocol) in [("paris", "udp"), ("dublin", "tcp")] {
            let config = resolve(&[
                "krt",
                "example.com",
                "--multipath",
                mode,
                "--protocol",
                protocol,
            ]);
            assert_eq!(
                config.multipath,
                match mode {
                    "paris" => Multipath::Paris,
                    _ => Multipath::Dublin,
                },
                "`--multipath {mode} --protocol {protocol}`"
            );
        }
    }

    #[test]
    fn the_classic_multipath_mode_resolves_with_every_protocol() {
        let config = resolve(&["krt", "example.com", "--multipath", "classic"]);
        assert_eq!(config.multipath, Multipath::Classic);
        assert_eq!(config.protocol, Protocol::Icmp);
    }

    #[test]
    fn prints_every_default_of_a_resolved_configuration() {
        assert_eq!(resolve(&["krt", "example.com"]).to_string(), DEFAULT_BLOCK);
    }

    #[test]
    fn prints_every_value_that_a_flag_changed() {
        let config = resolve(&[
            "krt",
            "example.com",
            "--output",
            "/tmp/x.jsonl",
            "--interval",
            "500ms",
            "--first-ttl",
            "2",
            "--max-ttl",
            "20",
            "--protocol",
            "udp",
            "--multipath",
            "paris",
            "-6",
            "--no-dns",
            "--source",
            "1.2.3.4",
            "--headless",
            "--duration",
            "2m",
            "--rounds",
            "10",
        ]);
        assert_eq!(
            config.to_string(),
            "\
resolved configuration:
  destination:    example.com
  output:         /tmp/x.jsonl
  interval:       500ms
  first ttl:      2
  max ttl:        20
  protocol:       udp
  multipath:      paris
  address family: ipv6
  reverse dns:    off
  source:         1.2.3.4
  display:        headless
  duration limit: 2m
  round limit:    10
  replay:         none
  run:            the last run
"
        );
    }

    #[test]
    fn prints_the_file_and_the_run_of_a_replay() {
        let config = resolve(&[
            "krt",
            "--replay",
            "/tmp/r.jsonl",
            "--run",
            "2026-08-19T12:00:00Z",
        ]);
        assert_eq!(
            config.to_string(),
            "\
resolved configuration:
  destination:    none
  output:         derived at run time
  interval:       1s
  first ttl:      1
  max ttl:        30
  protocol:       icmp
  multipath:      classic
  address family: auto
  reverse dns:    on
  source:         discovered at run time
  display:        table
  duration limit: none
  round limit:    none
  replay:         /tmp/r.jsonl
  run:            2026-08-19T12:00:00Z
"
        );
    }
}
