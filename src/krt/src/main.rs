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
//! table of that path: a head that names the run, and one row for each TTL.
//! The `hunt` command looks for the longest path it can find: it draws random
//! addresses, traces a pool of them at once, scores each path, and draws
//! another address each time one of them stops. `--mine` adds one mode to that
//! hunt: after a destination sets a record, the hunt probes a few addresses
//! near it, to find whether a neighbor of that destination gives a longer path.

// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![warn(clippy::missing_docs_in_private_items)]

mod graph;
mod hunt;
mod live;
mod names;
mod record;
mod run;
mod source;
mod stats;
mod status;
#[cfg(test)]
mod testing;
mod trace;
mod ui;

use buildinfo::version_string;
use chrono::Utc;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use record::{
    EndReason, Family, HuntId, Recording, RoundRecord, RunConfig, RunId, RunRecord, SourceKind,
    SourceLabel, Target,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::IsTerminal;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::rc::Rc;
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

/// The number of destinations that must answer a hunt, when the user names
/// none.
///
/// Eight reached destinations make a hunt that measured something. The address
/// space answers at a low rate, so a default far above eight spends minutes on
/// addresses that answer nothing.
const HUNT_ROUNDS_DEFAULT: u64 = 8;

/// The number of destinations that a hunt traces before it gives up, when the
/// user names none.
///
/// The draw of a hunt never runs out, so a hunt that finds fewer answers than
/// it wants would draw forever. The number stands well above the default rounds
/// because most of the address space answers nothing.
const MAX_TARGETS_DEFAULT: u64 = 128;

/// The number of destinations that a hunt traces at one moment, when the user
/// names none.
///
/// The number matches [`HUNT_ROUNDS_DEFAULT`], so a hunt of the default rounds
/// starts every destination it needs at once. Most of the address space answers
/// nothing, and a destination that answers nothing costs the whole target
/// timeout, so the pool is what keeps a hunt from spending that time on one
/// address at a time.
const HUNT_CONCURRENCY_DEFAULT: NonZeroUsize = match NonZeroUsize::new(8) {
    Some(count) => count,
    None => NonZeroUsize::MIN,
};

/// The number of probe rounds that each destination of a hunt takes, when the
/// user names none.
///
/// A trace of one probe round gives one sample of each hop, and one sample is
/// too few to read a round-trip time from.
const PROBES_PER_ROUND_DEFAULT: u64 = 3;

/// The longest that one destination of a hunt takes, when the user names none.
///
/// The time bounds every destination, and not the quiet ones alone. It stands
/// above the time that one probe round more than the default probe rounds takes
/// at the default interval, because the last round lands past the time of the
/// rounds, and a timeout below that time cuts a destination short of its last
/// round.
const TARGET_TIMEOUT_DEFAULT: &str = "10s";

/// The lowest number of rounds that stops a run. A run of zero rounds records
/// nothing.
///
/// The type is the type that `clap` takes for the bound of a range.
const ROUNDS_LOWEST: u64 = 1;

/// The number of addresses that one mine probes, when the user names none.
const MINE_DEPTH_DEFAULT: &str = "8";

/// The length of the block that one mine stays inside, when the user names
/// none.
const MINE_PREFIX_DEFAULT: u8 = 16;

/// The shortest block that a mine stays inside.
///
/// A `/8` holds a 256th of the address space, and a draw inside a shorter block
/// is a draw of the whole internet under another name.
const MINE_PREFIX_FLOOR: u8 = 8;

/// The longest block that a mine stays inside.
///
/// A mine draws at /24 granularity, so a block below a `/24` holds no /24 to
/// draw in.
const MINE_PREFIX_CEILING: u8 = 24;

/// The number of addresses that one mine probes of any one /24, when the user
/// names none.
const MINE_PER_PREFIX_DEFAULT: &str = "2";

/// The wait between two addresses of one mine, when the user names none.
const MINE_DELAY_DEFAULT: &str = "2s";

/// The lowest cap of the destinations of a hunt. A hunt that traces no
/// destination measures nothing.
///
/// The type is the type that `clap` takes for the bound of a range.
const TARGETS_LOWEST: u64 = 1;

/// The number of spaces between the longest key of the resolved configuration
/// block and its value.
///
/// The width of the key field comes off the keys of the block that prints, so a
/// key that a later slice adds moves the whole column and never runs into its
/// own value. A constant width would hold the column at the longest key of the
/// day, and the first longer key would stand against its value with nothing to
/// say so.
const CONFIG_KEY_GAP: usize = 1;

/// The label that the derived name of a recorded file carries in the place of
/// a destination, for a hunt.
///
/// A hunt traces many destinations, so no one of them names the file. Two hunts
/// of one machine therefore write into one file, and the identifier of the hunt
/// in every `run` record is what tells their runs apart.
const HUNT_FILE_LABEL: &str = "hunt";

/// The reason of a hunt whose draw gave no address at all.
const NO_ADDRESS_TO_HUNT: &str = "the draw gave no address to trace";

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
pub(crate) const ROUND: &str = "round";

/// The name of one TTL that answered, in the status line of one round.
const HOP: &str = "hop";

/// The name of one row of a table, in the line that counts the rows which a
/// window too short left out of a frame.
const ROW: &str = "row";

/// The last field of the status line of one round that reached the target.
pub(crate) const REACHED: &str = "reached";

/// The name of the destinations that a hunt traces, in the line of its
/// indicator and in the counts of its summary.
///
/// The word stands beside a bound of many, as in `17/128 targets`, so it never
/// takes the singular.
pub(crate) const TARGETS: &str = "targets";

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
// `paris` and `dublin` each hold one flow for one round, and not for one run,
// because a UDP run of `krt` holds the source port and lets the destination
// port vary. The tracer writes the number of the round into that free port for
// both of the two modes, and it carries the number of the probe in another
// field: the UDP checksum for `paris`, and the IP header for `dublin`. A run
// that held both ports would hold one flow for the whole run, and `krt` builds
// no such direction. `trace.rs` holds the three sentences below beside the
// direction of the ports, in
// `the_multipath_help_stands_beside_the_port_direction_that_makes_it_true`.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Multipath {
    /// Let each probe take its own flow, as traceroute always did.
    Classic,
    /// Hold one flow for each round, and carry the probe number in the UDP
    /// checksum.
    Paris,
    /// Hold one flow for each round, and carry the probe number in the IP
    /// header.
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
/// keys of the terminal. The `?` key shows the list of those keys under the
/// table. A run whose standard output is a pipe or a file, and a run that
/// `--headless` asked, print one status line each minute. The `replay` command
/// reads a file that an earlier run wrote, so it takes no destination and no
/// flag of a probe. The `hunt` command looks for the longest path it can find:
/// it draws random addresses, traces a pool of them at once, scores each path,
/// and draws another address each time one of them stops.
// The help names the `?` key and no other key of the live table, because
// `live::KEYS` is the one list that says what a key does and a doc comment of
// `clap` is a string literal that reads that list at no time. A help page that
// named all five keys would be a second list of them, and the second list
// drifts. One key name opens the first list from inside the table, and
// `live::tests::the_long_help_names_the_key_that_lists_the_keys` holds that
// name and the list together.
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

    /// The multipath mode. UDP only.
    #[arg(long, value_name = "M", value_enum, default_value_t = Multipath::Classic)]
    multipath: Multipath,

    /// Force IP version 4.
    #[arg(short = '4', conflicts_with = "ipv6")]
    ipv4: bool,

    /// Force IP version 6.
    #[arg(short = '6')]
    ipv6: bool,

    /// No table and no keys. Print one status line per minute.
    #[arg(long)]
    headless: bool,

    /// Draw the Recent column as an image of the whole history. Needs a
    /// terminal that names itself and draws images.
    #[arg(long)]
    graphics: bool,

    /// Stop after this much time.
    #[arg(long, value_name = "DUR", value_parser = parse_duration)]
    duration: Option<Duration>,

    /// Stop after this many rounds. One round is one sweep of the TTLs.
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u64).range(ROUNDS_LOWEST..),
    )]
    rounds: Option<u64>,

    /// The flags that a trace and a hunt both take.
    #[command(flatten)]
    shared: SharedArgs,

    /// The command that reads recorded work in the place of a trace.
    #[command(subcommand)]
    command: Option<Command>,
}

/// The flags that a trace and a hunt both take.
///
/// A hunt probes as a trace does, so the same seven flags set the file, the
/// period of a round, the range of the TTL, the protocol, the lookups, and the
/// source. One definition serves both, because two definitions of one flag
/// drift: `--max-ttl` would take one default from a trace and another from a
/// hunt, and nothing would say so.
///
/// The flags that a hunt does not take stay on [`Cli`] alone. A hunt draws its
/// own destination, so it takes none. It draws addresses of ip version 4 alone,
/// so the two flags of the address family say nothing about it. It prints one
/// table at the end and no live table, so `--headless` says nothing either. Its
/// own `--rounds` counts destinations, so the round limit and the time limit of
/// a trace stay where they are. `--multipath` stays there as well, so a hunt
/// names no mode and always probes the classic one. The block of a hunt prints
/// that mode, so the reader sees which one the probes take.
#[derive(clap::Args, Debug, PartialEq, Eq)]
struct SharedArgs {
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

    /// Skip reverse DNS. Show addresses only.
    #[arg(long)]
    no_dns: bool,

    /// Override the source label in the derived filename. Skip the lookup of
    /// the public address.
    #[arg(long, value_name = "IP")]
    source: Option<IpAddr>,
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

    /// Hunt for the longest path. Draw a pool of addresses, trace them at
    /// once, score each one, and draw another as each one stops.
    Hunt {
        /// Stop after this many destinations answer. A destination that
        /// answers nothing costs no round.
        #[arg(
            long,
            value_name = "N",
            default_value_t = HUNT_ROUNDS_DEFAULT,
            value_parser = clap::value_parser!(u64).range(ROUNDS_LOWEST..),
        )]
        rounds: u64,

        /// Give up after tracing this many destinations, answered or not.
        #[arg(
            long,
            value_name = "N",
            default_value_t = MAX_TARGETS_DEFAULT,
            value_parser = clap::value_parser!(u64).range(TARGETS_LOWEST..),
        )]
        max_targets: u64,

        /// Trace this many destinations at one moment. A larger pool finds the
        /// paths sooner and sends more probes at once.
        #[arg(
            long,
            value_name = "N",
            default_value_t = HUNT_CONCURRENCY_DEFAULT,
            value_parser = parse_concurrency,
        )]
        concurrency: NonZeroUsize,

        /// The number of probe rounds that each destination takes. One probe
        /// round is one sweep of the TTLs.
        #[arg(
            long,
            value_name = "N",
            default_value_t = PROBES_PER_ROUND_DEFAULT,
            value_parser = clap::value_parser!(u64).range(ROUNDS_LOWEST..),
        )]
        probes_per_round: u64,

        /// The longest that one destination takes, whether it answers or not.
        #[arg(
            long,
            value_name = "DUR",
            default_value = TARGET_TIMEOUT_DEFAULT,
            value_parser = parse_duration,
        )]
        target_timeout: Duration,

        /// The seed of the draw. A hunt of one seed visits the same addresses.
        #[arg(long, value_name = "N")]
        seed: Option<u64>,

        /// Let a partial path compete for a row of the summary.
        #[arg(long)]
        include_partial: bool,

        /// Mine the address space near the longest path found so far. The
        /// caps below stay low on purpose: probes that concentrate on one
        /// network read as a horizontal scan, which trips an intrusion
        /// detection system and earns an abuse complaint to the ISP of the
        /// user.
        #[arg(long)]
        mine: bool,

        /// The number of addresses that one mine probes.
        #[arg(
            long,
            value_name = "N",
            default_value = MINE_DEPTH_DEFAULT,
            requires = "mine",
            value_parser = parse_mine_count,
        )]
        mine_depth: NonZeroUsize,

        /// The length of the block that one mine stays inside. A mine draws
        /// every address inside the block of this length that holds the first
        /// hit.
        #[arg(
            long,
            value_name = "BITS",
            default_value_t = MINE_PREFIX_DEFAULT,
            requires = "mine",
            value_parser = parse_mine_prefix,
        )]
        mine_prefix: u8,

        /// The number of addresses that one mine probes of any one /24. The cap
        /// is what keeps a mine from reading as a horizontal scan of one
        /// organization.
        #[arg(
            long,
            value_name = "N",
            default_value = MINE_PER_PREFIX_DEFAULT,
            requires = "mine",
            value_parser = parse_mine_count,
        )]
        mine_per_prefix: NonZeroUsize,

        /// The wait between two addresses of one mine.
        #[arg(
            long,
            value_name = "DUR",
            default_value = MINE_DELAY_DEFAULT,
            requires = "mine",
            value_parser = parse_duration,
        )]
        mine_delay: Duration,

        /// The flags that a trace and a hunt both take.
        #[command(flatten)]
        shared: SharedArgs,
    },
}

/// The configuration of one hunt, after the command line resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HuntConfig {
    /// The number of destinations that must answer before the hunt stops.
    rounds: u64,
    /// The number of destinations that the hunt traces before it gives up.
    max_targets: u64,
    /// The number of destinations that the hunt traces at one moment.
    concurrency: NonZeroUsize,
    /// The number of probe rounds that each destination takes.
    probes_per_round: u64,
    /// The longest that one destination takes, whether it answers or not.
    target_timeout: Duration,
    /// The seed of the draw of the addresses.
    ///
    /// The value is resolved and never a flag: a command line that named no
    /// seed takes one off the clock, and the block of the resolved
    /// configuration prints it. A reader who wants that hunt back names the
    /// number to `--seed`.
    seed: u64,
    /// True when a partial path competes for a row of the summary.
    include_partial: bool,
    /// The mine of the near space, when the user asked for one.
    mine: Option<hunt::MinePlan>,
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
    /// True when the live table draws the Recent column as an image of the
    /// whole history.
    ///
    /// The block elements draw nine of the sixty samples that the fold holds,
    /// and an image of the same nine columns draws every one of them. The
    /// answer is a switch and not a resolved value, because the run cannot read
    /// the terminal here: `graphics_of` asks the terminal at the moment the
    /// screen starts, and it answers the block elements for a terminal that
    /// draws no image, for a terminal that named itself to nobody, and for a
    /// terminal that measures no character cell.
    graphics: bool,
    /// The time that stops the run. An absent time runs until the user stops it.
    duration: Option<Duration>,
    /// The number of rounds that stops the run.
    rounds: Option<u64>,
    /// The recorded file to fold and print. The `replay` command names it.
    replay: Option<PathBuf>,
    /// The run in the recorded file to fold.
    run: Option<String>,
    /// The configuration of the hunt. A run that hunts nothing holds none.
    hunt: Option<HuntConfig>,
}

impl Cli {
    /// The seven shared flags of the side of the command line that probes.
    ///
    /// A `hunt` carries its own copy of them, because the parser rejects a flag
    /// of a probe in front of a command. Every other line carries them at the
    /// top. The two checks that read a flag before the run resolves therefore
    /// read the same values that the run will use.
    fn probe_args(&self) -> &SharedArgs {
        match &self.command {
            Some(Command::Hunt { shared, .. }) => shared,
            _ => &self.shared,
        }
    }

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
    /// than `classic` needs UDP, because no other protocol lets the mode
    /// change a packet. ICMP carries no port to hold or to vary. A TCP probe
    /// puts the probe number in its source port under every mode, so the mode
    /// changes nothing, and the record then names a mode that the run did not
    /// use. A target timeout that the probe rounds of the same hunt run past
    /// cuts every destination short of its last round, so such a line asks for
    /// two things at once. The last round lands past the time of the rounds, so
    /// the timeout must hold one probe round more than the line asks for. A cap
    /// of the targets below the rounds of the same hunt asks for two things at
    /// once as well: such a hunt gives up before it can hold the rounds it
    /// wants.
    ///
    /// Returns the reason as text as well when the destination is the name of a
    /// command, which a flag in front of that command makes. That check runs
    /// ahead of the checks of the flags, because each of those reads a flag
    /// that such a line must move.
    fn resolve(self) -> Result<ResolvedConfig, String> {
        let probe = self.probe_args();

        // This guard stands in front of every check of a flag. A line that
        // reads a command as its destination holds the flags of a trace, and
        // those flags then contradict each other as the flags of a trace do. A
        // message about one of them names a fault that goes away as soon as the
        // line writes the command first, and it sends the reader after the
        // wrong flag.
        if let Some(destination) = self.destination.as_deref() {
            if let Some(command) = command_named(destination) {
                let outside = flags_outside(&command);
                let refused = if outside.is_empty() {
                    String::new()
                } else {
                    let named: Vec<String> =
                        outside.iter().map(|flag| format!("`{flag}`")).collect();
                    format!(
                        " `{command}` takes none of these flags: {}.",
                        named.join(", ")
                    )
                };
                return Err(format!(
                    "`{destination}` is the name of a command, and this line reads it as a destination: write `{PROGRAM} {command}` first, because every flag of a probe stands behind the command.{refused}"
                ));
            }
        }

        if probe.first_ttl > probe.max_ttl {
            return Err(format!(
                "`--first-ttl {}` is above `--max-ttl {}`: the first TTL starts the probe and the max TTL ends it",
                probe.first_ttl, probe.max_ttl
            ));
        }

        let carries_a_mode = matches!(probe.protocol, Protocol::Udp);
        if self.multipath != Multipath::Classic && !carries_a_mode {
            return Err(format!(
                "`--multipath {}` needs `--protocol udp`, but the protocol is `{}`",
                value_name(&self.multipath),
                value_name(&probe.protocol)
            ));
        }

        if let Some(reason) = hunt_contradiction(self.command.as_ref(), probe.interval) {
            return Err(reason);
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

        let (replay, run) = match &self.command {
            Some(Command::Replay { file, run }) => (Some(file.clone()), run.clone()),
            Some(Command::Hunt { .. }) | None => (None, None),
        };

        let (hunt, shared) = hunt_config(self.command, self.shared);

        Ok(ResolvedConfig {
            destination: self.destination,
            output: shared.output,
            interval: shared.interval,
            first_ttl: shared.first_ttl,
            max_ttl: shared.max_ttl,
            protocol: shared.protocol,
            multipath: self.multipath,
            address_family,
            reverse_dns: !shared.no_dns,
            source: shared.source,
            headless: self.headless,
            graphics: self.graphics,
            duration: self.duration,
            rounds: self.rounds,
            replay,
            run,
            hunt,
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
        let rows = self.rows();
        let width = rows
            .iter()
            .map(|(key, _)| key.len() + ":".len())
            .max()
            .unwrap_or_default()
            + CONFIG_KEY_GAP;
        writeln!(formatter, "resolved configuration:")?;
        for (key, value) in rows {
            let key = format!("{key}:");
            writeln!(formatter, "  {key:<width$}{value}")?;
        }
        Ok(())
    }
}

impl ResolvedConfig {
    /// The rows of the block that a run prints before it probes.
    ///
    /// A hunt and a trace hold different rows, because a hunt draws its own
    /// destination, draws addresses of ip version 4 alone, and draws no live
    /// table. A block that named a destination, an address family, and a
    /// display would state three things that the hunt does not do.
    ///
    /// The multipath mode stands in both blocks, because a hunt probes with a
    /// mode as a trace does. A hunt names no mode of its own, so its block
    /// always reads `classic`, and the row is what tells the reader so.
    ///
    /// The eight rows that both of them hold read one expression each, and the
    /// two lists then name those rows in the order each block wants. A second
    /// expression for one of those rows would print the source one way in a
    /// trace and another way in a hunt.
    fn rows(&self) -> Vec<(&'static str, String)> {
        let output = || {
            self.output.as_ref().map_or_else(
                || OUTPUT_DERIVED.to_owned(),
                |path| path.display().to_string(),
            )
        };
        let interval = || ui::render_duration(self.interval);
        let reverse_dns = || if self.reverse_dns { "on" } else { "off" }.to_owned();
        let source = || {
            self.source.map_or_else(
                || SOURCE_DISCOVERED.to_owned(),
                |address| address.to_string(),
            )
        };
        let Some(hunt) = &self.hunt else {
            return vec![
                (
                    "destination",
                    self.destination
                        .clone()
                        .unwrap_or_else(|| ABSENT.to_owned()),
                ),
                ("output", output()),
                ("interval", interval()),
                ("first ttl", self.first_ttl.to_string()),
                ("max ttl", self.max_ttl.to_string()),
                ("protocol", value_name(&self.protocol)),
                ("multipath", value_name(&self.multipath)),
                ("address family", self.address_family.to_string()),
                ("reverse dns", reverse_dns()),
                ("source", source()),
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
        };
        let mut rows = vec![
            ("output", output()),
            ("interval", interval()),
            ("first ttl", self.first_ttl.to_string()),
            ("max ttl", self.max_ttl.to_string()),
            ("protocol", value_name(&self.protocol)),
            ("multipath", value_name(&self.multipath)),
            ("reverse dns", reverse_dns()),
            ("source", source()),
            ("rounds", hunt.rounds.to_string()),
            ("max targets", hunt.max_targets.to_string()),
            ("at once", hunt.concurrency.to_string()),
            ("probes per round", hunt.probes_per_round.to_string()),
            ("target timeout", ui::render_duration(hunt.target_timeout)),
            ("seed", hunt.seed.to_string()),
            (
                "include partial",
                if hunt.include_partial { "on" } else { "off" }.to_owned(),
            ),
            (
                "mine",
                if hunt.mine.is_some() { "on" } else { "off" }.to_owned(),
            ),
        ];
        // A hunt that mines nothing names no bound of a mine. Four rows of
        // numbers that nothing reads would leave a reader of the block unable
        // to tell the hunt that mines from the hunt that does not.
        if let Some(mine) = hunt.mine {
            rows.extend([
                ("mine depth", mine.depth.to_string()),
                ("mine prefix", format!("/{}", mine.prefix)),
                ("mine per prefix", mine.per_prefix.to_string()),
                ("mine delay", ui::render_duration(mine.delay)),
            ]);
        }
        rows
    }
}

/// The configuration of the hunt that a command line names, and the flags of a
/// probe that the run then reads.
///
/// A hunt carries its own copy of the seven shared flags, because the parser
/// rejects a flag of a probe in front of a command. The hunt therefore wins over
/// the flags of the line that stands in front of it, which hold their defaults
/// for a line that names a command. `outside` is that outer copy, and a line
/// that names no hunt reads it.
///
/// A hunt that named no seed takes one off the clock, so every hunt resolves to
/// a seed that the block of the configuration prints.
fn hunt_config(command: Option<Command>, outside: SharedArgs) -> (Option<HuntConfig>, SharedArgs) {
    let Some(Command::Hunt {
        rounds,
        max_targets,
        concurrency,
        probes_per_round,
        target_timeout,
        seed,
        include_partial,
        mine,
        mine_depth,
        mine_prefix,
        mine_per_prefix,
        mine_delay,
        shared,
    }) = command
    else {
        return (None, outside);
    };
    (
        Some(HuntConfig {
            rounds,
            max_targets,
            concurrency,
            probes_per_round,
            target_timeout,
            seed: seed.unwrap_or_else(seed_from_clock),
            include_partial,
            mine: mine.then_some(hunt::MinePlan {
                depth: mine_depth,
                prefix: mine_prefix,
                per_prefix: mine_per_prefix,
                delay: mine_delay,
            }),
        }),
        shared,
    )
}

/// Reads a count of the addresses that one mine probes.
///
/// A mine of no address probes nothing, so the number stands above zero.
///
/// # Errors
///
/// Returns the reason as text when the number does not read, and when it is
/// zero.
fn parse_mine_count(text: &str) -> Result<NonZeroUsize, String> {
    text.parse()
        .map_err(|_| format!("`{text}` is no count of addresses above zero"))
}

/// Reads the length of the block that one mine stays inside.
///
/// The length stands from [`MINE_PREFIX_FLOOR`] to [`MINE_PREFIX_CEILING`]. A
/// shorter block holds so much of the address space that a draw inside it is a
/// draw of the whole internet, and a longer one holds no whole /24, which is
/// the grain that a mine draws at.
///
/// # Errors
///
/// Returns the reason as text when the number does not read, and when it stands
/// outside that range.
fn parse_mine_prefix(text: &str) -> Result<u8, String> {
    let prefix: u8 = text
        .parse()
        .map_err(|_| format!("`{text}` is no length of a block"))?;
    if !(MINE_PREFIX_FLOOR..=MINE_PREFIX_CEILING).contains(&prefix) {
        return Err(format!(
            "`{text}` stands outside the block lengths that a mine draws in, which are {MINE_PREFIX_FLOOR} through {MINE_PREFIX_CEILING}: a shorter block is most of the address space, and a longer one holds no whole /24"
        ));
    }
    Ok(prefix)
}

/// Reads the number of destinations that a hunt traces at one moment.
///
/// The number stands from one to the lanes that one process holds. A pool above
/// that ceiling would put two destinations of one moment in one lane, and two
/// tracers of one lane read each other's answers, so a hop of one destination
/// would land in the path of another.
///
/// # Errors
///
/// Returns the reason as text when the number does not read, when it is zero,
/// and when it stands above the ceiling.
fn parse_concurrency(text: &str) -> Result<NonZeroUsize, String> {
    let ceiling = usize::from(trace::Lane::COUNT);
    let count: NonZeroUsize = text
        .parse()
        .map_err(|_| format!("`{text}` is not a number of destinations from 1 to {ceiling}"))?;
    if count.get() > ceiling {
        return Err(format!(
            "`{text}` is above the {ceiling} destinations that one hunt traces at one moment: two destinations of one lane read each other's answers"
        ));
    }
    Ok(count)
}

/// Reads a duration from the text of a command line flag.
///
/// The text holds a whole number and one unit, with no space between them. The
/// units are `ms` for milliseconds, `s` for seconds, `m` for minutes, and `h`
/// for hours. `500ms`, `1s`, `2m`, and `3h` are examples.
///
/// `ui::render_duration` writes the text that this function reads. The two live
/// apart because the head of the frame writes a duration as well, and one
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

/// The command that a destination names, when the destination is the name of
/// one.
///
/// A line that names a flag in front of a command reads the command as the
/// destination, because the parser takes the first free word as the
/// destination and every flag of a probe conflicts with a command. Such a line
/// would trace a host named `hunt`.
///
/// The list of the names comes from a built parser and never from a list of
/// this file. A command that a later slice adds is a command that this guard
/// covers, with nothing to remember.
///
/// The parser must be built, because clap writes the `help` command into it as
/// it builds it. A parser straight from the derive macro therefore holds the
/// commands of this file alone, and `krt --headless help` went to the network
/// and looked for a host named `help`.
fn command_named(destination: &str) -> Option<String> {
    built_parser()
        .get_subcommands()
        .map(|command| command.get_name().to_owned())
        .find(|name| name == destination)
}

/// The parser of the tool, as clap holds it once it is ready to read a command
/// line.
///
/// Clap adds the `help` command and the `--help` and `--version` flags as it
/// builds the parser, and the derive macro alone adds none of the three. A
/// reader of the parser that skips this step therefore reads a list that the
/// tool never gives a user.
fn built_parser() -> clap::Command {
    let mut parser = Cli::command();
    parser.build();
    parser
}

/// The flags that clap writes into the parser as it builds it.
///
/// A built parser carries `--help` on every command, and `--version` on the top
/// level alone, because the version stands on [`Cli`]. Neither is a flag of a
/// probe, so a message about the flags of a probe names neither. The names are
/// the ids that clap gives the two arguments.
const GENERATED_FLAGS: [&str; 2] = ["help", "version"];

/// The flags of the top level that the command does not take, as a user writes
/// them.
///
/// The guard of a command read as a destination tells the reader to write the
/// command first. That repair works for a flag the command shares, and it fails
/// for a flag that stands on the top level alone: the command then answers that
/// the flag is unknown. The message therefore names the flags of the second
/// kind.
///
/// The two sets come from a built parser and never from a list of this file. A
/// flag that a later slice moves onto [`SharedArgs`] leaves this set on its
/// own, and a flag that a later slice adds to the top level joins it.
///
/// The parser must be built, for the reason that [`command_named`] gives: the
/// `help` command exists only in a built parser, and this function finds the
/// command of a name that guard gave it. A built parser also carries the two
/// flags that clap writes itself, so the top level drops
/// [`GENERATED_FLAGS`] first. `--version` stands on the top level alone, and
/// the message would otherwise name it as a flag that the command refuses.
///
/// A flag that carries a short name and no long name — the two flags of the
/// address family — reads by its short name. A positional argument is no flag,
/// so the set holds none. The order is the order of the text, so one command
/// line always reads one message.
fn flags_outside(command: &str) -> Vec<String> {
    let top = built_parser();
    let Some(inner) = top.find_subcommand(command) else {
        return Vec::new();
    };
    let long_names: BTreeSet<&str> = inner
        .get_arguments()
        .filter_map(clap::Arg::get_long)
        .collect();
    let short_names: BTreeSet<char> = inner
        .get_arguments()
        .filter_map(clap::Arg::get_short)
        .collect();
    let mut outside: Vec<String> = top
        .get_arguments()
        .filter(|argument| !GENERATED_FLAGS.contains(&argument.get_id().as_str()))
        .filter_map(
            |argument| match (argument.get_long(), argument.get_short()) {
                (Some(long), _) if !long_names.contains(long) => Some(format!("--{long}")),
                (None, Some(short)) if !short_names.contains(&short) => Some(format!("-{short}")),
                _ => None,
            },
        )
        .collect();
    outside.sort();
    outside
}

/// What the refusal of a target timeout names in the place of a time that no
/// duration holds.
const TIME_BEYOND_A_DURATION: &str = "more time than a duration holds";

/// The time that the probe rounds of one destination of a hunt take.
///
/// The tracer sends one probe round each interval, so the destination takes
/// the interval once for each probe round. `--probes-per-round` counts up to
/// the width of a `u64`, and a duration multiplies by a `u32`, so a large count
/// gives a product that no duration holds. The answer is then none, and the
/// caller reads that as a time that no timeout reaches.
fn probe_time(interval: Duration, probes_per_round: u64) -> Option<Duration> {
    u32::try_from(probes_per_round)
        .ok()
        .and_then(|rounds| interval.checked_mul(rounds))
}

/// The reason that the flags of one hunt contradict each other, when they do.
///
/// A cap of the targets below the rounds of the same line gives up before the
/// hunt can hold the rounds it wants. A target timeout that the probe rounds of
/// the same line run past cuts every destination short of its last round. Both
/// lines ask for two things at once, so the tool names the pair in the place of
/// a hunt that can never do what the line says.
///
/// The timeout must hold one probe round more than the line asks for. The first
/// round of a destination goes out one interval after the run starts, and each
/// round after it takes one interval more, so the last of N rounds lands past N
/// intervals. A timeout of exactly N intervals therefore stops the destination
/// one round short of the count on its own command line.
///
/// A command line that names no hunt holds no such pair, and it gives none.
fn hunt_contradiction(command: Option<&Command>, interval: Duration) -> Option<String> {
    let Some(Command::Hunt {
        rounds,
        max_targets,
        probes_per_round,
        target_timeout,
        ..
    }) = command
    else {
        return None;
    };
    if max_targets < rounds {
        return Some(format!(
            "`--max-targets {max_targets}` is below `--rounds {rounds}`: a hunt that traces {max_targets} destinations never holds {rounds} that answered"
        ));
    }
    // The saturating add holds a count of the full width of a `u64`. The
    // `u32::try_from` of `probe_time` then gives none, which reads as a time
    // that no timeout reaches.
    let needed = probe_time(interval, probes_per_round.saturating_add(1));
    if needed.is_none_or(|needed| *target_timeout <= needed) {
        let needs = needed.map_or_else(|| TIME_BEYOND_A_DURATION.to_owned(), ui::render_duration);
        return Some(format!(
            "`--target-timeout {}` cannot hold `--probes-per-round {probes_per_round}` at an interval of {}. The last round lands past the time of the rounds, so the timeout must hold one round more, which is {needs}: raise the timeout or lower the probe rounds",
            ui::render_duration(*target_timeout),
            ui::render_duration(interval)
        ));
    }
    None
}

/// The seed of a hunt that named none.
///
/// The moment that the hunt starts makes the seed, so two hunts of one machine
/// draw different addresses. The block of the resolved configuration prints the
/// number, so a reader who wants that hunt back names it to `--seed`.
///
/// The nanoseconds of the moment run past the width of the seed, and the low
/// bits are the ones that move, so the seed takes those.
fn seed_from_clock() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    u64::try_from(nanos & u128::from(u64::MAX)).unwrap_or_default()
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
pub(crate) fn counted(count: usize, name: &str) -> String {
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
                lines: frame.lines(width, ui::Paint::Plain),
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

/// The fault that stopped a hunt, and what the hunt found before it.
///
/// A fault that stops the loop of the hunt carries the summary of the rounds
/// that finished, and the caller prints that table before it prints the reason.
/// A fault in front of the loop — the privilege gate, the resolver, the draw,
/// the file, the signal — carries none, because no round ran.
struct HuntFailure {
    /// The summary of the rounds that finished, when the hunt reached the loop.
    ///
    /// The summary stands behind a box, so the fault that a hunt carries in its
    /// `Result` is the width of a pointer and not the width of a whole summary.
    summary: Option<Box<hunt::Summary>>,
    /// The fault, as the user reads it, and the code of its kind.
    failure: TraceFailure,
}

impl From<TraceFailure> for HuntFailure {
    /// A fault in front of the loop of the hunt, which found nothing to print.
    fn from(failure: TraceFailure) -> Self {
        Self {
            summary: None,
            failure,
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
///
/// The time limit of a run caps this grace, and the deadline of a destination
/// of a hunt caps it the same way. A run therefore takes the time that its
/// limit names, and no more.
///
/// A run waits for its names one step at a time, one step to a turn. A
/// destination of a hunt that waits here therefore holds up no other
/// destination of the pool, whatever the length of this grace. It holds its
/// own lane for the wait, so the hunt starts the destination that follows it
/// one wait later than it otherwise would.
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

/// The pixel size of one character cell, when the run draws the Recent column
/// as an image of the whole history.
///
/// Four questions stand between a run and that image, and the answer is the
/// size of a cell only when all four of them answer yes.
///
/// The reader asked for it. The flag is off by default, because the block
/// elements are one picture of a hop and an image is a second one, and the
/// documentation of [`crate::ui`] argues that two readers must never argue over
/// a hop. A terminal that draws the image over a cell that also holds a bar
/// gives a reader two pictures of one history.
///
/// The run draws the live table at all. A replay, a headless run, a run whose
/// standard output is a pipe, and a run whose standard output is a file each
/// draw the block elements. This question needs no test of its own: the image
/// path lives in [`crate::live`], and [`screen_of`] reaches
/// [`crate::live::Table`] only when [`display_of`] answers [`Display::Table`].
///
/// The terminal draws images, and it named itself as well. Three inline-image
/// protocols are in service, no terminal reads all three, and a terminal that
/// reads none of them puts the escape sequence of an image on the screen as
/// text. No terminal answers a question about the protocols it reads, so
/// `termgfx` names the terminal from the environment variables that the
/// terminal set and picks the protocol of that name. A terminal that set none
/// of those variables carries no name, so the protocol it gets is a guess, and
/// a guessed protocol is the same escape sequence on the screen as text.
/// `Capabilities::draws_images_by_name` is the answer that refuses the guess.
///
/// The terminal reports a pixel size. A terminal that answers the `TIOCGWINSZ`
/// ioctl with zero pixels reports no pixel size, exactly as `termsize` argues
/// that a terminal with no window reports no width. The image path takes no
/// guess there: an image at a guessed size stands over the wrong cells, and the
/// block elements say the same thing at the size the terminal already agreed
/// to.
///
/// The reads of the world stand apart from this decision, so a test names the
/// answers without a terminal to name them with.
fn graphics_of(
    asked: bool,
    draws_images_by_name: bool,
    cell: Option<(u32, u32)>,
) -> Option<(u32, u32)> {
    if !asked || !draws_images_by_name {
        return None;
    }
    cell
}

/// Whether the frames of a live table carry the color of a terminal, from
/// whether the reader set `NO_COLOR`.
///
/// A reader who sets that variable asks every tool for the glyphs alone, and
/// the one color of this table is the red of a lost probe. The mark of that
/// probe stays, because the mark is no bar of a time, and the codes that paint
/// it red go away.
///
/// The read of the environment stands apart from this decision, so a test names
/// the answer of a reader without a variable to name it with.
fn paint_of(no_color: bool) -> ui::Paint {
    if no_color {
        return ui::Paint::Plain;
    }
    ui::Paint::Colored
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
    // A trace starts one tracer, so nothing waits for its thread. The thread
    // stops at the round limit of the run, and a run of no round limit stops
    // when the process does.
    let (rounds, _tracer) = trace::spawn(&trace::TraceConfig {
        target: target.addr,
        run: run.clone(),
        interval: config.interval,
        first_ttl: config.first_ttl,
        max_ttl: config.max_ttl,
        protocol: config.protocol,
        multipath: config.multipath,
        privilege,
        // A trace of one destination probes in the first lane. The lanes
        // beyond it are for a hunt, which traces many destinations at once.
        lane: trace::Lane::FIRST,
        // The run loop stops at this number too, so the tracer sends no probe
        // behind the run that asked for it.
        rounds: config.rounds,
    })
    .map_err(|error| TraceFailure::new(&error, EXIT_TRACER_FAILED))?;

    let start = RunRecord {
        run,
        krt: version_string!().to_owned(),
        source,
        target,
        config: run_config(config, privilege),
        host: host_name(),
        hunt: None,
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
    let namer = names::Namer::new(resolver, start.run.clone());

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
            rounds,
            limits,
            &|| user_stopped(&flag),
            namer,
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

/// The tracer of a hunt of the command line.
///
/// Each destination takes one tracer of `trace.rs`, and the hunt holds many of
/// them at once. Every one of those tracers probes in a lane of its own, which
/// is what keeps two of them from reading each other's answers and what keeps
/// two UDP tracers from binding one source port.
///
/// Two tracers of one lane still cannot probe at once, so `start` waits for the
/// tracer that ran last in the lane it is handed. The wait is what the run loop
/// cannot give: a destination that stops at its target timeout stops while its
/// tracer still probes. The wait ends at the round limit of that destination,
/// which the command line holds under the target timeout of every destination.
struct SystemProbes<'a> {
    /// The command line that the hunt resolved, which holds the period, the
    /// range of the TTL, the protocol, and the multipath mode of every probe.
    config: &'a ResolvedConfig,
    /// The privilege mode that the platform gave.
    privilege: record::Privilege,
    /// The thread of the tracer that ran last in each lane.
    ///
    /// A lane whose first destination the hunt has not started yet holds no
    /// such thread.
    running: Vec<Option<trace::TracerThread>>,
}

impl SystemProbes<'_> {
    /// The configuration of the tracer of one destination.
    ///
    /// The round limit is the number of probe rounds that each destination of
    /// the hunt takes, so the tracer of a destination stops when that
    /// destination stops. `hunt::trace_one` gives the run loop the same number.
    ///
    /// A `SystemProbes` traces for a hunt alone, so the resolved command line
    /// always holds a plan. A line that holds none leaves the tracer without a
    /// round limit, which is what a trace of one destination takes.
    ///
    /// The build of the configuration stands apart from the start of the
    /// tracer, because a tracer that starts sends packets and a test of this
    /// wiring sends none.
    fn config_of(&self, target: Ipv4Addr, run: &RunId, lane: trace::Lane) -> trace::TraceConfig {
        trace::TraceConfig {
            target: IpAddr::V4(target),
            run: run.clone(),
            interval: self.config.interval,
            first_ttl: self.config.first_ttl,
            max_ttl: self.config.max_ttl,
            protocol: self.config.protocol,
            multipath: self.config.multipath,
            privilege: self.privilege,
            lane,
            rounds: self.config.hunt.map(|hunt| hunt.probes_per_round),
        }
    }
}

impl SystemProbes<'_> {
    /// The thread of the tracer that ran last in this lane.
    ///
    /// A lane past the end of the list holds no thread, and the list grows to
    /// reach it. The hunt builds the list at the size of its pool, so the growth
    /// is the guard behind that size and never the normal case.
    fn thread_of(&mut self, lane: trace::Lane) -> &mut Option<trace::TracerThread> {
        let place = lane.place();
        if self.running.len() <= place {
            self.running.resize_with(place + 1, || None);
        }
        &mut self.running[place]
    }
}

impl hunt::Probes for SystemProbes<'_> {
    fn start(
        &mut self,
        target: Ipv4Addr,
        run: &RunId,
        lane: trace::Lane,
    ) -> Result<std::sync::mpsc::Receiver<RoundRecord>, String> {
        // The tracer that ran last in this lane stops first. Two tracers of one
        // lane carry one probe identifier and one source port, so the second
        // one would read the answers of the first.
        if let Some(running) = self.thread_of(lane).take() {
            running.wait();
        }
        let (rounds, running) =
            trace::spawn(&self.config_of(target, run, lane)).map_err(|error| error.to_string())?;
        *self.thread_of(lane) = Some(running);
        Ok(rounds)
    }
}

/// Records one hunt, from the draw of the addresses to the summary of them all.
///
/// The order of the steps is the order of a trace, and for the same reasons.
/// The privilege gate comes first, so a platform that cannot probe says so
/// before the hunt makes a file. The reverse resolver starts next, so a hunt
/// that cannot start its resolver makes no file. The recorded file opens before
/// the first tracer starts, so no probe leaves the machine for a hunt that
/// cannot record it.
///
/// The search for the source address reads the route to one destination, and
/// the first address of the draw is that destination. The draw keeps that
/// address, so the hunt traces it as its first round and the search costs the
/// hunt no round. A draw that gives no address at all leaves the hunt nothing
/// to trace, and the run says so.
///
/// The hunt draws no live table. It prints the summary when it stops, and a
/// hunt that `Ctrl-C` stopped prints the summary of the rounds that finished.
///
/// # Errors
///
/// Returns the reason and the exit code of the fault that stopped the hunt,
/// with the summary of the rounds that finished beside them. A fault that
/// stopped the loop of the hunt holds that summary, and a fault in front of the
/// loop holds none.
fn hunt(config: &ResolvedConfig, plan: &HuntConfig) -> Result<hunt::Summary, HuntFailure> {
    let privilege = trace::acquire_privilege()
        .map_err(|error| TraceFailure::new(&error, EXIT_NO_PRIVILEGES))?;
    let resolver = resolver_of(config.reverse_dns).map_err(|error| {
        TraceFailure::new(
            &format!("the reverse resolver did not start: {error}"),
            EXIT_FAILURE,
        )
    })?;

    let mut draw = hunt::Draw::seeded(plan.seed);
    if let Some(mine) = plan.mine {
        draw = draw.mining(mine, plan.seed, Box::new(live::SystemClock));
    }
    let first = draw
        .peek()
        .ok_or_else(|| TraceFailure::new(&NO_ADDRESS_TO_HUNT.to_owned(), EXIT_FAILURE))?;
    let (source, warning) = source_from(
        source::discover(config.source, IpAddr::V4(first)),
        IpAddr::V4(first),
    );
    if let Some(warning) = warning {
        eprintln!("{PROGRAM}: {warning}");
    }
    let path = source::output_path(config.output.as_deref(), source.addr, HUNT_FILE_LABEL);
    let mut writer = record::Writer::append(&path).map_err(|error| {
        TraceFailure::new(&format!("{}: {error}", path.display()), EXIT_WRITE_FAILED)
    })?;
    println!("{RECORDING_TO} {}", path.display());

    let flag = stop_flag().map_err(|error| {
        TraceFailure::new(
            &format!("the stop signal did not register: {error}"),
            EXIT_FAILURE,
        )
    })?;
    let mut probes = SystemProbes {
        config,
        privilege,
        running: (0..plan.concurrency.get()).map(|_| None).collect(),
    };
    let facts = hunt::Facts {
        id: HuntId::at(Utc::now()),
        krt: version_string!().to_owned(),
        source,
        config: run_config(config, privilege),
        host: host_name(),
    };
    // One value carries both bounds, so the loop of the hunt and the indicator
    // that shows it can hold no numbers that disagree.
    let bounds = hunt::Bounds {
        rounds: plan.rounds,
        max_targets: plan.max_targets,
    };
    let hunt_plan = hunt::Plan {
        bounds,
        concurrency: plan.concurrency,
        probes_per_round: plan.probes_per_round,
        target_timeout: plan.target_timeout,
        name_grace: name_grace(),
        include_partial: plan.include_partial,
    };
    let mut sources = hunt::Sources {
        draw,
        probes: &mut probes,
        resolver: Rc::from(resolver),
    };
    // The indicator shows what the hunt is doing while it runs. A hunt takes
    // minutes and draws no live table, so a hunt without one prints nothing
    // between the line above and the summary at the end.
    let mut status = status::Indicator::new(
        status::style_of(std::io::stdout().is_terminal()),
        bounds,
        ui::frame_columns(),
        std::io::stdout(),
        live::SystemClock,
    );
    let summary = hunt::record(
        &facts,
        &hunt_plan,
        &mut sources,
        &|| user_stopped(&flag),
        &mut writer,
        &mut status,
    )
    .map_err(|stopped| {
        let code = match stopped.fault {
            hunt::HuntError::Run(run::RunError::Write(_)) => EXIT_WRITE_FAILED,
            hunt::HuntError::Run(run::RunError::Tracer { .. }) | hunt::HuntError::Tracer { .. } => {
                EXIT_TRACER_FAILED
            }
        };
        HuntFailure {
            summary: Some(stopped.summary),
            failure: TraceFailure::new(&stopped.fault, code),
        }
    })?;
    println!("{RECORDED} {}", path.display());
    Ok(summary)
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
/// The table takes the columns and the rows of that terminal one time, at the
/// start of the run. A window that changes size while the run stands leaves the
/// frame at the size it started with.
///
/// The read of `NO_COLOR` stands here, beside that read of the terminal, and
/// [`paint_of`] holds the decision it feeds. Any value of the variable counts,
/// as the convention of it says.
///
/// The reads that the image path needs stand here as well, for the same reason:
/// which terminal this is, whether that terminal draws an image at all, and how
/// many pixels one character cell of it holds. [`graphics_of`] holds the
/// decision that the three of them feed. The reads come after the guard took
/// the terminal, so they measure the alternate screen that the frames draw on.
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
    let (columns, rows) = ui::frame_size();
    let capabilities = termgfx::Capabilities::detect();
    let cell = graphics_of(
        config.graphics,
        capabilities.draws_images_by_name(),
        termgfx::cell_pixels(),
    );
    let table = live::Table::new(
        facts,
        std::io::stdout(),
        live::Keyboard,
        live::Window::new(columns, rows),
        live::Look {
            paint: paint_of(std::env::var_os("NO_COLOR").is_some()),
            graphics: cell.map(|cell| live::Graphics { capabilities, cell }),
        },
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
    if let Some(plan) = config.hunt {
        // The block names what the hunt will do, and then the hunt does it.
        print!("{config}");
        // The table of the rounds that finished prints either way, and the
        // reason that stopped the hunt follows it. A fault at the fifth round
        // of eight took nothing away from the four rounds in front of it.
        let (summary, failure) = match hunt(&config, &plan) {
            Ok(summary) => (Some(Box::new(summary)), None),
            Err(stopped) => (stopped.summary, Some(stopped.failure)),
        };
        if let Some(summary) = summary {
            for line in summary.lines() {
                println!("{line}");
            }
        }
        if let Some(failure) = failure {
            eprintln!("{PROGRAM}: {}", failure.reason);
            std::process::exit(failure.code);
        }
        return;
    }
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
        closing_line, display_of, graphics_of, host_name_or, name_grace, paint_of, parse_duration,
        pick_address, replay, resolve_target, run_config, source_from, stop_reason, user_stopped,
        value_name, AddressFamily, Cli, Command, Display, EndReason, Family, HuntConfig, Multipath,
        Protocol, ResolveError, ResolvedConfig, SourceKind, SourceLabel, SystemProbes, Target,
        HUNT_CONCURRENCY_DEFAULT, HUNT_ROUNDS_DEFAULT, MINE_PREFIX_CEILING, MINE_PREFIX_FLOOR,
        PROBES_PER_ROUND_DEFAULT, RESOLVE_PORT, SOURCE_FALLBACK, TARGET_TIMEOUT_DEFAULT,
        TIME_BEYOND_A_DURATION, UNKNOWN,
    };
    use crate::record::{
        Hop, Privilege, Record, RoundRecord, RunConfig, RunId, RunRecord, TtlRange, Writer,
    };
    use crate::run::Outcome;
    use crate::source::Discovery;
    use crate::testing::{address, SecondRunBetweenWrites};
    use crate::ui::{render_duration, Paint};
    use chrono::{DateTime, Utc};
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    use clap::{CommandFactory, Parser, ValueEnum};
    use std::collections::HashSet;
    use std::fs::OpenOptions;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
        assert_eq!(cli.shared.output, None);
        assert_eq!(cli.shared.interval, Duration::from_secs(1));
        assert_eq!(cli.shared.first_ttl, 1);
        assert_eq!(cli.shared.max_ttl, 30);
        assert_eq!(cli.shared.protocol, Protocol::Icmp);
        assert_eq!(cli.multipath, Multipath::Classic);
        assert!(!cli.ipv4, "the address family is automatic by default");
        assert!(!cli.ipv6, "the address family is automatic by default");
        assert!(!cli.shared.no_dns, "reverse DNS is on by default");
        assert_eq!(cli.shared.source, None);
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
            assert_eq!(cli.shared.protocol, expected, "`--protocol {text}`");
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
        assert_eq!(cli.shared.interval, Duration::from_millis(500));
    }

    #[test]
    fn parses_an_interval_in_minutes() {
        let cli = parse(&["krt", "example.com", "--interval", "2m"]);
        assert_eq!(cli.shared.interval, Duration::from_mins(2));
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
        assert_eq!(cli.shared.max_ttl, 255);
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
        assert_eq!(short.shared.output, Some(PathBuf::from("path.jsonl")));
        assert_eq!(short.shared.interval, Duration::from_millis(500));
        assert!(short.ipv4, "`-4` is the only form of the flag");
        assert_eq!(short.shared.output, long.shared.output);
        assert_eq!(short.shared.interval, long.shared.interval);
    }

    #[test]
    fn parses_a_source_of_ip_version_4() {
        let cli = parse(&["krt", "example.com", "--source", "1.2.3.4"]);
        assert_eq!(
            cli.shared.source,
            Some(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)))
        );
    }

    #[test]
    fn parses_a_source_of_ip_version_6() {
        let cli = parse(&["krt", "example.com", "--source", "::1"]);
        assert_eq!(cli.shared.source, Some(IpAddr::V6(Ipv6Addr::LOCALHOST)));
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
        let cli = parse(&["krt", "example.com", "--no-dns", "--headless", "--graphics"]);
        assert!(cli.shared.no_dns);
        assert!(cli.headless);
        assert!(cli.graphics);
    }

    #[test]
    fn the_graphics_flag_resolves_to_the_image_of_the_recent_column() {
        // The flag is the first of the four questions that `graphics_of` asks.
        // A configuration that dropped it would draw the block elements on
        // every terminal, and no line of the run would say why.
        let config = resolve(&["krt", "example.com", "--graphics"]);
        assert!(
            config.graphics,
            "the run that asked for the image carries that answer into its screen"
        );
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

    /// The pixel size of one character cell that the terminal of every test of
    /// the gate reports.
    ///
    /// Ten pixels by twenty is about the cell of a modern terminal at its
    /// default font. The numbers say nothing about the gate, which hands the
    /// size of a cell back or hands nothing back, so one pair of numbers serves
    /// every test below.
    const CELL: (u32, u32) = (10, 20);

    #[test]
    fn a_run_that_asked_for_the_image_on_a_terminal_that_draws_one_takes_the_size_of_a_cell() {
        // The image of the Recent column draws in pixels, and the terminal lays
        // the table out in cells, so the run needs the size of one cell before
        // it draws one pixel.
        assert_eq!(
            graphics_of(true, true, Some(CELL)),
            Some(CELL),
            "a reader who asked for the image on a terminal that reports a pixel size gets it"
        );
    }

    #[test]
    fn a_run_that_asked_for_no_image_draws_the_block_elements() {
        // The flag is off by default. The block elements are one picture of a
        // hop and an image is a second one, and two pictures of one hop is what
        // the table must never show.
        assert_eq!(
            graphics_of(false, true, Some(CELL)),
            None,
            "a terminal that draws images draws no image for a run that asked for none"
        );
    }

    #[test]
    fn a_terminal_that_draws_no_image_draws_the_block_elements() {
        // Every inline-image protocol carries its image in an escape sequence,
        // and a terminal that reads none of them puts that sequence on the
        // screen as text. A terminal that named itself to nobody stands here
        // too: `termgfx` guesses the protocol of such a terminal, and a guessed
        // protocol is the same sequence on the screen as text.
        assert_eq!(
            graphics_of(true, false, Some(CELL)),
            None,
            "the flag draws no image on a terminal that reads no image protocol, and none on a terminal that named none"
        );
    }

    #[test]
    fn a_terminal_that_reports_no_pixel_size_draws_the_block_elements() {
        // A terminal that answers the `TIOCGWINSZ` ioctl with zero pixels
        // reports no pixel size. The image path takes no guess there, because an
        // image at a guessed size stands over the wrong cells of the table.
        assert_eq!(
            graphics_of(true, true, None),
            None,
            "the flag draws no image on a terminal that measures no cell"
        );
    }

    #[test]
    fn a_run_paints_the_table_only_for_a_reader_who_set_no_color_on_nothing() {
        // The one color of the table is the red of a lost probe. A reader who
        // set `NO_COLOR` asks every tool for the glyphs alone, so the table of
        // such a reader carries no code of a color.
        assert_eq!(
            paint_of(true),
            Paint::Plain,
            "a reader who set NO_COLOR gets the table with glyphs alone"
        );
        assert_eq!(
            paint_of(false),
            Paint::Colored,
            "and a reader who set nothing gets the mark of a lost probe in red"
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
        assert!(
            !config.graphics,
            "the block elements of the Recent column are on by default"
        );
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
    fn rejects_a_multipath_mode_beside_tcp() {
        let message = contradiction(&[
            "krt",
            "example.com",
            "--multipath",
            "dublin",
            "--protocol",
            "tcp",
        ]);
        for part in ["--multipath", "dublin", "--protocol", "tcp"] {
            assert!(
                message.contains(part),
                "the message names `{part}`: {message}"
            );
        }
    }

    /// The help of one flag of the command line, by the id of that flag. The
    /// long help stands when the flag carries one, and the short help stands
    /// otherwise.
    fn flag_help(id: &str) -> String {
        let command = Cli::command();
        let argument = command
            .get_arguments()
            .find(|argument| argument.get_id() == id)
            .expect("the command line carries the flag");
        argument
            .get_long_help()
            .or_else(|| argument.get_help())
            .expect("the flag carries help")
            .to_string()
    }

    #[test]
    fn the_help_of_the_multipath_flag_names_every_protocol_that_carries_a_mode() {
        let help = flag_help("multipath");
        for protocol in Protocol::value_variants() {
            let name = value_name(protocol);
            let accepted = parse(&[
                "krt",
                "example.com",
                "--multipath",
                "paris",
                "--protocol",
                name.as_str(),
            ])
            .resolve()
            .is_ok();
            let in_the_help = name.to_uppercase();
            assert_eq!(
                help.contains(&in_the_help),
                accepted,
                "the help names `{in_the_help}` when `--multipath paris` takes `--protocol {name}`: {help}"
            );
        }
    }

    #[test]
    fn a_multipath_mode_other_than_classic_resolves_with_udp() {
        for (mode, expected) in [("paris", Multipath::Paris), ("dublin", Multipath::Dublin)] {
            let config = resolve(&[
                "krt",
                "example.com",
                "--multipath",
                mode,
                "--protocol",
                "udp",
            ]);
            assert_eq!(
                config.multipath, expected,
                "`--multipath {mode} --protocol udp`"
            );
        }
    }

    /// The refusal of a mode other than `classic` reaches UDP alone, so
    /// `classic` is what keeps every protocol of the command line usable.
    ///
    /// This test therefore resolves `classic` beside each protocol in turn,
    /// and it is the one test that asks `resolve` to accept `--protocol tcp`
    /// and `--protocol udp` at all.
    #[test]
    fn the_classic_multipath_mode_resolves_with_every_protocol() {
        for protocol in Protocol::value_variants() {
            let name = value_name(protocol);
            let config = resolve(&[
                "krt",
                "example.com",
                "--multipath",
                "classic",
                "--protocol",
                name.as_str(),
            ]);
            assert_eq!(
                config.multipath,
                Multipath::Classic,
                "`--multipath classic --protocol {name}`"
            );
            assert_eq!(
                config.protocol, *protocol,
                "`--multipath classic --protocol {name}`"
            );
        }
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

    /// Builds a path under the temporary directory that no other run reaches.
    ///
    /// Two runs of one test can overlap, because `cargo test` runs on many
    /// threads and more than one `cargo test` can run at once. The process
    /// identifier and the nanosecond keep the two runs apart.
    fn temp_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock must stand after the epoch")
            .as_nanos();
        let process = std::process::id();
        std::env::temp_dir().join(format!("krt-{label}-{process}-{nanos}.jsonl"))
    }

    /// A file that one test makes. The file goes away when the test ends, and
    /// also when the test panics.
    struct TempFile {
        /// The path of the file.
        path: PathBuf,
    }

    impl TempFile {
        /// Holds a path that no file uses yet, and that no other run reaches.
        ///
        /// The file that a test makes at the path goes away with this value,
        /// and a path that stays empty is no fault.
        fn absent(label: &str) -> Self {
            Self {
                path: temp_path(label),
            }
        }

        /// The path of the file.
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// The number of terminal columns that a replay of a shared file folds
    /// into.
    ///
    /// The test names the width, so the frame that it reads never depends on
    /// the terminal that ran the test. The width holds every column of the
    /// table and the whole of an address, so no row cuts the hop it names.
    const SHARED_REPLAY_COLUMNS: u16 = 200;

    /// The identifier of the first of two runs that share one file.
    const A_SHARED_RUN: &str = "2026-08-18T12:00:00.000Z";

    /// The identifier of the second of those two runs.
    const THE_OTHER_SHARED_RUN: &str = "2026-08-18T12:00:30.000Z";

    /// The first two numbers of every address of the first shared run.
    ///
    /// The two runs probe two different paths, so a reader of a frame tells
    /// the hops of one run from the hops of the other.
    const A_SHARED_NETWORK: &str = "10.0";

    /// The first two numbers of every address of the second shared run.
    const THE_OTHER_SHARED_NETWORK: &str = "172.16";

    /// The last TTL that each shared run probes.
    ///
    /// The tracer takes no TTL above this one. A round that names a hop at
    /// every one of those TTLs writes a line of more than 17000 bytes, which
    /// is far past the 8192 bytes that the defect first showed itself at.
    const SHARED_LAST_TTL: u8 = 254;

    /// The number of rounds that each shared run records.
    const SHARED_ROUNDS: u64 = 4;

    /// The round trip time of every hop of a shared run.
    const A_SHARED_RTT: f64 = 1.23;

    /// The name of the ICMP message that a hop below the target answers with.
    const A_TIME_EXCEEDED: &str = "time_exceeded";

    /// The name of the ICMP message that the target answers with.
    const AN_ECHO_REPLY: &str = "echo_reply";

    /// The build string that each shared run records.
    const A_SHARED_BUILD: &str = "0.1.0 (abc1234, clean)";

    /// The name of the machine that made both shared runs.
    const A_SHARED_MACHINE: &str = "tims-mac";

    /// Reads a moment that a test names, and converts it to UTC.
    fn moment(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("the test moment must parse")
            .with_timezone(&Utc)
    }

    /// The address of the hop of one shared run at one TTL.
    fn shared_hop(network: &str, ttl: u8) -> IpAddr {
        address(&format!("{network}.1.{ttl}"))
    }

    /// The address of every hop of one shared run, in TTL order.
    fn shared_hops(network: &str) -> Vec<String> {
        (1..=SHARED_LAST_TTL)
            .map(|ttl| shared_hop(network, ttl).to_string())
            .collect()
    }

    /// Every word that the lines of a frame hold.
    ///
    /// The table writes one address in one column, and it puts a space on each
    /// side of that column, so a word of a frame is a whole address. A test
    /// that joins the lines and searches the text for an address finds a
    /// longer address as well, because `10.0.1.1` is part of `10.0.1.10` and
    /// part of `10.0.1.100`. A test that reads these words finds the address
    /// that the frame names, and no other one.
    fn words_of(lines: &[String]) -> HashSet<&str> {
        lines
            .iter()
            .flat_map(|line| line.split_whitespace())
            .collect()
    }

    /// The record that opens one shared run.
    ///
    /// The last hop of the path is the target, as a run that reached its
    /// target records it.
    fn a_shared_run_record(run: &str, network: &str) -> Record {
        let target = shared_hop(network, SHARED_LAST_TTL);
        Record::Run(RunRecord {
            run: RunId::from(run),
            krt: A_SHARED_BUILD.to_owned(),
            source: SourceLabel {
                addr: address(&format!("{network}.0.1")),
                kind: SourceKind::Local,
            },
            target: Target {
                arg: target.to_string(),
                addr: target,
                family: Family::Ipv4,
            },
            config: RunConfig {
                interval_ms: 1000,
                protocol: Protocol::Icmp,
                first_ttl: 1,
                max_ttl: SHARED_LAST_TTL,
                multipath: Multipath::Classic,
                privilege: Privilege::Unprivileged,
                dns: false,
            },
            host: A_SHARED_MACHINE.to_owned(),
            hunt: None,
        })
    }

    /// One round of one shared run. Every TTL of the path answered.
    fn a_shared_round_record(run: &str, network: &str, seq: u64) -> Record {
        let hops = (1..=SHARED_LAST_TTL)
            .map(|ttl| Hop {
                ttl,
                addr: shared_hop(network, ttl),
                rtt_ms: A_SHARED_RTT,
                icmp: if ttl == SHARED_LAST_TTL {
                    AN_ECHO_REPLY
                } else {
                    A_TIME_EXCEEDED
                }
                .to_owned(),
            })
            .collect();
        Record::Round(RoundRecord {
            run: RunId::from(run),
            seq,
            ts: moment("2026-08-18T12:00:01.000Z"),
            dur_ms: 1000,
            ttl_range: TtlRange::new(1, SHARED_LAST_TTL).expect("the test range must hold"),
            reached: true,
            hops,
        })
    }

    /// Every record that one shared run writes: the record that opens the run,
    /// and one record for each round that the run made.
    fn the_records_of(run: &str, network: &str) -> Vec<Record> {
        let mut records = vec![a_shared_run_record(run, network)];
        records.extend((1..=SHARED_ROUNDS).map(|seq| a_shared_round_record(run, network, seq)));
        records
    }

    /// Two runs of one destination from one machine append to one file at one
    /// moment, and a replay of that file must report the path of each of them.
    ///
    /// A record that leaves the writer as two writes releases the lock of the
    /// file in the middle of itself, and the second run then appends a whole
    /// record into the gap. One line of the file holds two records after that,
    /// the reader refuses the whole file, and each replay reports the fault in
    /// the place of a path.
    ///
    /// The second run appends after every write of the first one, so the test
    /// reads that meeting every time.
    #[test]
    fn a_replay_of_a_file_that_two_runs_wrote_at_one_moment_reports_the_path_of_each_run() {
        let file = TempFile::absent("replay-of-a-shared-file");
        let first = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file.path())
            .expect("the test file must open for appending");
        let sink = SecondRunBetweenWrites::on(
            first,
            file.path(),
            the_records_of(THE_OTHER_SHARED_RUN, THE_OTHER_SHARED_NETWORK),
        )
        .expect("the second run must open the same file");
        let mut writer = Writer::to_sink(sink);
        for record in the_records_of(A_SHARED_RUN, A_SHARED_NETWORK) {
            writer.write(&record).expect("the record must be written");
        }
        drop(writer);

        let ours = shared_hops(A_SHARED_NETWORK);
        let theirs = shared_hops(THE_OTHER_SHARED_NETWORK);
        for (run, held, absent) in [
            (A_SHARED_RUN, &ours, &theirs),
            (THE_OTHER_SHARED_RUN, &theirs, &ours),
        ] {
            let result = replay(file.path(), Some(run), SHARED_REPLAY_COLUMNS);
            assert_eq!(
                result.warning, None,
                "the file holds no line that a cut ended"
            );
            let folded = match result.outcome {
                Ok(folded) => folded,
                Err(reason) => panic!("the replay of {run} must fold that run: {reason}"),
            };
            let named = words_of(&folded.lines);
            for hop in held {
                assert!(
                    named.contains(hop.as_str()),
                    "the frame of {run} names the hop {hop} that the run probed"
                );
            }
            for hop in absent {
                assert!(
                    !named.contains(hop.as_str()),
                    "the frame of {run} names no hop of the other run: {hop}"
                );
            }
        }
    }

    /// The name of the command that hunts for the longest path.
    const HUNT: &str = "hunt";

    /// The flag that counts the destinations of a hunt.
    const FLAG_ROUNDS: &str = "rounds";

    /// The flag that counts the probe rounds of one destination.
    const FLAG_PROBES: &str = "probes-per-round";

    /// The number of probe rounds that a test hunt gives each destination.
    ///
    /// The number differs from the default, so a wiring that read the default
    /// instead fails the test that reads this number back.
    const PROBES_OF_A_TEST_HUNT: u64 = 7;

    /// The destination that a test hands the tracer of a hunt. It is an address
    /// of the block that documentation takes, and no probe reaches it: the test
    /// builds the configuration of a tracer and starts none.
    const A_HUNT_DESTINATION: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);

    /// A hunt gives the tracer of a destination the probe rounds of its plan.
    ///
    /// The number reaches the tracer and the run loop by two paths. The tracer
    /// reads the resolved command line, and `hunt::Plan` carries the number to
    /// the limits of the run. A tracer of a smaller number closes its channel
    /// under a run loop that still waits, and the run then reports a dead
    /// tracer.
    #[test]
    fn a_hunt_gives_the_tracer_of_a_destination_the_probe_rounds_of_its_plan() {
        let flag = format!("--{FLAG_PROBES}");
        let count = PROBES_OF_A_TEST_HUNT.to_string();
        let config = resolve(&["krt", HUNT, &flag, &count]);
        let probes = SystemProbes {
            config: &config,
            privilege: Privilege::Unprivileged,
            running: Vec::new(),
        };
        let traced = probes.config_of(
            A_HUNT_DESTINATION,
            &RunId::at(Utc::now()),
            crate::trace::Lane::FIRST,
        );
        assert_eq!(traced.rounds, Some(PROBES_OF_A_TEST_HUNT));
    }

    /// The configuration of a hunt that a command line resolves to.
    ///
    /// # Panics
    ///
    /// Panics when the command line resolves to no hunt. Such a line is a
    /// mistake in the test.
    fn hunt(arguments: &[&str]) -> HuntConfig {
        resolve(arguments)
            .hunt
            .expect("the command line must resolve to a hunt")
    }

    /// The help of one flag of the `hunt` command.
    ///
    /// # Panics
    ///
    /// Panics when the command holds no such flag, or when the flag carries no
    /// help. Both are mistakes in the test.
    fn hunt_help(flag: &str) -> String {
        Cli::command()
            .find_subcommand(HUNT)
            .expect("the command line must hold the `hunt` command")
            .get_arguments()
            .find(|argument| argument.get_long() == Some(flag))
            .unwrap_or_else(|| panic!("the `hunt` command must hold `--{flag}`"))
            .get_help()
            .unwrap_or_else(|| panic!("`--{flag}` must carry help"))
            .to_string()
    }

    #[test]
    fn a_hunt_needs_no_destination() {
        assert_eq!(resolve(&["krt", HUNT]).destination, None);
    }

    #[test]
    fn a_hunt_takes_the_default_number_of_destinations() {
        assert_eq!(hunt(&["krt", HUNT]).rounds, HUNT_ROUNDS_DEFAULT);
    }

    /// The default asks for eight destinations that answered.
    ///
    /// Eight reached destinations make a hunt that measured something. The
    /// address space answers at a low rate, so a default far above eight spends
    /// minutes on addresses that answer nothing.
    #[test]
    fn the_default_hunt_asks_for_eight_destinations_that_answered() {
        assert_eq!(hunt(&["krt", HUNT]).rounds, 8);
    }

    #[test]
    fn a_hunt_takes_the_number_of_destinations_that_the_command_line_named() {
        assert_eq!(hunt(&["krt", HUNT, "--rounds", "12"]).rounds, 12);
    }

    #[test]
    fn a_hunt_takes_the_default_number_of_probe_rounds_for_each_destination() {
        assert_eq!(
            hunt(&["krt", HUNT]).probes_per_round,
            PROBES_PER_ROUND_DEFAULT
        );
    }

    #[test]
    fn a_hunt_takes_the_number_of_probe_rounds_that_the_command_line_named() {
        assert_eq!(
            hunt(&["krt", HUNT, "--probes-per-round", "5"]).probes_per_round,
            5
        );
    }

    #[test]
    fn a_hunt_takes_the_default_timeout_of_a_destination() {
        assert_eq!(
            render_duration(hunt(&["krt", HUNT]).target_timeout),
            TARGET_TIMEOUT_DEFAULT
        );
    }

    #[test]
    fn a_hunt_takes_the_timeout_of_a_destination_that_the_command_line_named() {
        assert_eq!(
            hunt(&["krt", HUNT, "--target-timeout", "5s"]).target_timeout,
            Duration::from_secs(5)
        );
    }

    /// A hunt that named no pool traces eight destinations at once.
    ///
    /// The number matches the default rounds, so a plain `krt hunt` starts
    /// every destination it needs at once.
    #[test]
    fn a_hunt_takes_the_default_number_of_destinations_at_once() {
        assert_eq!(
            hunt(&["krt", HUNT]).concurrency,
            HUNT_CONCURRENCY_DEFAULT,
            "a hunt of no flag traces the default number of destinations at once"
        );
        assert_eq!(HUNT_CONCURRENCY_DEFAULT.get(), 8);
    }

    #[test]
    fn a_hunt_takes_the_number_of_destinations_at_once_that_the_command_line_named() {
        assert_eq!(
            hunt(&["krt", HUNT, &format!("--{FLAG_CONCURRENCY}"), "5"])
                .concurrency
                .get(),
            5
        );
    }

    /// A hunt of no destination at once measures nothing, so the parser refuses
    /// it.
    #[test]
    fn a_hunt_of_no_destination_at_once_fails_at_the_parser() {
        assert!(
            Cli::try_parse_from(["krt", HUNT, &format!("--{FLAG_CONCURRENCY}"), "0"]).is_err(),
            "a hunt that traces no destination at once measures nothing"
        );
    }

    /// A hunt of more destinations at once than the process holds lanes for
    /// fails at the parser.
    ///
    /// Two destinations of one lane read each other's answers, so a pool above
    /// the lanes of the process is a hunt that measures the wrong path. The
    /// refusal names the ceiling.
    #[test]
    fn a_hunt_of_more_destinations_at_once_than_lanes_fails_and_names_the_ceiling() {
        let over = (crate::trace::Lane::COUNT + 1).to_string();
        let refused = Cli::try_parse_from(["krt", HUNT, &format!("--{FLAG_CONCURRENCY}"), &over])
            .expect_err("a pool above the lanes of the process measures the wrong path")
            .to_string();
        assert!(
            refused.contains(&crate::trace::Lane::COUNT.to_string()),
            "the refusal names the ceiling: {refused}"
        );
    }

    /// The block of a hunt names the destinations it traces at once.
    #[test]
    fn the_resolved_block_of_a_hunt_names_the_destinations_it_traces_at_once() {
        let block = resolve(&["krt", HUNT, &format!("--{FLAG_CONCURRENCY}"), "6"]).to_string();
        assert!(
            block.contains("at once:"),
            "the block of a hunt names the destinations it traces at once: {block}"
        );
        assert!(
            block.contains('6'),
            "the block names the number that the line asked for: {block}"
        );
    }

    /// The flag that counts the destinations a hunt traces at once.
    const FLAG_CONCURRENCY: &str = "concurrency";

    #[test]
    fn a_hunt_takes_the_seed_that_the_command_line_named() {
        assert_eq!(hunt(&["krt", HUNT, "--seed", "12345"]).seed, 12_345);
    }

    /// A hunt that named no seed still resolves to one.
    ///
    /// The block of the resolved configuration prints the number, so a reader
    /// who wants that hunt back names it to `--seed`. A hunt of no seed at all
    /// would leave that reader nothing to name.
    #[test]
    fn a_hunt_that_named_no_seed_takes_one_of_its_own() {
        let first = hunt(&["krt", HUNT]).seed;
        let second = hunt(&["krt", HUNT]).seed;
        assert_ne!(
            (first, second),
            (0, 0),
            "a hunt of no seed takes one off the clock"
        );
    }

    /// The flag that turns the mine of the near space on.
    const FLAG_MINE: &str = "--mine";

    /// The flag that counts the addresses of one mine.
    const FLAG_MINE_DEPTH: &str = "--mine-depth";

    /// The flag that names the block one mine stays inside.
    const FLAG_MINE_PREFIX: &str = "--mine-prefix";

    /// The flag that caps the addresses of one /24.
    const FLAG_MINE_PER_PREFIX: &str = "--mine-per-prefix";

    /// The flag that names the wait between two addresses of one mine.
    const FLAG_MINE_DELAY: &str = "--mine-delay";

    /// The mine of one hunt that a test resolved.
    ///
    /// # Panics
    ///
    /// Panics on a hunt that mines nothing. Such a call is a mistake in the
    /// test, not an answer the code under test can give.
    fn mine(arguments: &[&str]) -> crate::hunt::MinePlan {
        hunt(arguments)
            .mine
            .expect("the hunt of this test must mine the near space")
    }

    #[test]
    fn a_hunt_mines_nothing_by_default() {
        assert_eq!(hunt(&["krt", HUNT]).mine, None);
    }

    #[test]
    fn a_hunt_that_asked_mines_the_near_space() {
        assert!(hunt(&["krt", HUNT, FLAG_MINE]).mine.is_some());
    }

    #[test]
    fn a_mine_takes_the_default_depth_the_prefix_the_cap_and_the_delay() {
        let plan = mine(&["krt", HUNT, FLAG_MINE]);
        assert_eq!(plan.depth.get(), 8);
        assert_eq!(plan.prefix, 16);
        assert_eq!(plan.per_prefix.get(), 2);
        assert_eq!(plan.delay, Duration::from_secs(2));
    }

    #[test]
    fn a_mine_takes_the_depth_that_the_command_line_named() {
        assert_eq!(
            mine(&["krt", HUNT, FLAG_MINE, FLAG_MINE_DEPTH, "4"])
                .depth
                .get(),
            4
        );
    }

    #[test]
    fn a_mine_takes_the_prefix_that_the_command_line_named() {
        assert_eq!(
            mine(&["krt", HUNT, FLAG_MINE, FLAG_MINE_PREFIX, "20"]).prefix,
            20
        );
    }

    #[test]
    fn a_mine_takes_the_cap_of_one_prefix_that_the_command_line_named() {
        assert_eq!(
            mine(&["krt", HUNT, FLAG_MINE, FLAG_MINE_PER_PREFIX, "3"])
                .per_prefix
                .get(),
            3
        );
    }

    #[test]
    fn a_mine_takes_the_delay_that_the_command_line_named() {
        assert_eq!(
            mine(&["krt", HUNT, FLAG_MINE, FLAG_MINE_DELAY, "500ms"]).delay,
            Duration::from_millis(500)
        );
    }

    /// A mine of no address probes nothing, so the parser refuses it.
    #[test]
    fn a_mine_of_no_address_fails_at_the_parser() {
        assert!(Cli::try_parse_from(["krt", HUNT, FLAG_MINE, FLAG_MINE_DEPTH, "0"]).is_err());
    }

    /// A mine that probes no address of one /24 probes nothing.
    #[test]
    fn a_mine_of_no_address_of_one_prefix_fails_at_the_parser() {
        assert!(Cli::try_parse_from(["krt", HUNT, FLAG_MINE, FLAG_MINE_PER_PREFIX, "0"]).is_err());
    }

    /// A prefix below the floor is a block that is no near space.
    ///
    /// A `/4` holds a sixteenth of the address space, and a draw inside it is a
    /// draw of the whole internet under another name.
    #[test]
    fn a_mine_of_a_prefix_below_the_floor_fails_and_names_the_range() {
        let refused = Cli::try_parse_from(["krt", HUNT, FLAG_MINE, FLAG_MINE_PREFIX, "4"])
            .expect_err("a block that holds a sixteenth of the address space is no near space")
            .to_string();
        assert!(
            refused.contains(&MINE_PREFIX_FLOOR.to_string())
                && refused.contains(&MINE_PREFIX_CEILING.to_string()),
            "the refusal names the range: {refused}"
        );
    }

    /// A prefix above the ceiling is a block smaller than the /24 a mine draws
    /// at.
    #[test]
    fn a_mine_of_a_prefix_above_the_ceiling_fails_and_names_the_range() {
        let refused = Cli::try_parse_from(["krt", HUNT, FLAG_MINE, FLAG_MINE_PREFIX, "25"])
            .expect_err("a block below the /24 that a mine draws at holds no address to draw")
            .to_string();
        assert!(
            refused.contains(&MINE_PREFIX_CEILING.to_string()),
            "the refusal names the ceiling: {refused}"
        );
    }

    /// A flag of a mine without `--mine` asks for a mine that never runs.
    ///
    /// Every one of the four bounds is refused, so no line can name a number
    /// that the hunt then ignores.
    #[test]
    fn a_flag_of_a_mine_without_the_mine_flag_fails_at_the_parser() {
        for flag in [
            [FLAG_MINE_DEPTH, "4"],
            [FLAG_MINE_PREFIX, "20"],
            [FLAG_MINE_PER_PREFIX, "3"],
            [FLAG_MINE_DELAY, "500ms"],
        ] {
            assert!(
                Cli::try_parse_from(["krt", HUNT, flag[0], flag[1]]).is_err(),
                "`{} {}` without `{FLAG_MINE}` names a bound that no mine reads",
                flag[0],
                flag[1]
            );
        }
    }

    /// The block of a hunt that mines nothing says so.
    #[test]
    fn the_resolved_block_of_a_hunt_that_mines_nothing_names_no_bound_of_a_mine() {
        let block = resolve(&["krt", HUNT]).to_string();
        assert!(
            block.contains("mine:") && block.contains("off"),
            "the block of a hunt names whether it mines: {block}"
        );
        assert!(
            !block.contains("mine depth:"),
            "a hunt that mines nothing names no bound of a mine: {block}"
        );
    }

    /// The block of a hunt that mines names every bound of its mine.
    #[test]
    fn the_resolved_block_of_a_hunt_that_mines_names_every_bound() {
        let block = resolve(&["krt", HUNT, FLAG_MINE]).to_string();
        for row in [
            "mine:",
            "mine depth:",
            "mine prefix:",
            "mine per prefix:",
            "mine delay:",
        ] {
            assert!(
                block.contains(row),
                "the block of a hunt that mines names `{row}`: {block}"
            );
        }
        assert!(
            block.contains("/16"),
            "the block names the prefix as a block length: {block}"
        );
    }

    #[test]
    fn a_hunt_lets_no_partial_path_compete_by_default() {
        assert!(!hunt(&["krt", HUNT]).include_partial);
    }

    #[test]
    fn a_hunt_that_asked_lets_a_partial_path_compete() {
        assert!(hunt(&["krt", HUNT, "--include-partial"]).include_partial);
    }

    #[test]
    fn a_hunt_takes_the_protocol_of_a_probe() {
        assert_eq!(
            resolve(&["krt", HUNT, "--protocol", "udp"]).protocol,
            Protocol::Udp
        );
    }

    #[test]
    fn a_hunt_takes_the_range_of_the_ttl() {
        let config = resolve(&["krt", HUNT, "--first-ttl", "3", "--max-ttl", "9"]);
        assert_eq!((config.first_ttl, config.max_ttl), (3, 9));
    }

    #[test]
    fn a_hunt_takes_the_flag_that_reads_no_name() {
        assert!(!resolve(&["krt", HUNT, "--no-dns"]).reverse_dns);
    }

    #[test]
    fn a_hunt_takes_the_file_that_the_command_line_named() {
        assert_eq!(
            resolve(&["krt", HUNT, "--output", "hunt.jsonl"]).output,
            Some(PathBuf::from("hunt.jsonl"))
        );
    }

    #[test]
    fn a_hunt_takes_the_source_that_the_command_line_named() {
        assert_eq!(
            resolve(&["krt", HUNT, "--source", "1.2.3.4"]).source,
            Some(address("1.2.3.4"))
        );
    }

    /// A hunt probes at the period that the command line named.
    ///
    /// The tracer of each destination takes the period of the resolved
    /// configuration, so a hunt that reads the default alone probes once a
    /// second whatever the line asked for.
    #[test]
    fn a_hunt_takes_the_interval_that_the_command_line_named() {
        assert_eq!(
            resolve(&["krt", HUNT, "--interval", "500ms"]).interval,
            Duration::from_millis(500)
        );
    }

    /// The block of a hunt names the period that every probe of it takes.
    ///
    /// The period sets how long each destination holds the hunt, so a block
    /// that leaves it out keeps the number from every reader.
    #[test]
    fn the_resolved_block_of_a_hunt_names_the_interval() {
        let block = resolve(&["krt", HUNT, "--interval", "500ms"]).to_string();
        assert!(
            block.contains("interval:"),
            "the block of a hunt names the interval: {block}"
        );
        assert!(
            block.contains("500ms"),
            "the block of a hunt names the period that the line asked for: {block}"
        );
    }

    /// The block of a hunt names the multipath mode that its probes take.
    ///
    /// `--multipath` stands on the top of the command line alone, and a flag in
    /// front of `hunt` reads `hunt` as the destination. A hunt therefore always
    /// probes the classic mode, and a block that leaves the mode out keeps that
    /// fact from every reader.
    #[test]
    fn the_resolved_block_of_a_hunt_names_the_multipath_mode() {
        let block = resolve(&["krt", HUNT]).to_string();
        assert!(
            block.contains("multipath:"),
            "the block of a hunt names the multipath mode: {block}"
        );
        assert!(
            block.contains("classic"),
            "a hunt probes the classic multipath mode: {block}"
        );
    }

    /// The default lets a hunt trace 128 destinations before it gives up.
    #[test]
    fn the_default_hunt_gives_up_after_a_hundred_and_twenty_eight_targets() {
        let block = resolve(&["krt", HUNT]).to_string();
        assert!(
            block.contains("max targets:"),
            "the block of a hunt names the cap of its targets: {block}"
        );
        assert!(
            block.contains("128"),
            "the cap of a hunt that named none is 128: {block}"
        );
    }

    /// The block of a hunt names the cap that the command line asked for.
    #[test]
    fn the_resolved_block_of_a_hunt_names_the_cap_that_the_command_line_named() {
        let block = resolve(&["krt", HUNT, "--max-targets", "20"]).to_string();
        assert!(
            block.contains("max targets:"),
            "the block of a hunt names the cap of its targets: {block}"
        );
        assert!(
            block.contains("20"),
            "the block names the cap that the line asked for: {block}"
        );
    }

    /// A hunt of no target traces nothing, so the parser rejects it.
    #[test]
    fn a_hunt_of_no_target_is_rejected() {
        let error = rejection(&["krt", HUNT, "--max-targets", "0"]);
        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }

    /// A cap below the rounds of its own line contradicts that line.
    ///
    /// A hunt that gives up after four destinations never holds eight that
    /// answered. Such a line asks for two things at once, so the tool says so
    /// in the place of a hunt that gives up every time.
    #[test]
    fn a_cap_below_the_rounds_of_its_own_line_is_rejected() {
        let reason = contradiction(&["krt", HUNT, "--rounds", "8", "--max-targets", "4"]);
        for expected in ["--max-targets", "4", "--rounds", "8"] {
            assert!(
                reason.contains(expected),
                "the reason names `{expected}`: {reason}"
            );
        }
    }

    /// A cap that equals the rounds of its own line holds them.
    ///
    /// Such a hunt needs every destination it traces to answer, which is a hard
    /// ask and not a contradiction.
    #[test]
    fn a_cap_that_equals_the_rounds_of_its_own_line_resolves() {
        assert_eq!(
            hunt(&["krt", HUNT, "--rounds", "4", "--max-targets", "4"]).rounds,
            4
        );
    }

    /// A hunt of no round records nothing, so the parser rejects it.
    #[test]
    fn a_hunt_of_no_round_is_rejected() {
        let error = rejection(&["krt", HUNT, "--rounds", "0"]);
        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }

    /// A destination of no probe round measures nothing, so the parser rejects
    /// it.
    #[test]
    fn a_destination_of_no_probe_round_is_rejected() {
        let error = rejection(&["krt", HUNT, "--probes-per-round", "0"]);
        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }

    /// A timeout that cannot hold the probe rounds of its own line contradicts
    /// that line.
    ///
    /// The tracer sends one probe round each interval, and the first round goes
    /// out one interval after the run starts, so a destination of 20 probe
    /// rounds at a period of one second needs more than 20 seconds. The check
    /// asks for 21 seconds, which is the time of one round more. A timeout of
    /// ten seconds cuts such a destination short after ten of its rounds, and
    /// the record of it says nothing about the eleven that never went out.
    #[test]
    fn a_target_timeout_that_cannot_hold_the_probe_rounds_is_rejected() {
        let reason = contradiction(&["krt", HUNT, "--probes-per-round", "20"]);
        for expected in [
            "--target-timeout",
            "10s",
            "--probes-per-round",
            "20",
            "1s",
            "21s",
        ] {
            assert!(
                reason.contains(expected),
                "the reason names `{expected}`: {reason}"
            );
        }
    }

    /// A timeout that holds the probe rounds of its line and no more is
    /// rejected.
    ///
    /// A run of two probe rounds at a period of one second lands its last round
    /// past the second second, because the first round goes out one interval
    /// after the run starts. A timeout of 2001 milliseconds thus holds the two
    /// rounds by the arithmetic alone, and it still stops the destination after
    /// the first one. The check therefore asks for the time of one round more,
    /// which is three seconds here.
    #[test]
    fn a_target_timeout_that_holds_the_probe_rounds_and_no_more_is_rejected() {
        let reason = contradiction(&[
            "krt",
            HUNT,
            "--probes-per-round",
            "2",
            "--interval",
            "1s",
            "--target-timeout",
            "2001ms",
        ]);
        for expected in [
            "--target-timeout",
            "2001ms",
            "--probes-per-round",
            "2",
            "1s",
            "3s",
        ] {
            assert!(
                reason.contains(expected),
                "the reason names `{expected}`: {reason}"
            );
        }
    }

    /// A count of probe rounds whose time no duration holds is rejected.
    ///
    /// `--probes-per-round` counts up to the width of a `u64`, and the time
    /// that such a count takes runs past every duration. The check reads that
    /// time as one that no timeout reaches, so it refuses the line in the place
    /// of a panic or a product that wrapped.
    #[test]
    fn a_count_of_probe_rounds_that_no_duration_holds_is_rejected() {
        let count = u64::MAX.to_string();
        let reason = contradiction(&["krt", HUNT, "--probes-per-round", &count]);
        assert!(
            reason.contains(&count),
            "the reason names the count: {reason}"
        );
        assert!(
            reason.contains(TIME_BEYOND_A_DURATION),
            "the reason says that no duration holds the time it needs: {reason}"
        );
    }

    /// A timeout raised for the probe rounds of its line holds them.
    #[test]
    fn a_target_timeout_that_holds_the_probe_rounds_resolves() {
        assert_eq!(
            hunt(&[
                "krt",
                HUNT,
                "--probes-per-round",
                "20",
                "--target-timeout",
                "30s",
            ])
            .probes_per_round,
            20
        );
    }

    /// The defaults of a hunt hold each other, so `krt hunt` alone resolves.
    ///
    /// The timeout of a destination must hold one round more than the probe
    /// rounds that the same line asks for, because the last round lands past
    /// the time of the rounds alone. A pair of defaults that broke that rule
    /// would reject the plainest line of the command.
    #[test]
    fn the_default_timeout_of_a_hunt_holds_the_default_probe_rounds() {
        let config = resolve(&["krt", HUNT]);
        let plan = config
            .hunt
            .expect("the command line must resolve to a hunt");
        let rounds = u32::try_from(plan.probes_per_round).expect("the default counts a few rounds");
        assert!(
            plan.target_timeout > config.interval * (rounds + 1),
            "a timeout of {:?} holds one round more than {} probe rounds at a period of {:?}",
            plan.target_timeout,
            plan.probes_per_round,
            config.interval
        );
    }

    /// A flag of a probe stands behind the command and never in front of it.
    ///
    /// A line that names a flag in front of the command reads the command as
    /// the destination, so `krt --protocol udp hunt` would trace a host named
    /// `hunt`. The run says so and names the line that hunts.
    #[test]
    fn a_flag_of_a_probe_in_front_of_the_hunt_is_rejected() {
        let reason = parse(&["krt", "--protocol", "udp", HUNT])
            .resolve()
            .expect_err("a destination that names a command must be rejected");
        assert!(
            reason.contains(HUNT),
            "the reason names the command: {reason}"
        );
    }

    /// The guard of a command read as a destination runs in front of every
    /// other check.
    ///
    /// `krt --multipath paris hunt` breaks two rules at once: the mode needs
    /// UDP, and the line reads `hunt` as a destination. The second one is the
    /// fault, because the mode reaches no probe of a hunt at all. A message
    /// about the protocol sends the reader after a flag that the line must
    /// drop.
    #[test]
    fn a_multipath_mode_in_front_of_a_command_names_the_command() {
        let reason = contradiction(&["krt", "--multipath", "paris", HUNT]);
        assert!(
            reason.contains(HUNT),
            "the reason names the command that the line read as a destination: {reason}"
        );
    }

    /// The guard covers every command and not the one that prompted it.
    #[test]
    fn a_flag_of_a_probe_in_front_of_the_replay_is_rejected() {
        let reason = parse(&["krt", "--no-dns", REPLAY])
            .resolve()
            .expect_err("a destination that names a command must be rejected");
        assert!(
            reason.contains(REPLAY),
            "the reason names the command: {reason}"
        );
    }

    /// The name of the command that prints the help of another command.
    ///
    /// Clap writes this command into the parser as it builds it, so the
    /// derive macro alone names it nowhere.
    const HELP: &str = "help";

    /// The guard covers the command that clap itself adds.
    ///
    /// Clap puts the `help` command into the parser at build time, so a guard
    /// that reads the commands of a parser it did not build sees `replay` and
    /// `hunt` alone. `krt --headless help` then goes to the network and looks
    /// for a host named `help`.
    #[test]
    fn a_flag_of_a_probe_in_front_of_the_help_is_rejected() {
        let reason = contradiction(&["krt", "--headless", HELP]);
        assert!(
            reason.contains(HELP),
            "the reason names the command: {reason}"
        );
    }

    /// The reason names no flag that clap itself adds.
    ///
    /// Clap puts `--help` on every command and `--version` on the top level as
    /// it builds the parser. A reason that read those two as flags of a probe
    /// would name `--version` as a flag that the command refuses, and the
    /// reader would look for a flag that no line of a trace holds.
    #[test]
    fn the_reason_of_a_command_read_as_a_destination_names_no_generated_flag() {
        let reason = contradiction(&["krt", "--headless", HUNT]);
        assert!(
            !reason.contains("--version"),
            "clap adds `--version`, and the reason names it nowhere: {reason}"
        );
        assert!(
            !reason.contains("--help"),
            "clap adds `--help`, and the reason names it nowhere: {reason}"
        );
    }

    /// The reason names every flag of the top level that the command refuses.
    ///
    /// The repair that the reason offers — write the command first — works for
    /// a flag that the command shares, and it fails for a flag that stands on
    /// the top level alone: `krt hunt --headless` answers that the flag is
    /// unknown. The reason therefore names the flags of the second kind, so the
    /// reader takes one line and writes a command line that runs.
    #[test]
    fn the_reason_of_a_command_read_as_a_destination_names_the_flags_it_refuses() {
        let reason = contradiction(&["krt", "--headless", HUNT]);
        assert!(
            reason.contains("--headless"),
            "a hunt draws no live table, so the reason names `--headless`: {reason}"
        );
        let reason = contradiction(&["krt", "--no-dns", REPLAY]);
        assert!(
            reason.contains("--no-dns"),
            "a replay reads the names of the file it folds, so the reason names `--no-dns`: {reason}"
        );
    }

    /// The reason names no flag that the command takes.
    ///
    /// A hunt takes `--protocol`, so the repair works for that flag and the
    /// reason says nothing about it. A reason that named every flag of the top
    /// level would send the reader away from a flag that the command holds.
    #[test]
    fn the_reason_of_a_command_read_as_a_destination_names_no_flag_it_takes() {
        let reason = contradiction(&["krt", "--protocol", "udp", HUNT]);
        assert!(
            !reason.contains("--protocol"),
            "a hunt takes `--protocol`, so the reason names it nowhere: {reason}"
        );
    }

    /// A host whose name merely starts with the name of a command still traces.
    #[test]
    fn a_destination_that_only_starts_with_the_name_of_a_command_still_traces() {
        assert_eq!(
            resolve(&["krt", "hunter.example.com"])
                .destination
                .as_deref(),
            Some("hunter.example.com")
        );
    }

    /// The two flags that carry the word `round` each say what they count.
    ///
    /// A run of `krt` calls one sweep of the TTLs a round, and a hunt calls one
    /// destination a round. A reader of the help must not have to guess which
    /// of the two a flag counts.
    #[test]
    fn the_help_of_the_rounds_of_a_hunt_says_that_it_counts_destinations() {
        let help = hunt_help(FLAG_ROUNDS);
        assert!(
            help.contains("destination"),
            "`--{FLAG_ROUNDS}` counts destinations, and its help must say so: {help}"
        );
    }

    #[test]
    fn the_help_of_the_probes_of_a_round_says_that_it_counts_probe_rounds() {
        let help = hunt_help(FLAG_PROBES);
        assert!(
            help.contains("probe round"),
            "`--{FLAG_PROBES}` counts probe rounds, and its help must say so: {help}"
        );
    }

    #[test]
    fn the_help_of_the_rounds_of_a_trace_says_that_it_counts_sweeps_of_the_ttls() {
        let help = Cli::command()
            .get_arguments()
            .find(|argument| argument.get_long() == Some(FLAG_ROUNDS))
            .expect("the command line must hold `--rounds`")
            .get_help()
            .expect("`--rounds` must carry help")
            .to_string();
        assert!(
            help.contains("sweep"),
            "the `--{FLAG_ROUNDS}` of a trace counts sweeps of the TTLs: {help}"
        );
    }

    /// The block of a hunt names what the hunt will do.
    #[test]
    fn the_resolved_block_of_a_hunt_names_every_number_of_the_hunt() {
        let block = resolve(&[
            "krt",
            HUNT,
            "--rounds",
            "8",
            "--probes-per-round",
            "5",
            "--target-timeout",
            "20s",
            "--seed",
            "12345",
        ])
        .to_string();
        for expected in [
            "rounds:",
            "8",
            "probes per round:",
            "5",
            "target timeout:",
            "20s",
            "seed:",
            "12345",
        ] {
            assert!(
                block.contains(expected),
                "the block of a hunt names `{expected}`: {block}"
            );
        }
    }

    /// The glyph that ends the key of a row of the block.
    const COLON: &str = ":";

    /// The column that every value of a block stands in.
    fn value_columns(block: &str) -> Vec<usize> {
        block
            .lines()
            .skip(1)
            .map(|line| {
                let (key, value) = split_key(line);
                // The count is of characters and not of bytes, because a column
                // of a terminal holds a character.
                key.chars().count()
                    + COLON.len()
                    + (value.chars().count() - value.trim_start().chars().count())
            })
            .collect()
    }

    /// The key of one row of a block, and the text that follows its colon.
    ///
    /// The split is on the first colon, so a value that holds a colon of its
    /// own — an address of ip version 6, and a path on some machines — takes no
    /// part in the answer.
    ///
    /// # Panics
    ///
    /// Panics on a row that holds no colon. Such a row is a defect of the
    /// block, not an answer that a test names.
    fn split_key(line: &str) -> (&str, &str) {
        line.split_once(COLON)
            .expect("every row of the block holds a key")
    }

    /// Asserts that every value of a block stands in one column, one space
    /// clear of the longest key.
    fn values_line_up(arguments: &[&str]) {
        let block = resolve(arguments).to_string();
        let columns = value_columns(&block);
        let first = *columns.first().expect("the block holds a row");
        assert!(
            columns.iter().all(|column| *column == first),
            "every value of the block stands in one column: {block}"
        );
        for line in block.lines().skip(1) {
            assert!(
                split_key(line).1.starts_with(' '),
                "one space at the least stands between a key and its value: {line}"
            );
        }
    }

    /// The keys of a hunt are longer than the keys of a trace, so a width that
    /// a constant fixed would run the longest of them into its value.
    #[test]
    fn every_value_of_the_block_of_a_hunt_stands_in_one_column() {
        values_line_up(&["krt", HUNT]);
    }

    #[test]
    fn every_value_of_the_block_of_a_trace_stands_in_one_column() {
        values_line_up(&["krt", "example.com"]);
    }

    /// The block of a hunt names no field that says nothing about a hunt.
    ///
    /// A hunt draws its own destination, it draws addresses of ip version 4
    /// alone, and it draws no live table. A block that named a destination, an
    /// address family, or a display would state three things that the run does
    /// not do.
    #[test]
    fn the_resolved_block_of_a_hunt_names_no_field_of_a_trace_alone() {
        let block = resolve(&["krt", HUNT]).to_string();
        for absent in ["destination:", "address family:", "display:"] {
            assert!(
                !block.contains(absent),
                "the block of a hunt names no `{absent}`: {block}"
            );
        }
    }
}
