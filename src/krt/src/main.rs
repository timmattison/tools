//! `krt` (Knights of the Round Trip) records the network path to a
//! destination, hop by hop.
//!
//! A command line that names a destination prints the configuration that it
//! resolved, opens the recorded file, starts the tracer, and appends one record
//! for each round until a limit stops the run. A run that holds a terminal
//! draws the live table of that path and takes the keys of the terminal. A run
//! whose standard output is a pipe or a file, and a run that `--headless`
//! asked, print one status line each minute in the place of the table. The
//! `replay` command reads a recorded file, folds one run of it, and prints the
//! table of that path: a header line that names the run, and one row for each
//! TTL.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod live;
mod names;
mod record;
mod run;
mod source;
mod stats;
#[cfg(test)]
mod testing;
mod trace;
mod ui;

use buildinfo::version_string;
use chrono::Utc;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use record::{
    EndReason, Family, Recording, RunConfig, RunId, RunRecord, SourceKind, SourceLabel, Target,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::io::IsTerminal;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The name that starts every message that `krt` writes to standard error.
const PROGRAM: &str = "krt";

/// The exit code of a failure.
const EXIT_FAILURE: i32 = 1;

/// The exit code of a platform that needs raw socket privileges and does not
/// hold them.
const EXIT_NO_PRIVILEGES: i32 = 2;

/// The exit code of a run whose recorded file did not take a record.
///
/// The recording is the whole purpose of the tool. A run that cannot record
/// stops, because a run that keeps a display while it silently records nothing
/// is worse than a run that stops.
const EXIT_WRITE_FAILED: i32 = 3;

/// The exit code of a run whose tracer stopped, or never started.
const EXIT_TRACER_FAILED: i32 = 4;

/// What a trace prints before it probes, ahead of the path of its file.
const RECORDING_TO: &str = "recording to";

/// What a trace prints when it stops, ahead of the count of its rounds.
const RECORDED: &str = "recorded";

/// What the warning of an unread source address says after the reason.
const SOURCE_FALLBACK: &str = "The run records the unspecified address of the family in its place.";

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

/// The text between two fields of a status line and of the closing line.
const SUMMARY_SEPARATOR: &str = "  ";

/// The value of a field that `krt` cannot fill.
///
/// A replay of a run whose `run` record is absent names no target, and a
/// machine that reports no name leaves the run without a host. Both fields
/// carry this word in the place of the value.
const UNKNOWN: &str = "unknown";

/// The port of a resolution.
///
/// The resolver takes a host and a port together, and it gives back socket
/// addresses. A trace of `krt` probes no port of the destination, so this
/// number takes no part in the answer.
const RESOLVE_PORT: u16 = 0;

/// The flag that asks for ip version 4.
const FLAG_VERSION_4: &str = "-4";

/// The flag that asks for ip version 6.
const FLAG_VERSION_6: &str = "-6";

/// The name of one round of a run, in a status line and in the header line of
/// a frame.
const ROUND: &str = "round";

/// The name of one TTL that answered, in the status line of one round.
const HOP: &str = "hop";

/// The last field of the status line of one round that reached the target.
const REACHED: &str = "reached";

/// The last field of the status line of one round that did not reach the
/// target.
///
/// The table of a replay says the same thing with the star of the destination:
/// a run that never reached the target holds no row that carries the star. A
/// status line stands alone, with no table above it, so it says the words.
const NEVER_REACHED: &str = "never reached";

/// The name of one recorded run, in the note that names the folded one.
const RUN: &str = "run";

/// What the note of a file of more than one run says about the file, ahead of
/// the count of the runs.
const THE_FILE_HOLDS: &str = "the file holds";

/// What that same note says ahead of the identifier of the folded run.
const THIS_FRAME_FOLDS: &str = "This frame folds the run";

/// The reason of a file that holds no run.
///
/// A file that holds no run at all stops the message here. A `--run` that names
/// a run the file does not hold adds that identifier and the runs of the file.
const NO_RUN: &str = "the file holds no run";

/// What the message of an absent run says before it lists the runs of the file.
const THE_RUNS_OF_THE_FILE: &str = "The runs of this file are";

/// The text between two run identifiers of a message.
const RUN_LIST_SEPARATOR: &str = ", ";

/// What a warning says about the rounds before a cut final line.
const RECORDS_BEFORE_THE_CUT: &str = "The records before the cut still read.";

/// The protocol of a probe.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Protocol {
    /// Send ICMP echo requests.
    Icmp,
    /// Send UDP datagrams.
    Udp,
    /// Send TCP packets.
    Tcp,
}

/// The way a probe keeps or varies the flow of a packet.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
/// A trace resolves the destination, opens the recorded file, and probes every
/// hop to the destination once per round. It appends one record for each round.
/// A run under a terminal draws the live table of that path, and it takes the
/// keys of the terminal. A run whose standard output is a pipe or a file, and a
/// run that `--headless` asked, print one status line each minute. The `replay`
/// command reads a file that an earlier run wrote, so it takes no destination
/// and no flag of a probe.
#[derive(Parser, Debug)]
// `args_conflicts_with_subcommands` rejects a flag of a probe beside a command,
// because a replay probes nothing. `subcommand_negates_reqs` lifts the demand
// for a destination when the line names a command, so the destination stays
// plainly required and a replay still needs none. The two rules then live in
// the shape of the command line, and no message can ask for an argument that
// another rule forbids.
#[command(
    name = PROGRAM,
    version = version_string!(),
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true,
)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag of the design is one switch of the command line"
)]
struct Cli {
    /// The host or the address to trace.
    #[arg(value_name = "DESTINATION", required = true)]
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

    /// Override the source label in the derived filename. Skip the lookup of
    /// the public address.
    #[arg(long, value_name = "IP")]
    source: Option<IpAddr>,

    /// No table and no keys. Print one status line per minute.
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

    /// The command that reads recorded work in the place of a trace.
    #[command(subcommand)]
    command: Option<Command>,
}

/// A command that reads recorded work in the place of a new trace.
#[derive(Subcommand, Debug, PartialEq, Eq)]
enum Command {
    /// Fold a recorded file and print what it holds. Then exit.
    Replay {
        /// The recorded file to fold.
        #[arg(value_name = "FILE")]
        file: PathBuf,

        /// Pick which run in the file to fold. The last run is the default.
        #[arg(long, value_name = "ID")]
        run: Option<String>,
    },
}

/// The configuration of one run, after the command line resolves.
///
/// Every field holds a resolved value and not a flag, so a reader of the
/// configuration reads the behavior of the run and never reads the switch that
/// made it.
#[derive(Debug)]
struct ResolvedConfig {
    /// The host or the address to trace. A replay traces nothing, so the
    /// `replay` command takes no destination and this field holds none.
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
    /// The recorded file to fold and print. The `replay` command names it.
    replay: Option<PathBuf>,
    /// The run in the recorded file to fold.
    run: Option<String>,
}

impl Cli {
    /// Resolves the command line into the configuration of one run.
    ///
    /// The two flags of the address family collapse into one value, and the
    /// `--no-dns` switch becomes the behavior it controls. The `replay` command
    /// becomes the recorded file and the run to fold, so every reader takes one
    /// flat configuration and never reads the shape of the command line.
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

        let (replay, run) = match self.command {
            Some(Command::Replay { file, run }) => (Some(file), run),
            None => (None, None),
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
            replay,
            run,
        })
    }
}

/// Writes the block that a trace prints before it probes.
///
/// The block names no `replay` and no `run`. `main` prints the block only when
/// the command line names no `replay`, and `resolve` fills the run only inside
/// a `replay`, so neither field can reach the block with a value. A replay
/// prints the table of the run it folded in the place of the block.
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
            ("interval", ui::render_duration(self.interval)),
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
                    .map_or_else(|| ABSENT.to_owned(), ui::render_duration),
            ),
            (
                "round limit",
                self.rounds
                    .map_or_else(|| ABSENT.to_owned(), |rounds| rounds.to_string()),
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
/// `ui::render_duration` writes the text that this function reads. The two live
/// apart because the header line of the frame writes a duration as well, and one
/// writer keeps the three places that print a period in agreement. A test below
/// asserts that the pair agrees over every accepted form.
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

/// Picks the address of the version that the run asked for.
///
/// The `auto` family takes the first address that the resolver named, whatever
/// its version.
fn pick_address(found: &[SocketAddr], family: AddressFamily) -> Option<IpAddr> {
    found
        .iter()
        .map(SocketAddr::ip)
        .find(|address| match family {
            AddressFamily::Auto => true,
            AddressFamily::Version4 => address.is_ipv4(),
            AddressFamily::Version6 => address.is_ipv6(),
        })
}

/// Writes the reason that a destination names no address to probe.
///
/// A run that asked for one version names that version and the flag that asks
/// for it, so the user reads one line and corrects the command line. A run that
/// asked for no version takes an address of either version, so no flag of the
/// command line changed the answer, and the message names none.
fn no_address_message(destination: &str, family: AddressFamily) -> String {
    let flag = match family {
        AddressFamily::Auto => {
            return format!(
                "`{destination}` names no address: name a destination that resolves to one"
            );
        }
        AddressFamily::Version4 => FLAG_VERSION_4,
        AddressFamily::Version6 => FLAG_VERSION_6,
    };
    format!(
        "`{destination}` names no {family} address: drop `{flag}`, or name a destination that holds one"
    )
}

/// Why a destination names no address to probe.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
enum ResolveError {
    /// The resolver refused the destination.
    #[error("`{destination}` does not resolve: {reason}")]
    Lookup {
        /// The destination as the user typed it.
        destination: String,
        /// The reason that the resolver gave.
        reason: String,
    },
    /// The destination names no address of the version that the run asked for.
    #[error("{}", no_address_message(destination, *family))]
    NoAddress {
        /// The destination as the user typed it.
        destination: String,
        /// The IP version that the run asked for.
        family: AddressFamily,
    },
}

/// Reads the address that a destination names.
///
/// The destination is a host name or a literal address, and the answer carries
/// the destination as the user typed it, the address, and the version of that
/// address.
///
/// # Errors
///
/// Returns [`ResolveError::Lookup`] when the resolver refuses the destination.
/// Returns [`ResolveError::NoAddress`] when the destination names no address of
/// the version that the run asked for.
fn resolve_target(destination: &str, family: AddressFamily) -> Result<Target, ResolveError> {
    let found: Vec<SocketAddr> = (destination, RESOLVE_PORT)
        .to_socket_addrs()
        .map_err(|error| ResolveError::Lookup {
            destination: destination.to_owned(),
            reason: error.to_string(),
        })?
        .collect();
    let addr = pick_address(&found, family).ok_or_else(|| ResolveError::NoAddress {
        destination: destination.to_owned(),
        family,
    })?;
    Ok(Target {
        arg: destination.to_owned(),
        addr,
        family: match addr {
            IpAddr::V4(_) => Family::Ipv4,
            IpAddr::V6(_) => Family::Ipv6,
        },
    })
}

/// The name of the machine that makes a run.
fn host_name() -> String {
    host_name_or(sysinfo::System::host_name())
}

/// The name of the machine, from what the system reported.
///
/// A system that reports no name, and a system that reports an empty one, both
/// leave the run without a name, so the record carries one word in its place.
fn host_name_or(reported: Option<String>) -> String {
    reported
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| UNKNOWN.to_owned())
}

/// The signals that stop a run.
///
/// SIGINT is the Ctrl-C of the terminal, and SIGTERM is the polite kill.
/// SIGKILL is the one signal that no program handles, and `krt` does not
/// pretend otherwise.
#[cfg(unix)]
const TERMINATION_SIGNALS: [std::os::raw::c_int; 2] =
    [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM];

// No test calls `stop_flag`. The registration changes the whole process, so a
// test that called it would take the Ctrl-C of `cargo test` away from the test
// runner. `user_stopped` carries the behavior that a test drives.

/// A flag that a termination signal sets.
///
/// The run loop reads the flag once per turn of its loop. A run that the user
/// stops therefore writes the record that closes it and flushes the file. A run
/// that registers no handler loses that record, because the signal ends the
/// process where it stands.
///
/// # Errors
///
/// Returns the reason when the platform refuses the registration.
#[cfg(unix)]
fn stop_flag() -> std::io::Result<Arc<AtomicBool>> {
    let flag = Arc::new(AtomicBool::new(false));
    for signal in TERMINATION_SIGNALS {
        signal_hook::flag::register(signal, Arc::clone(&flag))?;
    }
    Ok(flag)
}

/// A flag that a termination signal sets.
///
/// This platform registers no handler, so nothing sets the flag. The user of
/// this platform stops a run with the `q` key or with Ctrl-C, which the key
/// handler of the live table reads. A run of this platform that draws no table
/// reads no key, and only a limit of the command line stops such a run.
///
/// # Errors
///
/// Returns no reason. The result holds the shape of the unix build, so one call
/// site serves both platforms.
#[cfg(not(unix))]
fn stop_flag() -> std::io::Result<Arc<AtomicBool>> {
    Ok(Arc::new(AtomicBool::new(false)))
}

/// Answers whether the user stopped the run.
fn user_stopped(flag: &AtomicBool) -> bool {
    flag.load(Ordering::Relaxed)
}

/// Writes a count and the name of what it counts.
///
/// One of a thing keeps the singular name, and every other count adds one `s`.
/// The two names of this file, `round` and `run`, both take that plural.
fn counted(count: usize, name: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {name}{plural}")
}

/// Writes the reason of a file that holds no run to fold.
///
/// A `--run` that names a run the file does not hold adds that identifier and
/// every run that the file does hold, so the user reads one line and corrects
/// the flag. A file that holds no run at all has nothing to name, and a message
/// that promises a list and then holds none reads as a defect of the tool. Such
/// a message stops at the reason.
fn no_run_message(path: &Path, wanted: Option<&str>, held: &[RunId]) -> String {
    let reason = format!("{}: {NO_RUN}", path.display());
    match wanted {
        Some(wanted) if !held.is_empty() => {
            let names: Vec<String> = held.iter().map(|id| format!("`{id}`")).collect();
            format!(
                "{reason} `{wanted}`. {THE_RUNS_OF_THE_FILE} {}",
                names.join(RUN_LIST_SEPARATOR)
            )
        }
        _ => reason,
    }
}

/// The name of a recorded file, without its directory.
///
/// The directory holds columns that the table needs for its numbers, and it
/// says nothing about the run: the user named the file to the `replay` command,
/// so the user already knows where it stands. A path that ends in no name — the
/// root of a file system does — keeps every part of itself, because a header
/// line that named no file at all would tell a reader less.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Writes the note that names the run that the frame folded.
///
/// The header line of the frame names the target of the run and not the run
/// itself, because the identifier is a moment and every row of the table stands
/// under the one moment. A file of two runs would therefore leave a reader
/// unable to tell which of the two the frame folded, so the note says it. The
/// note goes to standard error, where the warning of a cut file already goes,
/// so standard output stays the frame alone: a reader who redirects a replay
/// gets a table and nothing else.
fn folded_run_message(path: &Path, runs: usize, folded: &RunId) -> String {
    format!(
        "{}: {THE_FILE_HOLDS} {}. {THIS_FRAME_FOLDS} `{folded}`.",
        path.display(),
        counted(runs, RUN)
    )
}

/// Reads a recorded file and folds one run of it into the lines of one frame.
///
/// The run that `--run` names is the run to fold, and the last run of the file
/// is the run to fold when the flag is absent.
///
/// The width is the number of terminal columns that the frame draws in. The
/// caller reads it, so a test of the fold names the width it wants and never
/// the terminal that ran it.
///
/// A file that does not read at all, a file that holds no run, and a file that
/// does not hold the run that `--run` names each give the reason in the
/// outcome. The warning of a cut final line rides beside the outcome and not
/// inside it, because a cut is often the reason that the file holds no run to
/// fold: a `kill -9` during the first record leaves a file that holds no
/// complete record, and such a file reads as an empty one until the warning
/// says otherwise.
fn replay(path: &Path, wanted: Option<&str>, width: u16) -> Replay {
    let recording = match Recording::read(path) {
        Ok(recording) => recording,
        Err(error) => {
            return Replay {
                warning: None,
                outcome: Err(error.to_string()),
            };
        }
    };
    // A `kill -9` leaves a file whose final line is cut short. Every round
    // before the cut still reads, so the replay reports the cut and goes on.
    let warning = recording
        .truncated()
        .map(|truncated| format!("{}: {truncated}. {RECORDS_BEFORE_THE_CUT}", path.display()));
    let found = match wanted {
        Some(wanted) => recording.run(&RunId::from(wanted)),
        None => recording.last_run(),
    };
    let held = recording.run_ids();
    let outcome = match found {
        Some(run) => {
            let mut table = stats::HopTable::new();
            for round in run.rounds() {
                table.observe(round);
            }
            // A `name` record names one address, and one address answers at any
            // number of TTLs, so the map is keyed by the address and not by the
            // hop. A run of this build writes such a record. A file that an
            // older build recorded holds none, and the map then leaves the
            // address raw.
            let names: BTreeMap<IpAddr, String> = run
                .names()
                .iter()
                .map(|name| (name.addr, name.host.clone()))
                .collect();
            let start = run.start();
            let file = file_name(path);
            let frame = ui::Frame {
                header: ui::Header {
                    destination: start.map(|start| start.target.arg.as_str()),
                    address: start.map(|start| start.target.addr),
                    source: start.map(|start| start.source.addr),
                    rounds: run.rounds().len(),
                    interval: start.map(|start| Duration::from_millis(start.config.interval_ms)),
                    file: &file,
                    // A file that the run cannot measure still folds, and the
                    // header then names the file and no size.
                    bytes: std::fs::metadata(path).ok().map(|data| data.len()),
                },
                table: &table,
                names: &names,
                destination: start.map(|start| start.target.addr),
            };
            Ok(Folded {
                lines: frame.lines(width),
                note: (held.len() > 1).then(|| folded_run_message(path, held.len(), run.id())),
            })
        }
        None => Err(no_run_message(path, wanted, &held)),
    };
    Replay { warning, outcome }
}

/// The warning that a recorded file raised, and what the replay of it found.
struct Replay {
    /// The warning about the file, when the file holds a cut final line.
    warning: Option<String>,
    /// What the replay folded, or the reason that no run folds.
    outcome: Result<Folded, String>,
}

/// What a replay folded out of one run.
///
/// The frame and the note travel together, so one replay reads the file once
/// and every line of the answer comes from the same run.
struct Folded {
    /// The lines of the frame of the run.
    lines: Vec<String>,
    /// The note that names the folded run, when the file holds more than one.
    note: Option<String>,
}

/// The fault that stopped a trace, and the code that names its kind.
struct TraceFailure {
    /// The reason, as the user reads it.
    reason: String,
    /// The exit code of that kind of fault.
    code: i32,
}

impl TraceFailure {
    /// Builds the fault from a reason and the code of its kind.
    fn new(reason: &dyn fmt::Display, code: i32) -> Self {
        Self {
            reason: reason.to_string(),
            code,
        }
    }
}

/// The unspecified address of the family of an address.
fn unspecified_of(target: IpAddr) -> IpAddr {
    match target {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    }
}

/// The label that the record carries, and the one line that standard error
/// takes before the display starts.
///
/// A machine that names no route to the target still records. The search for
/// the source therefore stops no run: a search that read no address at all
/// leaves the unspecified address of the family of the target in the record,
/// and the warning names the fault. A machine on a captive network, and a
/// machine on a network with no route out, both still record. The first one
/// carries the warning that the search wrote, and the second one carries the
/// warning that this function writes.
///
/// The search runs once a run, so the warning prints once a run and never once
/// a round.
///
/// This function reads the outcome of the search and never makes it, and it
/// hands the warning back and never prints it. A test therefore drives each of
/// the three outcomes, and it reaches no network. [`trace()`] makes the search
/// and writes the line.
fn source_from(
    found: std::io::Result<source::Discovery>,
    target: IpAddr,
) -> (SourceLabel, Option<String>) {
    match found {
        Ok(discovery) => (discovery.label, discovery.note),
        Err(error) => (
            SourceLabel {
                addr: unspecified_of(target),
                kind: SourceKind::Local,
            },
            Some(format!(
                "the source address did not read: {error}. {SOURCE_FALLBACK}"
            )),
        ),
    }
}

/// The period of one round in milliseconds, for the record that opens a run.
///
/// A period too large for the field takes the largest number the field holds.
/// No command line reaches that period, because `parse_duration` stops well
/// below it.
fn interval_millis(interval: Duration) -> u64 {
    u64::try_from(interval.as_millis()).unwrap_or(u64::MAX)
}

/// The moment that the time limit of a run falls due.
///
/// A limit too large to add to the clock leaves the run without a moment, so
/// the run goes until the user stops it. No command line reaches that limit.
fn deadline_of(limit: Option<Duration>) -> Option<Instant> {
    limit.and_then(|limit| Instant::now().checked_add(limit))
}

/// Names why a run stopped, for the line that a trace prints when it stops.
fn stop_reason(reason: EndReason) -> &'static str {
    match reason {
        EndReason::Quit => "the user stopped the run",
        EndReason::Duration => "the time limit stopped the run",
        EndReason::Rounds => "the round limit stopped the run",
        // A fault leaves the run through an error, and `main` writes that
        // reason to standard error in the place of this line. The words are
        // here so the table of the reasons stays complete.
        EndReason::Error => "a fault stopped the run",
    }
}

/// Writes the one line that a trace prints when it stops.
///
/// The line holds the number of rounds that the run recorded, the file that
/// holds them, and why the run stopped. Two spaces separate the fields, as they
/// do on the status line of one round.
fn closing_line(outcome: &run::Outcome, path: &Path) -> String {
    let rounds = usize::try_from(outcome.rounds).unwrap_or(usize::MAX);
    [
        format!("{RECORDED} {}", counted(rounds, ROUND)),
        path.display().to_string(),
        stop_reason(outcome.reason).to_owned(),
    ]
    .join(SUMMARY_SEPARATOR)
}

/// The reverse resolver of one run.
///
/// `--no-dns` gives the resolver that looks nothing up, so no lookup of such a
/// run leaves the machine. Every other run takes the system resolver of the
/// platform.
///
/// A resolver that does not start stops the run, and that is a decision. The
/// crate reports a start failure as an `io::Error`, and `krt` treats one as
/// fatal. A fatal start also keeps the `dns` field of the `run` record true by
/// construction: a run that reaches the loop holds the resolver that the user
/// asked for.
///
/// # Errors
///
/// Returns the reason when the system resolver does not start.
fn resolver_of(reverse_dns: bool) -> std::io::Result<Box<dyn names::Resolver>> {
    if reverse_dns {
        trace::resolver()
    } else {
        Ok(Box::new(names::NoLookups))
    }
}

/// The grace that a run gives its names after its last round.
///
/// The grace is the timeout of the reverse resolver. Every lookup settles when
/// that time runs out, so this grace outlives every lookup that can still
/// answer, and a hop whose name server answers slowly still takes a `name`
/// record. The wait ends at the moment that no address waits, so a run whose
/// addresses settled pays none of it.
fn name_grace() -> Duration {
    trace::resolver_timeout()
}

/// Reads the configuration that one run records, out of the command line that
/// the run resolved and the privilege mode that the platform gave.
///
/// The `run` record states what the run does, and a reader of a recorded file
/// takes that statement as the truth. So every field here reads one field of
/// the resolved command line, and nothing here holds a value of its own.
///
/// The `dns` field is the one that a reader is most likely to doubt, because
/// a file that holds no `name` record reads the same whether `--no-dns` turned
/// the lookups off or whether no address of the run resolved. The field
/// separates the two.
fn run_config(config: &ResolvedConfig, privilege: record::Privilege) -> RunConfig {
    RunConfig {
        interval_ms: interval_millis(config.interval),
        protocol: config.protocol,
        first_ttl: config.first_ttl,
        max_ttl: config.max_ttl,
        multipath: config.multipath,
        privilege,
        dns: config.reverse_dns,
    }
}

/// The screen that a run shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Display {
    /// The live table of the path, which holds the terminal and reads its keys.
    Table,
    /// One status line each minute, and no table.
    Headless,
}

/// The screen that a run shows, from the `--headless` flag and from whether
/// standard output is a terminal.
///
/// The table holds the terminal in raw mode on the alternate screen, and it
/// draws one whole frame for each round. A run whose standard output is a pipe
/// or a file has no terminal to hold, no key to read, and no screen to clear,
/// and a table there writes one whole frame into that file for each round. Such
/// a run therefore takes the headless screen, which writes one line each
/// minute. `--headless` asks for that same line from a run that does hold a
/// terminal.
///
/// The read of the terminal stands apart from this decision, so a test names
/// the answer of a terminal without a terminal to name it with.
fn display_of(headless: bool, is_terminal: bool) -> Display {
    if headless || !is_terminal {
        return Display::Headless;
    }
    Display::Table
}

/// Records one trace, from the destination of the command line to the record
/// that closes the run.
///
/// The order of the steps is the order of their cost, and each one stops the
/// run before the next one spends anything. The resolution comes first, because
/// it needs no privilege and touches no file. The privilege gate comes next, so
/// a platform that cannot probe says so before the run makes a file. The
/// reverse resolver starts after the gate and before the recorded file opens,
/// so a run that cannot start its resolver makes no file and prints no path.
/// The recorded file opens before the tracer starts, so no probe leaves the
/// machine for a run that cannot record it.
///
/// The screen comes last of all, after every step that can print a line and
/// after every step that can stop the run. A live table takes the terminal and
/// draws on the alternate screen, and a screen that stood in front of those
/// steps would put each of their lines on a screen that the drop of the guard
/// then takes away.
///
/// # Errors
///
/// Returns the reason and the exit code of the fault that stopped the run.
fn trace(config: &ResolvedConfig) -> Result<run::Outcome, TraceFailure> {
    // The parser makes the destination required outside a command, so a trace
    // always names one.
    let destination = config.destination.as_deref().unwrap_or_default();

    let target = resolve_target(destination, config.address_family)
        .map_err(|error| TraceFailure::new(&error, EXIT_FAILURE))?;
    let privilege = trace::acquire_privilege()
        .map_err(|error| TraceFailure::new(&error, EXIT_NO_PRIVILEGES))?;
    // The resolver starts here, before the run makes a file and before it
    // prints the path of that file, so no message of a run that stops misleads
    // a reader.
    let resolver = resolver_of(config.reverse_dns).map_err(|error| {
        TraceFailure::new(
            &format!("the reverse resolver did not start: {error}"),
            EXIT_FAILURE,
        )
    })?;

    // The search runs here, and it runs once. The warning of a fallback
    // therefore reaches standard error once a run and never once a round, and
    // it lands before the recorded file opens and before the display starts, so
    // no line of the display covers it.
    let (source, warning) = source_from(source::discover(config.source, target.addr), target.addr);
    if let Some(warning) = warning {
        eprintln!("{PROGRAM}: {warning}");
    }
    let path = source::output_path(config.output.as_deref(), source.addr, destination);
    let mut writer = record::Writer::append(&path).map_err(|error| {
        TraceFailure::new(&format!("{}: {error}", path.display()), EXIT_WRITE_FAILED)
    })?;
    println!("{RECORDING_TO} {}", path.display());

    let run = RunId::at(Utc::now());
    let rounds = trace::spawn(&trace::TraceConfig {
        target: target.addr,
        run: run.clone(),
        interval: config.interval,
        first_ttl: config.first_ttl,
        max_ttl: config.max_ttl,
        protocol: config.protocol,
        multipath: config.multipath,
        privilege,
    })
    .map_err(|error| TraceFailure::new(&error, EXIT_TRACER_FAILED))?;

    let start = RunRecord {
        run,
        krt: version_string!().to_owned(),
        source,
        target,
        config: run_config(config, privilege),
        host: host_name(),
    };

    let flag = stop_flag().map_err(|error| {
        TraceFailure::new(
            &format!("the stop signal did not register: {error}"),
            EXIT_FAILURE,
        )
    })?;
    let limits = run::Limits {
        rounds: config.rounds,
        deadline: deadline_of(config.duration),
        name_grace: name_grace(),
    };
    let mut namer = names::Namer::new(resolver, start.run.clone());

    // The screen and the guard stand in a scope of their own, and the closing
    // line stands under it. The guard drops at the end of the scope, which
    // takes the alternate screen away and puts the lines of the reader back. A
    // line that printed in front of that drop lands on the alternate screen,
    // and the drop then takes the screen away with the line on it.
    let outcome = {
        // `_guard` is a name, and a name holds the guard for the whole scope.
        // A bare `_` in its place drops the guard where it stands, which gives
        // the terminal back before the first frame draws on it. The two spell
        // almost the same, and one of them is a defect.
        let (mut screen, _guard) = screen_of(config, &start, &path)?;
        run::record(
            &start,
            &rounds,
            &limits,
            &|| user_stopped(&flag),
            &mut namer,
            &mut writer,
            screen.as_mut(),
        )
        .map_err(|error| {
            let code = match error {
                run::RunError::Write(_) => EXIT_WRITE_FAILED,
                run::RunError::Tracer { .. } => EXIT_TRACER_FAILED,
            };
            TraceFailure::new(&error, code)
        })?
    };
    println!("{}", closing_line(&outcome, &path));
    Ok(outcome)
}

/// The screen of one run, and the hold that screen takes on the terminal.
///
/// The table holds the terminal, so it travels with the guard that gives that
/// terminal back. The headless screen holds nothing, and its guard is `None`.
/// The caller binds both, and every way out of the run then drops the guard:
/// the stop that the user asked for, the fault that ends the run early, and the
/// panic that nobody asked for.
///
/// The guard comes before the table, because the table draws into the terminal
/// that the guard takes.
///
/// # Errors
///
/// Returns the reason and [`EXIT_FAILURE`] when the terminal refused the hold.
/// A terminal that will not go into raw mode reads no key of the user, and a
/// run that drew a table on it would stop for nothing that the user pressed.
fn screen_of(
    config: &ResolvedConfig,
    start: &RunRecord,
    path: &Path,
) -> Result<(Box<dyn live::Screen>, Option<live::TerminalGuard>), TraceFailure> {
    if display_of(config.headless, std::io::stdout().is_terminal()) == Display::Headless {
        return Ok((
            Box::new(live::Headless::new(std::io::stdout(), live::SystemClock)),
            None,
        ));
    }
    let guard = live::TerminalGuard::enter().map_err(|error| {
        TraceFailure::new(
            &format!("the live table did not take the terminal: {error}"),
            EXIT_FAILURE,
        )
    })?;
    let facts = live::RunFacts {
        destination: start.target.arg.clone(),
        address: start.target.addr,
        source: start.source.addr,
        interval: config.interval,
        path: path.to_owned(),
    };
    let table = live::Table::new(
        facts,
        std::io::stdout(),
        live::Keyboard,
        ui::frame_columns(),
    );
    Ok((Box::new(table), Some(guard)))
}

fn main() {
    // The parse handles `--version`, `-V`, and `--help` on its own. A
    // contradiction between two flags leaves the parser, so `clap` writes it to
    // standard error in the style of every other error of a command line.
    let cli = Cli::parse();
    let config = match cli.resolve() {
        Ok(config) => config,
        Err(message) => Cli::command()
            .error(clap::error::ErrorKind::ValueValidation, message)
            .exit(),
    };
    let Some(path) = config.replay.as_deref() else {
        // The block names what the run will do, and then the run does it.
        print!("{config}");
        if let Err(failure) = trace(&config) {
            eprintln!("{PROGRAM}: {}", failure.reason);
            std::process::exit(failure.code);
        }
        return;
    };
    // The warning comes before the outcome, so a reader of standard error sees
    // the state of the file before the answer that state produced.
    let result = replay(path, config.run.as_deref(), ui::frame_columns());
    if let Some(warning) = result.warning {
        eprintln!("{PROGRAM}: {warning}");
    }
    match result.outcome {
        Ok(folded) => {
            // The note stands on standard error with the warning above it, so
            // standard output holds the frame and nothing else.
            if let Some(note) = folded.note {
                eprintln!("{PROGRAM}: {note}");
            }
            for line in folded.lines {
                println!("{line}");
            }
        }
        Err(reason) => {
            eprintln!("{PROGRAM}: {reason}");
            std::process::exit(EXIT_FAILURE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        closing_line, display_of, host_name_or, name_grace, parse_duration, pick_address,
        resolve_target, run_config, source_from, stop_reason, user_stopped, AddressFamily, Cli,
        Command, Display, EndReason, Family, Multipath, Protocol, ResolveError, ResolvedConfig,
        SourceKind, SourceLabel, RESOLVE_PORT, SOURCE_FALLBACK, UNKNOWN,
    };
    use crate::record::{Privilege, RunConfig};
    use crate::run::Outcome;
    use crate::source::Discovery;
    use crate::testing::address;
    use crate::ui::render_duration;
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    use clap::{CommandFactory, Parser};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
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

    /// The configuration that a command line records, under the privilege that
    /// a probe of this run needs.
    fn recorded_config(arguments: &[&str], privilege: Privilege) -> RunConfig {
        run_config(&resolve(arguments), privilege)
    }

    #[test]
    fn a_run_that_reads_names_records_that_it_reads_them() {
        assert!(
            recorded_config(&["krt", "example.com"], Privilege::Unprivileged).dns,
            "reverse DNS is on by default, and the record must say so"
        );
    }

    #[test]
    fn the_no_dns_flag_reaches_the_dns_field_of_the_record() {
        assert!(
            !recorded_config(&["krt", "example.com", "--no-dns"], Privilege::Unprivileged).dns,
            "`--no-dns` turns the lookups off, and the record must say so"
        );
    }

    #[test]
    fn every_field_of_the_recorded_config_reads_the_command_line_that_set_it() {
        let recorded = recorded_config(
            &[
                "krt",
                "example.com",
                "--interval",
                "2s",
                "--first-ttl",
                "3",
                "--max-ttl",
                "9",
                "--protocol",
                "udp",
                "--multipath",
                "paris",
            ],
            Privilege::Privileged,
        );
        assert_eq!(
            recorded,
            RunConfig {
                interval_ms: 2000,
                protocol: Protocol::Udp,
                first_ttl: 3,
                max_ttl: 9,
                multipath: Multipath::Paris,
                privilege: Privilege::Privileged,
                dns: true,
            }
        );
    }

    /// The grace that a run gives its names is never shorter than the longest
    /// that one lookup takes.
    ///
    /// A grace below the timeout of the resolver drops the name of a hop whose
    /// name server answers slowly and truly, and the file of the run then holds
    /// the raw address of that hop.
    #[test]
    fn the_grace_of_the_names_outlives_every_lookup_that_can_still_answer() {
        let grace = name_grace();
        let timeout = crate::trace::resolver_timeout();
        assert!(
            grace >= timeout,
            "the run gives its names {grace:?}, and one lookup takes up to {timeout:?}"
        );
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
";

    /// Every text that the parser rejects, for the message tests.
    const BAD_TEXTS: [&str; 16] = [
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
        "1秒",
        "1🎉",
        "1é",
        "秒",
        "café",
        "½s",
    ];

    /// Every text where a digit runs into a multi-byte character that is no unit.
    ///
    /// The characters are 3 bytes, 4 bytes, and 2 bytes long, in that order. The
    /// parser makes the message for an unknown unit.
    const MULTI_BYTE_UNKNOWN_UNITS: [&str; 3] = ["1秒", "1🎉", "1é"];

    /// Every text that starts with a multi-byte character in the place of a digit.
    ///
    /// `½` is a numeric character to Unicode, but it is no ASCII digit, so the
    /// parser makes the message for a text without a number.
    const MULTI_BYTE_TEXTS_WITHOUT_A_NUMBER: [&str; 3] = ["秒", "café", "½s"];

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
        for text in ["1sec", "5x"].into_iter().chain(MULTI_BYTE_UNKNOWN_UNITS) {
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
        for text in ["ms", "abc"]
            .into_iter()
            .chain(MULTI_BYTE_TEXTS_WITHOUT_A_NUMBER)
        {
            let message = error_of(text);
            assert!(
                message.contains("no number"),
                "the message names the fault: {message}"
            );
        }
    }

    /// The parser splits a text on a character boundary.
    ///
    /// The parser reads the split point from `char_indices`, so the point is
    /// always on a character boundary. A split point that mixes a count of
    /// characters with a count of bytes cuts a multi-byte character in half,
    /// and `split_at` panics. A measurement of the unit from the end of the
    /// text, such as `text.len() - text.chars().rev().take_while(|c|
    /// !c.is_ascii_digit()).count()`, is one such point: it panics on `½s`,
    /// `秒`, `1秒`, `1🎉`, and `1é`. This test holds the boundary in place.
    #[test]
    fn a_duration_that_holds_a_multi_byte_character_never_panics() {
        for text in MULTI_BYTE_UNKNOWN_UNITS
            .into_iter()
            .chain(MULTI_BYTE_TEXTS_WITHOUT_A_NUMBER)
        {
            let message = error_of(text);
            assert!(
                message.contains(text),
                "the message names the text: {message}"
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
        assert_eq!(cli.command, None, "a trace runs no command");
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
        let cli = parse(&["krt", "replay", "path.jsonl"]);
        assert_eq!(cli.destination, None);
        assert_eq!(
            cli.command,
            Some(Command::Replay {
                file: PathBuf::from("path.jsonl"),
                run: None,
            })
        );
    }

    #[test]
    fn rejects_a_destination_beside_a_replay() {
        let error = rejection(&["krt", "example.com", "replay", "path.jsonl"]);
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
        let message = error.to_string();
        assert!(
            message.contains("DESTINATION"),
            "the message names the destination: {message}"
        );
        assert!(
            message.contains("replay"),
            "the message names the replay: {message}"
        );
    }

    #[test]
    fn rejects_a_run_outside_the_replay_command() {
        let error = rejection(&["krt", "--run", "2026-08-19T12:00:00Z"]);
        assert_eq!(error.kind(), ErrorKind::UnknownArgument);
        let message = error.to_string();
        assert!(
            message.contains("--run"),
            "the message names the run: {message}"
        );
    }

    #[test]
    fn rejects_a_run_beside_a_destination() {
        let error = rejection(&["krt", "example.com", "--run", "2026-08-19T12:00:00Z"]);
        assert_eq!(error.kind(), ErrorKind::UnknownArgument);
        let message = error.to_string();
        assert!(
            message.contains("--run"),
            "the message names the run: {message}"
        );
    }

    #[test]
    fn parses_a_run_of_a_replay() {
        let cli = parse(&[
            "krt",
            "replay",
            "path.jsonl",
            "--run",
            "2026-08-19T12:00:00Z",
        ]);
        assert_eq!(
            cli.command,
            Some(Command::Replay {
                file: PathBuf::from("path.jsonl"),
                run: Some("2026-08-19T12:00:00Z".to_owned()),
            })
        );
    }

    /// The verdict the parser must reach for one row of the argument matrix.
    #[derive(Debug)]
    enum Verdict {
        /// The command line is accepted.
        Parses,
        /// The command line is rejected, with this reason.
        Rejects(ErrorKind),
    }

    /// One row of the matrix of the arguments that constrain each other.
    struct ArgumentRow {
        /// The command line, program name included.
        arguments: &'static [&'static str],
        /// The verdict the parser must reach.
        verdict: Verdict,
    }

    /// A destination, for a row that carries one.
    const A_DESTINATION: &str = "example.com";

    /// The name of the command that folds a recorded file.
    const REPLAY: &str = "replay";

    /// The path of a recorded file, for a row that carries one.
    const A_REPLAY_FILE: &str = "path.jsonl";

    /// The id of one run of a recorded file, for a row that carries one.
    const A_RUN_ID: &str = "2026-08-19T12:00:00Z";

    /// A whole replay, for a row that carries one.
    const A_REPLAY: [&str; 2] = [REPLAY, A_REPLAY_FILE];

    /// A run of a replay, for a row that carries one.
    const A_RUN: [&str; 2] = ["--run", A_RUN_ID];

    /// Every combination of the destination, the replay, and the run.
    ///
    /// The three arguments constrain each other, and clap resolves such
    /// relationships together rather than one at a time. A change to one of them
    /// moves the verdict of rows that name the other two, so the whole matrix is
    /// stated here and not one row per test. Two defects of this branch were of
    /// that kind, while `--replay` and `--run` were flags of a trace:
    /// `conflicts_with_all` on the destination stopped the `requires` of
    /// `--run` from firing, and it later made the usage line of a missing
    /// argument offer a destination that no longer fit beside `--run`. The
    /// `replay` command now holds both rules in the grammar, so the matrix
    /// records what the grammar gives.
    ///
    /// The replay holds three states, because the command carries the file it
    /// folds: no command, the command alone, and the command with a file. The
    /// run holds two, and the destination holds two, so the matrix has twelve
    /// rows.
    const ARGUMENT_MATRIX: [ArgumentRow; 12] = [
        ArgumentRow {
            arguments: &["krt"],
            verdict: Verdict::Rejects(ErrorKind::MissingRequiredArgument),
        },
        ArgumentRow {
            arguments: &["krt", A_RUN[0], A_RUN[1]],
            verdict: Verdict::Rejects(ErrorKind::UnknownArgument),
        },
        ArgumentRow {
            arguments: &["krt", REPLAY],
            verdict: Verdict::Rejects(ErrorKind::MissingRequiredArgument),
        },
        ArgumentRow {
            arguments: &["krt", REPLAY, A_RUN[0], A_RUN[1]],
            verdict: Verdict::Rejects(ErrorKind::MissingRequiredArgument),
        },
        ArgumentRow {
            arguments: &["krt", A_REPLAY[0], A_REPLAY[1]],
            verdict: Verdict::Parses,
        },
        ArgumentRow {
            arguments: &["krt", A_REPLAY[0], A_REPLAY[1], A_RUN[0], A_RUN[1]],
            verdict: Verdict::Parses,
        },
        ArgumentRow {
            arguments: &["krt", A_DESTINATION],
            verdict: Verdict::Parses,
        },
        ArgumentRow {
            arguments: &["krt", A_DESTINATION, A_RUN[0], A_RUN[1]],
            verdict: Verdict::Rejects(ErrorKind::UnknownArgument),
        },
        ArgumentRow {
            arguments: &["krt", A_DESTINATION, REPLAY],
            verdict: Verdict::Rejects(ErrorKind::ArgumentConflict),
        },
        ArgumentRow {
            arguments: &["krt", A_DESTINATION, REPLAY, A_RUN[0], A_RUN[1]],
            verdict: Verdict::Rejects(ErrorKind::ArgumentConflict),
        },
        ArgumentRow {
            arguments: &["krt", A_DESTINATION, A_REPLAY[0], A_REPLAY[1]],
            verdict: Verdict::Rejects(ErrorKind::ArgumentConflict),
        },
        ArgumentRow {
            arguments: &[
                "krt",
                A_DESTINATION,
                A_REPLAY[0],
                A_REPLAY[1],
                A_RUN[0],
                A_RUN[1],
            ],
            verdict: Verdict::Rejects(ErrorKind::ArgumentConflict),
        },
    ];

    #[test]
    fn the_argument_matrix_holds() {
        for row in &ARGUMENT_MATRIX {
            let line = row.arguments.join(" ");
            match row.verdict {
                Verdict::Parses => {
                    if let Err(error) = Cli::try_parse_from(row.arguments.iter().copied()) {
                        panic!("`{line}` must parse, but the parser rejected it: {error}");
                    }
                }
                Verdict::Rejects(kind) => {
                    assert_eq!(rejection(row.arguments).kind(), kind, "`{line}`");
                }
            }
        }
    }

    /// The text that starts the first usage line of a message.
    const USAGE_PREFIX: &str = "Usage: ";

    /// The word of a usage line that stands for every optional flag.
    ///
    /// It names no one argument, so a filled command line leaves it out.
    const OPTIONS_PLACEHOLDER: &str = "[OPTIONS]";

    /// Reads the value that one placeholder of a message stands for.
    ///
    /// clap writes a placeholder in angle brackets, and it writes an optional
    /// element of a usage line in square brackets. Both spellings name the same
    /// argument, so this function reads either one.
    ///
    /// # Panics
    ///
    /// Panics on a placeholder the test does not know. Such a placeholder is a
    /// new argument, and it belongs in the matrix.
    fn value_of(placeholder: &str) -> &'static str {
        let name = placeholder.trim_matches(|character| matches!(character, '<' | '>' | '[' | ']'));
        match name {
            "DESTINATION" => A_DESTINATION,
            "FILE" => A_REPLAY_FILE,
            "ID" => A_RUN_ID,
            other => panic!("the test cannot supply `{other}`; add it to the matrix"),
        }
    }

    /// Writes the arguments that one fragment of a message asks for.
    ///
    /// The fragment is one name of the list of the missing arguments, such as
    /// `<FILE>` or `--run <ID>`, or one whole usage line. A word in brackets is
    /// a placeholder, and it becomes a value. Every other word is literal text
    /// of a command line, such as a flag, the name of a command, or the name of
    /// the program, and it stays as it is.
    fn arguments_of(fragment: &str) -> Vec<String> {
        fragment
            .split_whitespace()
            .filter(|word| *word != OPTIONS_PLACEHOLDER)
            .map(|word| {
                if word.starts_with('<') || word.starts_with('[') {
                    value_of(word).to_owned()
                } else {
                    word.to_owned()
                }
            })
            .collect()
    }

    /// The arguments a missing-argument rejection names.
    fn missing_arguments(error: &clap::Error) -> Vec<String> {
        match error.get(ContextKind::InvalidArg) {
            Some(ContextValue::Strings(names)) => names.clone(),
            other => panic!("the rejection names no missing argument: {other:?}"),
        }
    }

    /// The usage lines of a rejection, as the user reads them.
    ///
    /// The first line starts with `Usage: `, and clap indents every line after
    /// it to the same column. The block ends at the first empty line.
    ///
    /// # Panics
    ///
    /// Panics when the message carries no usage line. Every rejection carries
    /// one, so an empty result means clap now writes the block another way, and
    /// a test that read it would then hold nothing.
    fn usage_forms(error: &clap::Error) -> Vec<String> {
        let rendered = error.render().to_string();
        let mut forms = Vec::new();
        let mut lines = rendered.lines();
        if let Some(first) = lines.find_map(|line| line.strip_prefix(USAGE_PREFIX)) {
            forms.push(first.trim().to_owned());
            for line in lines {
                if line.trim().is_empty() {
                    break;
                }
                forms.push(line.trim().to_owned());
            }
        }
        assert!(
            !forms.is_empty(),
            "the message carries no usage line: {rendered}"
        );
        forms
    }

    /// Asserts that the parser accepts the command line the message asked for.
    fn assert_the_parser_accepts(line: &[String], what_asked: &str) {
        assert!(
            Cli::try_parse_from(line.iter()).is_ok(),
            "{what_asked} `{}`, which the parser rejects",
            line.join(" ")
        );
    }

    /// A message that asks for an argument must ask for one the parser accepts.
    ///
    /// A rejection that names a missing argument tells the user to supply it. The
    /// user obeys, and the parser must then accept the line. It does not, when
    /// the named argument conflicts with an argument the line already carries,
    /// and the user reads two messages that each send them back to the other.
    ///
    /// The user obeys two parts of such a message. The list of the missing
    /// arguments holds what to add to the line they typed. Each usage line under
    /// that list holds a whole command line of its own, so the test fills the
    /// line in and parses it as it stands. A test of the list alone reads the
    /// machine-readable half and passes on a usage line that offers an argument
    /// the parser then rejects, because the list never names that argument.
    #[test]
    fn obeying_a_missing_argument_message_gives_a_command_line_that_parses() {
        for row in &ARGUMENT_MATRIX {
            if !matches!(
                row.verdict,
                Verdict::Rejects(ErrorKind::MissingRequiredArgument)
            ) {
                continue;
            }
            let error = rejection(row.arguments);
            let typed = row.arguments.join(" ");
            let named = missing_arguments(&error);

            let mut obeyed: Vec<String> = row
                .arguments
                .iter()
                .map(|word| (*word).to_owned())
                .collect();
            for name in &named {
                obeyed.extend(arguments_of(name));
            }
            assert_the_parser_accepts(
                &obeyed,
                &format!("`{typed}` names {named:?}, and obeying that list gives"),
            );

            for form in usage_forms(&error) {
                assert_the_parser_accepts(
                    &arguments_of(&form),
                    &format!("`{typed}` offers the usage line `{form}`, and obeying it gives"),
                );
            }
        }
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
    fn a_run_shows_the_table_only_under_a_terminal_that_the_headless_flag_left_alone() {
        assert_eq!(
            display_of(false, true),
            Display::Table,
            "a run that holds a terminal and took no flag draws the live table"
        );
        assert_eq!(
            display_of(true, true),
            Display::Headless,
            "the flag takes the table off a run that does hold a terminal"
        );
        // A run whose standard output is a pipe or a file has no terminal to
        // hold, no key to read, and no screen to clear. A table there writes
        // one whole frame into that file for each round, where one line each
        // minute says the same thing in four lines an hour.
        assert_eq!(
            display_of(false, false),
            Display::Headless,
            "a run that holds no terminal draws no table"
        );
        assert_eq!(
            display_of(true, false),
            Display::Headless,
            "and the flag leaves such a run where it stands"
        );
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
"
        );
    }

    /// The block names neither the replay nor the run.
    ///
    /// `main` prints the block only when the command line names no `replay`,
    /// and `resolve` fills the run only inside a `replay`, so neither field can
    /// reach the block with a value. A replay prints the table of the run it
    /// folded in the place of the block.
    #[test]
    fn the_block_names_neither_the_replay_nor_the_run() {
        let config = resolve(&[
            "krt",
            "replay",
            "/tmp/r.jsonl",
            "--run",
            "2026-08-19T12:00:00Z",
        ]);
        let block = config.to_string();
        for absent in ["replay:", "run:", "/tmp/r.jsonl", "2026-08-19T12:00:00Z"] {
            assert!(
                !block.contains(absent),
                "the block holds no `{absent}`: {block}"
            );
        }
    }

    /// A literal address of ip version 4, for a test of the resolution.
    const AN_IPV4_ADDRESS: &str = "1.2.3.4";

    /// The loopback address of ip version 6, for a test of the resolution.
    const AN_IPV6_ADDRESS: &str = "::1";

    /// An address of ip version 6 that no machine holds, for a test of the
    /// resolution. The block `2001:db8::/32` is the block of the documents.
    const ANOTHER_IPV6_ADDRESS: &str = "2001:db8::1";

    /// A destination that holds no label, for a test of the resolution.
    const NO_LABEL: &str = "";

    /// The name that a machine reports, for a test of the host name.
    const A_HOST_NAME: &str = "tims-mac";

    /// The name that a machine reports when it holds none.
    const AN_EMPTY_HOST_NAME: &str = "";

    /// Every family of the address, for a test that walks all three.
    const EVERY_FAMILY: [AddressFamily; 3] = [
        AddressFamily::Auto,
        AddressFamily::Version4,
        AddressFamily::Version6,
    ];

    /// Builds the socket address of one literal address that a test names.
    ///
    /// Every test of the resolution names a literal address. `to_socket_addrs`
    /// reads a literal address inside the machine and asks no resolver, so
    /// every such test runs offline. A host name in the place of a literal
    /// reaches a name server, and the test then answers for the network of the
    /// machine and not for this code.
    fn socket(text: &str) -> SocketAddr {
        SocketAddr::new(address(text), RESOLVE_PORT)
    }

    /// Reads the fault of a destination that names no address to probe.
    fn resolve_failure(destination: &str, family: AddressFamily) -> ResolveError {
        resolve_target(destination, family)
            .expect_err("the destination must name no address to probe")
    }

    /// Every reason that closes a run, for the table of the closing words.
    const EVERY_END_REASON: [EndReason; 4] = [
        EndReason::Quit,
        EndReason::Duration,
        EndReason::Rounds,
        EndReason::Error,
    ];

    #[test]
    fn each_reason_that_closes_a_run_carries_its_own_words() {
        assert_eq!(stop_reason(EndReason::Quit), "the user stopped the run");
        assert_eq!(
            stop_reason(EndReason::Duration),
            "the time limit stopped the run"
        );
        assert_eq!(
            stop_reason(EndReason::Rounds),
            "the round limit stopped the run"
        );
        assert_eq!(stop_reason(EndReason::Error), "a fault stopped the run");
    }

    /// A word that two reasons share leaves a reader unable to tell them apart,
    /// so the table gives each reason its own.
    #[test]
    fn no_two_reasons_share_their_words() {
        let mut words: Vec<&str> = EVERY_END_REASON.iter().copied().map(stop_reason).collect();
        words.sort_unstable();
        let count = words.len();
        words.dedup();
        assert_eq!(words.len(), count, "each reason carries its own words");
    }

    #[test]
    fn the_closing_line_names_the_rounds_the_file_and_the_reason() {
        let outcome = Outcome {
            rounds: 3,
            reason: EndReason::Rounds,
        };
        assert_eq!(
            closing_line(&outcome, Path::new("1.2.3.4-example.com.jsonl")),
            "recorded 3 rounds  1.2.3.4-example.com.jsonl  the round limit stopped the run"
        );
    }

    /// One round keeps the singular name, as every other count of this tool
    /// does.
    #[test]
    fn the_closing_line_of_one_round_keeps_the_singular_name() {
        let outcome = Outcome {
            rounds: 1,
            reason: EndReason::Quit,
        };
        assert_eq!(
            closing_line(&outcome, Path::new("trace.jsonl")),
            "recorded 1 round  trace.jsonl  the user stopped the run"
        );
    }

    #[test]
    fn the_auto_family_takes_the_first_address_that_the_resolver_named() {
        let found = [socket(AN_IPV6_ADDRESS), socket(AN_IPV4_ADDRESS)];
        assert_eq!(
            pick_address(&found, AddressFamily::Auto),
            Some(address(AN_IPV6_ADDRESS))
        );
    }

    #[test]
    fn ip_version_4_takes_the_first_address_of_that_version() {
        let found = [socket(AN_IPV6_ADDRESS), socket(AN_IPV4_ADDRESS)];
        assert_eq!(
            pick_address(&found, AddressFamily::Version4),
            Some(address(AN_IPV4_ADDRESS))
        );
    }

    #[test]
    fn ip_version_6_takes_the_first_address_of_that_version() {
        let found = [socket(AN_IPV4_ADDRESS), socket(AN_IPV6_ADDRESS)];
        assert_eq!(
            pick_address(&found, AddressFamily::Version6),
            Some(address(AN_IPV6_ADDRESS))
        );
    }

    #[test]
    fn ip_version_4_takes_no_address_of_a_list_of_ip_version_6_only() {
        let found = [socket(AN_IPV6_ADDRESS), socket(ANOTHER_IPV6_ADDRESS)];
        assert_eq!(pick_address(&found, AddressFamily::Version4), None);
    }

    #[test]
    fn ip_version_6_takes_no_address_of_a_list_of_ip_version_4_only() {
        let found = [socket(AN_IPV4_ADDRESS), socket("5.6.7.8")];
        assert_eq!(pick_address(&found, AddressFamily::Version6), None);
    }

    #[test]
    fn every_family_takes_no_address_of_an_empty_list() {
        for family in EVERY_FAMILY {
            assert_eq!(pick_address(&[], family), None, "the `{family}` family");
        }
    }

    #[test]
    fn a_literal_address_of_ip_version_4_resolves_to_itself() {
        let target = resolve_target(AN_IPV4_ADDRESS, AddressFamily::Auto)
            .expect("a literal address must resolve");
        assert_eq!(target.arg, AN_IPV4_ADDRESS);
        assert_eq!(target.addr, address(AN_IPV4_ADDRESS));
        assert_eq!(target.family, Family::Ipv4);
    }

    #[test]
    fn a_literal_address_of_ip_version_6_resolves_to_itself() {
        let target = resolve_target(AN_IPV6_ADDRESS, AddressFamily::Auto)
            .expect("a literal address must resolve");
        assert_eq!(target.addr, address(AN_IPV6_ADDRESS));
        assert_eq!(target.family, Family::Ipv6);
    }

    #[test]
    fn a_literal_address_of_ip_version_6_resolves_under_the_flag_of_ip_version_6() {
        let target = resolve_target(ANOTHER_IPV6_ADDRESS, AddressFamily::Version6)
            .expect("a literal address must resolve");
        assert_eq!(target.addr, address(ANOTHER_IPV6_ADDRESS));
        assert_eq!(target.family, Family::Ipv6);
    }

    #[test]
    fn an_address_of_ip_version_4_names_no_address_under_the_flag_of_ip_version_6() {
        let error = resolve_failure(AN_IPV4_ADDRESS, AddressFamily::Version6);
        assert_eq!(
            error,
            ResolveError::NoAddress {
                destination: AN_IPV4_ADDRESS.to_owned(),
                family: AddressFamily::Version6,
            }
        );
        let message = error.to_string();
        assert!(
            message.contains(&AddressFamily::Version6.to_string()),
            "the message names ip version 6: {message}"
        );
        assert!(
            message.contains(AN_IPV4_ADDRESS),
            "the message names the destination: {message}"
        );
    }

    #[test]
    fn an_address_of_ip_version_6_names_no_address_under_the_flag_of_ip_version_4() {
        assert_eq!(
            resolve_failure(AN_IPV6_ADDRESS, AddressFamily::Version4),
            ResolveError::NoAddress {
                destination: AN_IPV6_ADDRESS.to_owned(),
                family: AddressFamily::Version4,
            }
        );
    }

    /// A destination that holds no label never reaches a name server, because
    /// the resolver of the machine refuses it. The test is therefore offline,
    /// as every other test of the resolution is.
    #[test]
    fn a_destination_that_holds_no_label_does_not_resolve() {
        let error = resolve_failure(NO_LABEL, AddressFamily::Auto);
        assert!(
            matches!(error, ResolveError::Lookup { .. }),
            "the resolver refused the destination: {error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains(&format!("`{NO_LABEL}`")),
            "the message names the destination: {message}"
        );
    }

    #[test]
    fn a_machine_that_reports_a_name_carries_that_name() {
        assert_eq!(host_name_or(Some(A_HOST_NAME.to_owned())), A_HOST_NAME);
    }

    #[test]
    fn a_machine_that_reports_no_name_carries_one_word() {
        assert_eq!(host_name_or(None), UNKNOWN);
    }

    /// An empty name is no name, so it takes the same word as an absent one.
    #[test]
    fn a_machine_that_reports_an_empty_name_carries_one_word() {
        assert_eq!(host_name_or(Some(AN_EMPTY_HOST_NAME.to_owned())), UNKNOWN);
    }

    #[test]
    fn a_flag_that_a_signal_set_stops_the_run() {
        let flag = AtomicBool::new(false);
        assert!(!user_stopped(&flag), "a clear flag leaves the run going");
        flag.store(true, Ordering::SeqCst);
        assert!(user_stopped(&flag), "a set flag stops the run");
    }

    /// The reason of a fault that leaves a search without any address.
    const A_SOURCE_FAULT: &str = "the network is unreachable";

    /// The note of a search that fell back to the local egress address.
    ///
    /// `source.rs` builds this line. The text of it is of no interest here:
    /// what matters is that the caller reads the same characters back.
    const A_FALLBACK_NOTE: &str = "the public address service answered with text that is not an address: away. The run records the local egress address in its place.";

    /// The unspecified address of ip version 4, as its text reads.
    const UNSPECIFIED_VERSION_4: &str = "0.0.0.0";

    /// The unspecified address of ip version 6, as its text reads.
    const UNSPECIFIED_VERSION_6: &str = "::";

    /// Builds the outcome of a search that read no address at all.
    ///
    /// The fault is a made one, so the test opens no socket and asks no
    /// service.
    fn source_fault() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::NetworkUnreachable, A_SOURCE_FAULT)
    }

    /// Reads what a search that read no address hands back for one target.
    fn failed_search_of(target: &str) -> (SourceLabel, Option<String>) {
        source_from(Err(source_fault()), address(target))
    }

    /// A search that read an address hands back that label and no warning.
    ///
    /// Without this a run that read its public address writes a warning line
    /// that names no fault, and every user reads that line on every run.
    ///
    /// The target of this test is of the other family than the label, and the
    /// result holds no part of it. A step that reads the target on this path
    /// throws the address of the search away.
    #[test]
    fn a_search_that_needed_no_fallback_hands_back_no_warning() {
        let label = SourceLabel {
            addr: address(AN_IPV4_ADDRESS),
            kind: SourceKind::Public,
        };
        let (source, warning) = source_from(
            Ok(Discovery {
                label: label.clone(),
                note: None,
            }),
            address(AN_IPV6_ADDRESS),
        );
        assert_eq!(source, label, "the label passes through whole");
        assert_eq!(
            warning, None,
            "a search that needed no fallback carries no warning"
        );
    }

    /// A search that fell back hands its note to the caller unchanged.
    ///
    /// The note names why the public service gave no address, and the search
    /// builds it. A step that rewrites the note, or that swallows it, leaves
    /// the user of a captive network with a file of local addresses and no
    /// word about why.
    #[test]
    fn a_search_that_fell_back_hands_its_note_to_the_caller_unchanged() {
        let label = SourceLabel {
            addr: address(AN_IPV4_ADDRESS),
            kind: SourceKind::Local,
        };
        let (source, warning) = source_from(
            Ok(Discovery {
                label: label.clone(),
                note: Some(A_FALLBACK_NOTE.to_owned()),
            }),
            address(AN_IPV4_ADDRESS),
        );
        assert_eq!(source, label, "the label passes through whole");
        assert_eq!(
            warning.as_deref(),
            Some(A_FALLBACK_NOTE),
            "the note reaches the caller as the search wrote it"
        );
    }

    /// A search that read no address at all records the unspecified address of
    /// the family of the target, and it names the fault.
    ///
    /// Without this the run stops, and a machine on a network with no route out
    /// records nothing at all. The address is of the family of the target,
    /// because a record of one family that carries a source of the other reads
    /// as a fault of the tool.
    #[test]
    fn a_search_that_failed_records_the_unspecified_address_of_ip_version_4_and_says_why() {
        let (source, warning) = failed_search_of(AN_IPV4_ADDRESS);
        assert_eq!(source.addr, address(UNSPECIFIED_VERSION_4));
        assert_eq!(source.kind, SourceKind::Local);
        let warning = warning.expect("a search that read no address carries a warning");
        assert!(
            warning.contains(A_SOURCE_FAULT),
            "the warning names the fault: {warning}"
        );
        assert!(
            warning.contains(SOURCE_FALLBACK),
            "the warning names what the run recorded in its place: {warning}"
        );
    }

    /// The family of the unspecified address follows the family of the target,
    /// and it never falls back to ip version 4.
    #[test]
    fn a_search_that_failed_records_the_unspecified_address_of_ip_version_6_and_says_why() {
        let (source, warning) = failed_search_of(AN_IPV6_ADDRESS);
        assert_eq!(source.addr, address(UNSPECIFIED_VERSION_6));
        assert_eq!(source.kind, SourceKind::Local);
        let warning = warning.expect("a search that read no address carries a warning");
        assert!(
            warning.contains(A_SOURCE_FAULT),
            "the warning names the fault: {warning}"
        );
        assert!(
            warning.contains(SOURCE_FALLBACK),
            "the warning names what the run recorded in its place: {warning}"
        );
    }
}
