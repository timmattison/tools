//! The hunt: many destinations, one file, and the table that ranks them.
//!
//! A hunt draws an address, traces it, scores the path it found, and takes the
//! next round. It stops when it runs out of rounds, and it prints one table of
//! four rows: the shortest path, the longest path, the fastest path, and the
//! slowest path.
//!
//! The word `round` carries two meanings around this module, and the two names
//! keep them apart. A round of the hunt is one destination, and `Plan::rounds`
//! counts those. A probe round is one sweep of the TTLs, and
//! `Plan::probes_per_round` counts the probe rounds that each destination
//! takes. The run loop of `run.rs` knows the second meaning alone.
//!
//! Both sources that a hunt draws on are seams, so no test of this module sends
//! a packet:
//!
//! - The addresses come from an iterator of candidates. A test hands the draw a
//!   list, and the seeded iterator of [`random`] is the source of a real hunt.
//!   `--seed` is what makes one hunt repeat another.
//! - The rounds come from a [`Probes`]. A test hands back a channel it filled,
//!   and the tracer of `trace.rs` is the source of a real hunt.
//!
//! The draw rejects every candidate that no packet routes to, and it rejects
//! every candidate it already gave. A hunt therefore traces each destination
//! once, and it spends no round on an address that answers nothing by
//! construction.
//!
//! Each destination takes one run of `run::record`, so a hunt writes the
//! records that a trace writes and a replay folds any one of its runs with no
//! change. The screen of that run is the [`Scorer`], which folds the rounds
//! into the numbers that the table ranks and draws nothing: a hunt shows one
//! table at the end and no live table. The fold therefore rides on the door
//! that the run loop already knocks on, and the hunt reads no file back.
//!
//! The hunt traces one destination at a time and never two at once. A
//! measurement of 64 destinations at the normal interval is a small load, and
//! it stays that way only while the hunt is serial.

use crate::live::Screen;
use crate::names;
use crate::names::Namer;
use crate::record::{
    EndReason, Family, HuntId, NameRecord, RoundRecord, RunConfig, RunId, RunRecord, SourceLabel,
    Target, Writer,
};
use crate::run;
use crate::run::RunError;
use crate::stats::{HopTable, TtlRow};
use crate::status::{Event, Status};
use crate::ui;
use crate::{counted, REACHED, ROUND};
use chrono::{DateTime, Utc};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr};
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// The number of candidates that one draw reads before it gives up.
///
/// The reserved blocks hold about one part in seven of the address space, so a
/// draw of a well formed source answers on its first candidate almost every
/// time. The bound is for the source that answers nothing else: a scripted
/// list of reserved addresses, and a hunt whose visited set already holds every
/// candidate its source gives. Without it, such a source spins forever.
const ATTEMPTS: usize = 1000;

/// The number of bits of an address of ip version 4.
const ADDRESS_BITS: u8 = 32;

/// One block of the address space that no packet routes to.
///
/// The block carries its own network and prefix, and it writes itself as the
/// CIDR text of the two. A name beside them would be a second spelling of the
/// same block, and the two spellings drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Block {
    /// The first address of the block.
    network: Ipv4Addr,
    /// The number of leading bits that every address of the block holds.
    prefix: u8,
}

impl Block {
    /// Builds the block of one network and one prefix.
    const fn new(network: Ipv4Addr, prefix: u8) -> Self {
        Self { network, prefix }
    }

    /// Answers whether this block holds the address.
    fn holds(self, addr: Ipv4Addr) -> bool {
        // A prefix of 32 shifts by no bit, and the mask is then every bit. No
        // block of the table carries a prefix of zero, which would shift by the
        // whole width and overflow.
        let mask = u32::MAX << (ADDRESS_BITS - self.prefix);
        addr.to_bits() & mask == self.network.to_bits()
    }
}

impl fmt::Display for Block {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.network, self.prefix)
    }
}

/// Every block that the draw rejects.
///
/// The table is the special-purpose registry of ip version 4, as RFC 6890 and
/// the registry that follows it hold it. A packet to any of these addresses
/// reaches no host of the internet, so a round spent on one measures nothing.
///
/// `255.255.255.255/32` stands inside `240.0.0.0/4`, and it keeps its own entry
/// because the registry holds it as an entry of its own. The table reads as the
/// registry, and not as the shortest set of blocks that covers it.
const RESERVED: [Block; 16] = [
    // This network.
    Block::new(Ipv4Addr::UNSPECIFIED, 8),
    // Private.
    Block::new(Ipv4Addr::new(10, 0, 0, 0), 8),
    // Shared address space, which a carrier puts behind one public address.
    Block::new(Ipv4Addr::new(100, 64, 0, 0), 10),
    // Loopback.
    Block::new(Ipv4Addr::new(127, 0, 0, 0), 8),
    // Link local.
    Block::new(Ipv4Addr::new(169, 254, 0, 0), 16),
    // Private.
    Block::new(Ipv4Addr::new(172, 16, 0, 0), 12),
    // The assignments of the IETF.
    Block::new(Ipv4Addr::new(192, 0, 0, 0), 24),
    // Documentation.
    Block::new(Ipv4Addr::new(192, 0, 2, 0), 24),
    // The anycast address of a relay from ip version 6 to ip version 4.
    Block::new(Ipv4Addr::new(192, 88, 99, 0), 24),
    // Private.
    Block::new(Ipv4Addr::new(192, 168, 0, 0), 16),
    // Benchmarking.
    Block::new(Ipv4Addr::new(198, 18, 0, 0), 15),
    // Documentation.
    Block::new(Ipv4Addr::new(198, 51, 100, 0), 24),
    // Documentation.
    Block::new(Ipv4Addr::new(203, 0, 113, 0), 24),
    // Multicast.
    Block::new(Ipv4Addr::new(224, 0, 0, 0), 4),
    // Reserved for a use that no standard names.
    Block::new(Ipv4Addr::new(240, 0, 0, 0), 4),
    // The limited broadcast address.
    Block::new(Ipv4Addr::BROADCAST, 32),
];

/// The block that holds an address, when a reserved block does. An address
/// that a packet routes to holds none.
fn reserved(addr: Ipv4Addr) -> Option<Block> {
    RESERVED.into_iter().find(|block| block.holds(addr))
}

/// The candidates of a seeded pseudo-random sequence, without end.
///
/// A hunt of one seed reads the same candidates in the same order as any other
/// hunt of that seed, for one build of `krt`. The generator of the `rand` crate
/// is free to change its sequence between two versions of that crate, so a seed
/// repeats a hunt of the same binary and promises nothing across an upgrade.
fn random(seed: u64) -> impl Iterator<Item = Ipv4Addr> {
    let mut rng = StdRng::seed_from_u64(seed);
    std::iter::repeat_with(move || Ipv4Addr::from_bits(rng.random()))
}

/// The draw of one hunt: the source of the candidates, and the addresses that
/// the hunt already visited.
pub(crate) struct Draw {
    /// The candidates, in the order the draw reads them.
    candidates: Box<dyn Iterator<Item = Ipv4Addr>>,
    /// Every address that this draw already gave.
    visited: HashSet<Ipv4Addr>,
    /// The address that a peek took out of the source and that no ask has
    /// taken yet.
    peeked: Option<Ipv4Addr>,
}

impl Draw {
    /// Builds the draw over a source of candidates.
    pub(crate) fn new(candidates: Box<dyn Iterator<Item = Ipv4Addr>>) -> Self {
        Self {
            candidates,
            visited: HashSet::new(),
            peeked: None,
        }
    }

    /// Builds the draw of a real hunt, over the seeded sequence of [`random`].
    pub(crate) fn seeded(seed: u64) -> Self {
        Self::new(Box::new(random(seed)))
    }

    /// The next address to trace, without taking it.
    ///
    /// The search for the source address of a hunt reads the route to one
    /// destination, and the first destination of the hunt is that one. The
    /// address stays in the draw, so the hunt traces it as its first round and
    /// the search costs the hunt no round.
    pub(crate) fn peek(&mut self) -> Option<Ipv4Addr> {
        if self.peeked.is_none() {
            self.peeked = self.take();
        }
        self.peeked
    }

    /// The next address to trace.
    pub(crate) fn address(&mut self) -> Option<Ipv4Addr> {
        self.peeked.take().or_else(|| self.take())
    }

    /// The next candidate that routes and that this hunt has not visited.
    ///
    /// The draw reads candidates until one of them passes. A source that ran
    /// out, and a source that gave [`ATTEMPTS`] candidates that the draw
    /// rejected, both give no address, and the hunt then stops.
    fn take(&mut self) -> Option<Ipv4Addr> {
        for _ in 0..ATTEMPTS {
            let candidate = self.candidates.next()?;
            if reserved(candidate).is_none() && self.visited.insert(candidate) {
                return Some(candidate);
            }
        }
        None
    }
}

/// Whether the destination of one round of a hunt answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathKind {
    /// The destination answered, and the path ends at it.
    Reached,
    /// The destination answered nothing, and the path ends at the last hop
    /// that did answer.
    Partial,
}

/// What one destination of a hunt gave.
///
/// The score reads the rounds of one trace and nothing else, so a test builds
/// one from scripted rounds and reaches no network.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Score {
    /// The address that the hunt traced.
    addr: Ipv4Addr,
    /// The name of that address, when a reverse lookup gave one.
    host: Option<String>,
    /// The run that recorded the trace, so `krt replay <file> --run <id>`
    /// prints the whole path.
    run: RunId,
    /// Whether the destination answered.
    kind: PathKind,
    /// The length of the path. A reached path ends at the TTL that the
    /// destination answered at, and a partial one ends at the highest TTL that
    /// any hop answered at. A destination that nothing answered for gives zero.
    length: u8,
    /// The mean round-trip time of the last hop that answered, in
    /// milliseconds. A path that no hop answered holds none.
    rtt_ms: Option<f64>,
    /// The loss of that same hop, as a percentage. A path that no hop answered
    /// holds none.
    loss: Option<f64>,
    /// The number of TTLs at or below the length that answered nothing.
    ///
    /// The count is of the holes inside the measured path. A TTL past the end
    /// of the path is no hole: the path stops there, and a probe that went
    /// further measured nothing that belongs to this path.
    gaps: usize,
}

/// The screen of one destination of a hunt.
///
/// A hunt draws no table. This screen folds the rounds of one destination into
/// the numbers that the summary ranks, and it shows nothing. The run loop
/// hands every round and every name to the screen already, so the fold rides
/// on the door that is there and the hunt reads no file back.
pub(crate) struct Scorer<'a> {
    /// The address that this destination stands at.
    addr: Ipv4Addr,
    /// The run that records the trace of it.
    run: RunId,
    /// The first TTL that a round of this trace probes.
    first_ttl: u8,
    /// Every round of the trace, folded by TTL.
    table: HopTable,
    /// The smallest TTL that the destination itself answered at.
    reached_at: Option<u8>,
    /// The name of each address that a reverse lookup gave.
    names: BTreeMap<IpAddr, String>,
    /// The indicator of the hunt that this destination belongs to.
    ///
    /// The turns of one destination are the heartbeat of the whole hunt, and
    /// this screen is what the run loop of that destination knocks on. The
    /// indicator therefore hears every turn without the hunt loop asking the
    /// run loop for one.
    status: &'a mut dyn Status,
}

impl<'a> Scorer<'a> {
    /// Builds the screen of one destination.
    pub(crate) fn new(
        addr: Ipv4Addr,
        run: RunId,
        first_ttl: u8,
        status: &'a mut dyn Status,
    ) -> Self {
        Self {
            addr,
            run,
            first_ttl,
            table: HopTable::new(),
            reached_at: None,
            names: BTreeMap::new(),
            status,
        }
    }

    /// What this destination gave.
    ///
    /// A destination that answered ends the path at the TTL it answered at, and
    /// one that answered nothing ends the path at the highest TTL that any hop
    /// answered at. The time and the loss both read the row of that TTL, which
    /// is the last hop of the path either way.
    pub(crate) fn score(self) -> Score {
        let (kind, length) = match self.reached_at {
            Some(ttl) => (PathKind::Reached, ttl),
            None => (PathKind::Partial, self.deepest_answer()),
        };
        let last = self.row(length);
        let rtt_ms = last.and_then(|row| row.stats().avg());
        let loss = last.and_then(TtlRow::loss);
        let gaps = self.gaps(length);
        let host = self.names.get(&IpAddr::V4(self.addr)).cloned();
        Score {
            addr: self.addr,
            host,
            run: self.run,
            kind,
            length,
            rtt_ms,
            loss,
            gaps,
        }
    }

    /// The highest TTL that any hop answered at. A trace that nothing answered
    /// for gives zero, which is a TTL that no probe carries.
    fn deepest_answer(&self) -> u8 {
        self.table
            .rows()
            .filter(|row| row.stats().recv() > 0)
            .map(TtlRow::ttl)
            .max()
            .unwrap_or(0)
    }

    /// The row of one TTL. A TTL that no round probed holds none.
    fn row(&self, ttl: u8) -> Option<&TtlRow> {
        self.table.rows().find(|row| row.ttl() == ttl)
    }

    /// The number of TTLs at or below the length that answered nothing.
    ///
    /// A path of no length holds no TTL at all, and it therefore holds no hole.
    /// The `partial` mark and the length of zero already say that the trace
    /// found nothing.
    fn gaps(&self, length: u8) -> usize {
        self.table
            .rows()
            .filter(|row| (self.first_ttl..=length).contains(&row.ttl()) && row.stats().recv() == 0)
            .count()
    }
}

impl Screen for Scorer<'_> {
    /// A hunt takes no key of the terminal, so this screen asks for no stop.
    ///
    /// `Ctrl-C` reaches a hunt through the signal flag, which stops the trace
    /// of the destination that stands and the hunt that holds it.
    fn poll(&mut self) -> bool {
        false
    }

    /// Folds one round into the table, and keeps the TTL of the destination
    /// when the destination answered.
    ///
    /// The smallest such TTL wins. A path that changes under a load balancer
    /// reaches the destination at one TTL in one round and at another in the
    /// next, and a packet did reach the destination in the smaller number of
    /// hops.
    fn round(&mut self, round: &RoundRecord) {
        self.table.observe(round);
        let answered = round
            .hops
            .iter()
            .filter(|hop| hop.addr == IpAddr::V4(self.addr))
            .map(|hop| hop.ttl)
            .min();
        if let Some(ttl) = answered {
            self.reached_at = Some(self.reached_at.map_or(ttl, |held| held.min(ttl)));
        }
    }

    /// Keeps the name of each address that a reverse lookup gave.
    ///
    /// The row of the summary names the destination alone, and the screen keeps
    /// every name it is handed: the run loop writes one record for each address
    /// of the path, and the destination is one of them.
    fn names(&mut self, names: &[NameRecord]) {
        for name in names {
            self.names.insert(name.addr, name.host.clone());
        }
    }
}

/// The source of the rounds of one destination.
///
/// A hunt of the command line spawns the tracer of `trace.rs`. A test hands
/// back a channel that it filled, so no test of the hunt sends a packet.
pub(crate) trait Probes {
    /// Starts the trace of one destination, and answers with the channel that
    /// its rounds arrive on.
    ///
    /// The run identifier is the identifier that the rounds of this
    /// destination carry.
    ///
    /// # Errors
    ///
    /// Returns the reason as text when the tracer does not start.
    fn start(&mut self, target: Ipv4Addr, run: &RunId) -> Result<Receiver<RoundRecord>, String>;
}

/// The fault that stopped a hunt.
#[derive(Debug, thiserror::Error)]
pub(crate) enum HuntError {
    /// A run of the hunt failed.
    #[error("{0}")]
    Run(
        /// The fault that the run reported.
        #[from]
        RunError,
    ),
    /// The tracer of one destination did not start.
    #[error("the tracer of {target} did not start: {reason}")]
    Tracer {
        /// The destination whose tracer did not start.
        target: Ipv4Addr,
        /// The reason that the tracer gave.
        reason: String,
    },
}

/// A hunt that a fault stopped: what it found before the fault, and the fault.
///
/// The rounds that finished measured what they measured, and a fault at round
/// 40 of 64 takes nothing away from the 39 in front of it. The two travel
/// together so the caller prints the table of those rounds and then the reason
/// that the hunt stopped.
#[derive(Debug)]
pub(crate) struct HuntStopped {
    /// The summary of the rounds that finished.
    pub(crate) summary: Summary,
    /// The fault that stopped the hunt.
    pub(crate) fault: HuntError,
}

/// What every run of one hunt has in common.
pub(crate) struct Facts {
    /// The identifier of the hunt, which every run of it carries.
    pub(crate) id: HuntId,
    /// The build string of the `krt` that makes the hunt.
    pub(crate) krt: String,
    /// The address that the probes leave from.
    pub(crate) source: SourceLabel,
    /// The configuration that every run of the hunt records.
    pub(crate) config: RunConfig,
    /// The name of the machine that makes the hunt.
    pub(crate) host: String,
}

/// The numbers that bound one hunt.
pub(crate) struct Plan {
    /// The number of destinations that the hunt traces.
    pub(crate) rounds: u64,
    /// The number of probe rounds that each destination takes.
    pub(crate) probes_per_round: u64,
    /// The longest that one destination takes, whether it answers or not.
    pub(crate) target_timeout: Duration,
    /// The longest that each run waits, after its last round, for the names
    /// that its lookups have not given yet.
    pub(crate) name_grace: Duration,
    /// True when a partial path competes for a row of the summary.
    pub(crate) include_partial: bool,
}

/// The three things that a hunt draws on: the addresses, the rounds, and the
/// names.
pub(crate) struct Sources<'a> {
    /// The addresses that the hunt traces.
    pub(crate) draw: Draw,
    /// The rounds of each destination.
    pub(crate) probes: &'a mut dyn Probes,
    /// The resolver that every namer of the hunt asks.
    ///
    /// One resolver serves every destination, so a hunt starts the system
    /// resolver once and not once for each address it draws.
    pub(crate) resolver: Rc<dyn names::Resolver>,
}

/// Records one hunt: one run for each destination, and the summary of them all.
///
/// The hunt traces one destination at a time and never two at once. A
/// measurement of 64 destinations at the normal interval is a small load, and
/// it stays that way only while the hunt is serial.
///
/// Each destination takes one run of `run::record`, with a round limit of the
/// probe rounds of the plan and a deadline of the target timeout. The deadline
/// bounds every destination, and not the quiet ones alone: no destination holds
/// the hunt for longer than that timeout. The round limit is what stops a
/// destination that answers, because `Cli::resolve` refuses a plan whose probe
/// rounds run past its timeout.
///
/// `stop` answers whether the user asked the hunt to stop, and it reaches both
/// this loop and the run of the destination that stands. A destination that the
/// user cut short takes no row and no count of the summary: the summary counts
/// the rounds that finished.
///
/// A fault stops the hunt where the user does, and it keeps the same summary.
/// The rounds in front of the fault measured what they measured, and the caller
/// prints their table before it prints the reason.
///
/// # Errors
///
/// Returns a [`HuntStopped`] of the summary of the rounds that finished and the
/// fault that stopped the hunt. The fault is [`HuntError::Run`] when a record
/// does not reach the file, and [`HuntError::Tracer`] when the tracer of a
/// destination does not start.
pub(crate) fn record<W: Write>(
    facts: &Facts,
    plan: &Plan,
    sources: &mut Sources<'_>,
    stop: &dyn Fn() -> bool,
    writer: &mut Writer<W>,
    status: &mut dyn Status,
) -> Result<Summary, HuntStopped> {
    let started = Instant::now();
    // Both ways out of the loop build the same summary over the scores that the
    // hunt holds at that point, so the fault and the finish read alike.
    let summarize =
        |scores: Vec<Score>| Summary::new(scores, started.elapsed(), plan.include_partial);
    let mut scores = Vec::new();
    let mut previous = None;
    for _ in 0..plan.rounds {
        if stop() {
            break;
        }
        let Some(target) = sources.draw.address() else {
            break;
        };
        let moment = next_moment(previous, Utc::now());
        previous = Some(moment);
        let run = RunId::at(moment);
        match trace_one(facts, plan, sources, stop, writer, target, run, status) {
            Ok(Some(score)) => scores.push(score),
            Ok(None) => {}
            Err(fault) => {
                return Err(HuntStopped {
                    summary: summarize(scores),
                    fault,
                })
            }
        }
    }
    Ok(summarize(scores))
}

/// The moment that names the run of the next destination.
///
/// A run identifier holds the moment of the start to the millisecond, and a
/// hunt traces two destinations inside one millisecond whenever both of them
/// answer at once. Two runs of one identifier would leave a reader unable to
/// tell them apart, and `krt replay <file> --run <id>` unable to fold either
/// one. So the moment of a destination stands at least one millisecond after
/// the moment of the destination in front of it.
///
/// The shift is below the resolution that the identifier states, and the
/// identifier stays a moment that sorts, so the runs of one hunt still read in
/// the order the hunt traced them.
fn next_moment(previous: Option<DateTime<Utc>>, now: DateTime<Utc>) -> DateTime<Utc> {
    let Some(previous) = previous else {
        return now;
    };
    let least = previous + chrono::TimeDelta::milliseconds(1);
    if now > least {
        now
    } else {
        least
    }
}

/// Records one destination of a hunt, and scores the path it found.
///
/// A destination that the user cut short gives no score. The summary counts the
/// rounds that finished, and a round that stopped in the middle measured a path
/// that the tool never finished measuring.
///
/// # Errors
///
/// Returns [`HuntError::Run`] when a record does not reach the file, and
/// [`HuntError::Tracer`] when the tracer of this destination does not start.
#[allow(
    clippy::too_many_arguments,
    reason = "the loop of the hunt holds one of these and hands every one of them to this step; a struct of the eight would be the loop itself"
)]
fn trace_one<W: Write>(
    facts: &Facts,
    plan: &Plan,
    sources: &mut Sources<'_>,
    stop: &dyn Fn() -> bool,
    writer: &mut Writer<W>,
    target: Ipv4Addr,
    run: RunId,
    status: &mut dyn Status,
) -> Result<Option<Score>, HuntError> {
    let rounds = sources
        .probes
        .start(target, &run)
        .map_err(|reason| HuntError::Tracer { target, reason })?;
    let start = RunRecord {
        run: run.clone(),
        krt: facts.krt.clone(),
        source: facts.source.clone(),
        target: Target {
            // The hunt drew the address, so the address is what the user
            // named. A reader of the file thus finds the same text in the
            // field that a trace of that address by hand would write.
            arg: target.to_string(),
            addr: IpAddr::V4(target),
            family: Family::Ipv4,
        },
        config: facts.config,
        host: facts.host.clone(),
        hunt: Some(facts.id.clone()),
    };
    let limits = run::Limits {
        rounds: Some(plan.probes_per_round),
        // No destination holds the hunt for longer than this, whether it
        // answers or not. A limit too large to add to the clock leaves the
        // destination without a moment, and the round limit then stops it.
        deadline: Instant::now().checked_add(plan.target_timeout),
        name_grace: plan.name_grace,
    };
    let mut namer = Namer::new(Box::new(Rc::clone(&sources.resolver)), run.clone());
    let mut scorer = Scorer::new(target, run, facts.config.first_ttl, status);
    let outcome = run::record(
        &start,
        &rounds,
        &limits,
        stop,
        &mut namer,
        writer,
        &mut scorer,
    )?;
    if outcome.reason == EndReason::Quit {
        return Ok(None);
    }
    Ok(Some(scorer.score()))
}

/// The label of the row that names the shortest path.
const SHORTEST: &str = "shortest";

/// The label of the row that names the longest path.
const LONGEST: &str = "longest";

/// The label of the row that names the fastest path.
const FASTEST: &str = "fastest";

/// The label of the row that names the slowest path.
const SLOWEST: &str = "slowest";

/// The word that a row of a path the destination answered nothing for carries.
pub(crate) const PARTIAL: &str = "partial";

/// The one column that a row of the summary stands in from the left edge.
///
/// The table of a folded run stands one column in, and this one stands beside
/// it in the same terminal.
const ROW_START: &str = " ";

/// The text between two columns of the summary table.
///
/// Two columns, as the table of a folded run holds between two of its columns,
/// and for the reason that `ui::COLUMN_SPACING` gives: a number one column from
/// the number beside it reads as one longer number.
const COLUMN_GAP: &str = "  ";

/// What the summary says when no destination gave a path that the table ranks.
const NOTHING_TO_RANK: &str = "no destination gave a path to rank";

/// One row of the summary table: what the row ranks, and the destination that
/// holds it.
struct Row<'a> {
    /// The label of the row.
    label: &'static str,
    /// The destination that holds the row.
    score: &'a Score,
}

/// One column of the summary table.
///
/// The entry holds the heading, the side that the cell stands on, and the cell
/// of one row. One list therefore holds all three, so a column that leaves the
/// table takes its heading and its cells with it. Three lists would agree until
/// one of them changed, and then every cell behind the changed column would
/// stand under the heading of another column.
struct Column {
    /// The heading of the column.
    heading: &'static str,
    /// True when the cell stands against the right edge of the column.
    right: bool,
    /// The cell of one row.
    cell: fn(&Row) -> String,
}

/// The columns of the summary table, in the order they print.
const COLUMNS: [Column; 8] = [
    Column {
        heading: "Row",
        right: false,
        cell: |row| row.label.to_owned(),
    },
    Column {
        heading: "Host",
        right: false,
        cell: |row| row.score.host_text(),
    },
    Column {
        heading: "Len",
        right: true,
        cell: |row| row.score.length.to_string(),
    },
    Column {
        heading: "Path",
        right: false,
        cell: |row| row.score.kind.to_string(),
    },
    Column {
        heading: "Avg",
        right: true,
        cell: |row| ui::render_time(row.score.rtt_ms),
    },
    Column {
        heading: "Loss%",
        right: true,
        cell: |row| {
            row.score
                .loss
                .map_or_else(|| ui::NO_NUMBER.to_owned(), ui::render_percent)
        },
    },
    Column {
        heading: "Gaps",
        right: true,
        cell: |row| row.score.gaps.to_string(),
    },
    Column {
        heading: "Run",
        right: false,
        cell: |row| row.score.run.to_string(),
    },
];

impl fmt::Display for PathKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Reached => REACHED,
            Self::Partial => PARTIAL,
        })
    }
}

impl Score {
    /// The mean round-trip time that ranks this path.
    ///
    /// A path that no hop answered ranks after every path that one did. The
    /// table filters those paths out before it ranks, so no row of it ever
    /// reads this value.
    fn time(&self) -> f64 {
        self.rtt_ms.unwrap_or(f64::INFINITY)
    }

    /// The name of the destination with its address beside it, or the address
    /// alone.
    ///
    /// The address stays beside the name for the reason a row of a folded run
    /// keeps it: a name is what a resolver said, and an address is what
    /// answered. A run of `--no-dns` reads no name, and the cell then holds the
    /// address by itself.
    fn host_text(&self) -> String {
        self.host.as_ref().map_or_else(
            || self.addr.to_string(),
            |host| format!("{host} ({})", self.addr),
        )
    }
}

/// What a hunt found, and what it cost.
///
/// The summary reads the scores of the destinations that the hunt finished, so
/// a hunt that `Ctrl-C` stopped prints the same table over the rounds it did
/// finish. A hunt that a fault stopped prints it too, and the fault follows the
/// table.
#[derive(Debug)]
pub(crate) struct Summary {
    /// The score of each destination, in the order the hunt traced them.
    scores: Vec<Score>,
    /// The time that the whole hunt took.
    elapsed: Duration,
    /// True when a partial path competes for a row of the table.
    include_partial: bool,
}

impl Summary {
    /// Builds the summary of one hunt.
    pub(crate) fn new(scores: Vec<Score>, elapsed: Duration, include_partial: bool) -> Self {
        Self {
            scores,
            elapsed,
            include_partial,
        }
    }

    /// The lines of the summary: the table, a blank line, and the counts.
    ///
    /// The table takes the width that its cells need, and it cuts no name. It
    /// prints once, after the hunt, and it holds four rows at the most, so a
    /// column that grows costs a reader nothing. The table of a folded run
    /// stands under a different rule, because it draws on a terminal that the
    /// run holds and it redraws for every round.
    pub(crate) fn lines(&self) -> Vec<String> {
        let ranked = self.ranked();
        let mut lines = if ranked.is_empty() {
            vec![format!("{ROW_START}{NOTHING_TO_RANK}")]
        } else {
            table(&ranked)
        };
        lines.push(String::new());
        lines.push(self.counts());
        lines
    }

    /// The rows of the table, in the order they print.
    ///
    /// A row that no destination holds is absent. Every reached path holds a
    /// time, because the destination answered, so the fastest row and the
    /// slowest row go away only when a hunt of `--include-partial` ranks
    /// partial paths alone and no hop of any of them answered.
    fn ranked(&self) -> Vec<Row<'_>> {
        let candidates: Vec<&Score> = self
            .scores
            .iter()
            .filter(|score| self.include_partial || score.kind == PathKind::Reached)
            .collect();
        let timed: Vec<&Score> = candidates
            .iter()
            .copied()
            .filter(|score| score.rtt_ms.is_some())
            .collect();
        [
            (SHORTEST, pick(&candidates, |a, b| a.length < b.length)),
            (LONGEST, pick(&candidates, |a, b| a.length > b.length)),
            (FASTEST, pick(&timed, |a, b| a.time() < b.time())),
            (SLOWEST, pick(&timed, |a, b| a.time() > b.time())),
        ]
        .into_iter()
        .filter_map(|(label, score)| score.map(|score| Row { label, score }))
        .collect()
    }

    /// The line that counts what the hunt did.
    ///
    /// The line stands at the left edge, where the closing line of a trace
    /// stands, and the table above it stands one column in, where the table of
    /// a folded run stands. The two are different things: the table is a table,
    /// and this line closes the run.
    fn counts(&self) -> String {
        let reached = self
            .scores
            .iter()
            .filter(|score| score.kind == PathKind::Reached)
            .count();
        [
            counted(self.scores.len(), ROUND),
            format!("{reached} {REACHED}"),
            format!("{} {PARTIAL}", self.scores.len() - reached),
            ui::render_duration(self.elapsed),
        ]
        .join(ui::FIELD_SEPARATOR)
    }
}

/// The first score of the list that beats every other one.
///
/// `better` is strict, so the first of two scores that tie keeps the row. A
/// hunt therefore ranks the destination it traced first, and two runs of one
/// seed print the same table.
fn pick<'a>(scores: &[&'a Score], better: impl Fn(&Score, &Score) -> bool) -> Option<&'a Score> {
    let mut best: Option<&'a Score> = None;
    for score in scores {
        if best.is_none_or(|held| better(score, held)) {
            best = Some(score);
        }
    }
    best
}

/// The lines of the table: the column header, and one line for each row.
///
/// Each column takes the width of the widest cell it holds, and of its own
/// heading. The widths come out of [`COLUMNS`], which holds the heading and the
/// cell of each column together, so no cell can land under the heading of
/// another column.
fn table(rows: &[Row]) -> Vec<String> {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| COLUMNS.iter().map(|column| (column.cell)(row)).collect())
        .collect();
    let widths: Vec<usize> = COLUMNS
        .iter()
        .enumerate()
        .map(|(index, column)| {
            cells
                .iter()
                .map(|row| ui::display_width(&row[index]))
                .chain(std::iter::once(ui::display_width(column.heading)))
                .max()
                .unwrap_or_default()
        })
        .collect();
    let headings: Vec<String> = COLUMNS
        .iter()
        .map(|column| column.heading.to_owned())
        .collect();
    std::iter::once(&headings)
        .chain(cells.iter())
        .map(|row| line_of(row, &widths))
        .collect()
}

/// One line of the table: every cell, padded to the width of its column.
///
/// The line loses the spaces that follow its last cell. A trailing space says
/// nothing, and it turns a copy of the table into text that a reader must
/// clean.
fn line_of(cells: &[String], widths: &[usize]) -> String {
    let padded: Vec<String> = cells
        .iter()
        .zip(widths)
        .zip(COLUMNS.iter())
        .map(|((cell, width), column)| pad(cell, *width, column.right))
        .collect();
    format!("{ROW_START}{}", padded.join(COLUMN_GAP))
        .trim_end()
        .to_owned()
}

/// One cell, padded to the width of its column.
///
/// The measure is in terminal columns and not in bytes, so a name that holds a
/// wide glyph keeps its column.
fn pad(cell: &str, width: usize, right: bool) -> String {
    let spaces = " ".repeat(width.saturating_sub(ui::display_width(cell)));
    if right {
        return format!("{spaces}{cell}");
    }
    format!("{cell}{spaces}")
}

#[cfg(test)]
mod tests {
    use super::{
        random, record, reserved, Draw, Facts, HuntError, HuntStopped, PathKind, Plan, Probes,
        RunError, Score, Scorer, Sources, Summary, ATTEMPTS, FASTEST, LONGEST, NOTHING_TO_RANK,
        PARTIAL, SHORTEST, SLOWEST,
    };
    use crate::live::Screen;
    use crate::status::{Event, Status};
    use crate::record::{
        EndReason, Family, HuntId, NameRecord, Privilege, Record, Recording, RoundRecord,
        RunConfig, RunId, SourceKind, SourceLabel, Writer,
    };
    use crate::testing::{named, round};
    use crate::{Multipath, Protocol};
    use chrono::Utc;
    use std::collections::HashSet;
    use std::collections::VecDeque;
    use std::net::{IpAddr, Ipv4Addr};
    use std::rc::Rc;
    use std::time::Duration;
    use std::time::Instant;

    /// An address that a packet routes to, and that no test rejects.
    const ROUTABLE: &str = "93.184.216.34";

    /// A second address that a packet routes to.
    const OTHER_ROUTABLE: &str = "1.1.1.1";

    /// The number of addresses that the test of a seeded draw reads.
    const SEEDED_DRAWS: usize = 64;

    /// The seed of the hunt that a test repeats.
    const SEED: u64 = 12_345;

    /// A second seed, which draws another sequence.
    const OTHER_SEED: u64 = 54_321;

    /// Reads an address that a test names.
    fn address(text: &str) -> Ipv4Addr {
        text.parse().expect("the test address must parse")
    }

    /// The draw over a scripted list of candidates.
    fn draw_of(candidates: &[&str]) -> Draw {
        let list: Vec<Ipv4Addr> = candidates.iter().copied().map(address).collect();
        Draw::new(Box::new(list.into_iter()))
    }

    /// The address that a draw gives, after it reads the rejected candidates
    /// that stand in front of one routable address.
    ///
    /// Every test of a block reads this, so each of them proves the rejection
    /// through the door that the hunt uses and not through the guard alone.
    fn drawn_past(rejected: &[&str]) -> Option<Ipv4Addr> {
        let mut candidates = rejected.to_vec();
        candidates.push(ROUTABLE);
        draw_of(&candidates).address()
    }

    /// Asserts that the draw reads past every address of one reserved block.
    ///
    /// The list names the first address of the block and the last one, so the
    /// test covers both ends of the guard and not one address inside it.
    fn rejects(block: &str, addresses: &[&str]) {
        assert_eq!(
            drawn_past(addresses),
            Some(address(ROUTABLE)),
            "the draw must read past `{block}`, which no packet routes to"
        );
    }

    #[test]
    fn the_draw_gives_a_routable_address() {
        assert_eq!(draw_of(&[ROUTABLE]).address(), Some(address(ROUTABLE)));
    }

    #[test]
    fn the_draw_of_a_source_that_ran_out_gives_no_address() {
        assert_eq!(draw_of(&[]).address(), None);
    }

    #[test]
    fn the_draw_rejects_an_address_that_this_hunt_already_visited() {
        let mut draw = draw_of(&[ROUTABLE, ROUTABLE, OTHER_ROUTABLE]);
        assert_eq!(draw.address(), Some(address(ROUTABLE)));
        assert_eq!(
            draw.address(),
            Some(address(OTHER_ROUTABLE)),
            "the second ask must read past the address that the first ask gave"
        );
    }

    /// A source that gives nothing but rejected addresses stops the draw.
    ///
    /// The list is without end, so a draw that read it until a candidate
    /// passed would never answer at all.
    #[test]
    fn a_source_of_rejected_addresses_alone_stops_the_draw() {
        let loopback = address("127.0.0.1");
        let mut draw = Draw::new(Box::new(std::iter::repeat(loopback)));
        assert_eq!(draw.address(), None);
    }

    #[test]
    fn the_draw_reads_no_more_candidates_than_the_bound_of_the_attempts() {
        let mut read = 0;
        let counted = std::iter::repeat_with(move || {
            read += 1;
            assert!(read <= ATTEMPTS, "the draw read {read} candidates");
            address("10.0.0.1")
        });
        assert_eq!(Draw::new(Box::new(counted)).address(), None);
    }

    #[test]
    fn the_draw_rejects_the_addresses_of_this_network() {
        rejects("0.0.0.0/8", &["0.0.0.0", "0.255.255.255"]);
    }

    #[test]
    fn the_draw_rejects_the_private_addresses_of_ten() {
        rejects("10.0.0.0/8", &["10.0.0.0", "10.255.255.255"]);
    }

    #[test]
    fn the_draw_rejects_the_addresses_that_a_carrier_shares() {
        rejects("100.64.0.0/10", &["100.64.0.0", "100.127.255.255"]);
    }

    #[test]
    fn the_draw_rejects_the_loopback_addresses() {
        rejects("127.0.0.0/8", &["127.0.0.0", "127.255.255.255"]);
    }

    #[test]
    fn the_draw_rejects_the_link_local_addresses() {
        rejects("169.254.0.0/16", &["169.254.0.0", "169.254.255.255"]);
    }

    #[test]
    fn the_draw_rejects_the_private_addresses_of_a_hundred_seventy_two() {
        rejects("172.16.0.0/12", &["172.16.0.0", "172.31.255.255"]);
    }

    #[test]
    fn the_draw_rejects_the_addresses_that_the_ietf_holds() {
        rejects("192.0.0.0/24", &["192.0.0.0", "192.0.0.255"]);
    }

    #[test]
    fn the_draw_rejects_the_first_block_of_documentation() {
        rejects("192.0.2.0/24", &["192.0.2.0", "192.0.2.255"]);
    }

    #[test]
    fn the_draw_rejects_the_addresses_of_the_six_to_four_relay() {
        rejects("192.88.99.0/24", &["192.88.99.0", "192.88.99.255"]);
    }

    #[test]
    fn the_draw_rejects_the_private_addresses_of_a_hundred_ninety_two() {
        rejects("192.168.0.0/16", &["192.168.0.0", "192.168.255.255"]);
    }

    #[test]
    fn the_draw_rejects_the_addresses_of_a_benchmark() {
        rejects("198.18.0.0/15", &["198.18.0.0", "198.19.255.255"]);
    }

    #[test]
    fn the_draw_rejects_the_second_block_of_documentation() {
        rejects("198.51.100.0/24", &["198.51.100.0", "198.51.100.255"]);
    }

    #[test]
    fn the_draw_rejects_the_third_block_of_documentation() {
        rejects("203.0.113.0/24", &["203.0.113.0", "203.0.113.255"]);
    }

    #[test]
    fn the_draw_rejects_the_multicast_addresses() {
        rejects("224.0.0.0/4", &["224.0.0.0", "239.255.255.255"]);
    }

    #[test]
    fn the_draw_rejects_the_addresses_that_no_use_holds() {
        rejects("240.0.0.0/4", &["240.0.0.0", "255.255.255.254"]);
    }

    #[test]
    fn the_draw_rejects_the_broadcast_address() {
        rejects("255.255.255.255/32", &["255.255.255.255"]);
    }

    /// The block of the guard is the block that the table names.
    ///
    /// The draw reads the answer as a yes or a no, so a table whose entries
    /// were shifted by one bit would still reject the addresses that the tests
    /// above name. This test reads the block itself.
    #[test]
    fn the_guard_names_the_block_that_holds_the_address() {
        let found = reserved(address("172.20.1.1")).expect("the address stands in a block");
        assert_eq!(found.to_string(), "172.16.0.0/12");
    }

    #[test]
    fn the_guard_names_no_block_for_a_routable_address() {
        assert_eq!(reserved(address(ROUTABLE)), None);
    }

    #[test]
    fn a_draw_of_the_same_seed_visits_the_same_addresses_in_the_same_order() {
        assert_eq!(seeded_addresses(SEED), seeded_addresses(SEED));
    }

    #[test]
    fn a_draw_of_another_seed_visits_other_addresses() {
        assert_ne!(seeded_addresses(SEED), seeded_addresses(OTHER_SEED));
    }

    /// Every address that a seeded draw gives is one that a packet routes to.
    ///
    /// The tests of the blocks each hand the draw one address. This one reads
    /// the source of a real hunt, so it covers the pair of the source and the
    /// guard over many draws.
    #[test]
    fn every_address_of_a_seeded_draw_routes() {
        for addr in seeded_addresses(SEED) {
            assert_eq!(reserved(addr), None, "{addr} is an address of a block");
        }
    }

    #[test]
    fn a_seeded_draw_gives_no_address_twice() {
        let addresses = seeded_addresses(SEED);
        let distinct: HashSet<Ipv4Addr> = addresses.iter().copied().collect();
        assert_eq!(distinct.len(), addresses.len());
    }

    /// The source of a real hunt gives candidates without end.
    #[test]
    fn the_seeded_source_never_runs_out() {
        assert_eq!(random(SEED).take(SEEDED_DRAWS).count(), SEEDED_DRAWS);
    }

    /// The addresses that a seeded draw gives, in the order it gave them.
    fn seeded_addresses(seed: u64) -> Vec<Ipv4Addr> {
        let mut draw = Draw::seeded(seed);
        (0..SEEDED_DRAWS)
            .map(|_| draw.address().expect("the seeded draw never runs out"))
            .collect()
    }

    /// The identifier of the run that every scored trace of a test recorded.
    const RUN: &str = "2026-08-18T12:00:00.123Z";

    /// The first TTL that every scored trace of a test probes.
    const FIRST_TTL: u8 = 1;

    /// The last TTL that every scored trace of a test probes.
    const MAX_TTL: u8 = 20;

    /// The address of the destination of every scored trace of a test.
    const DESTINATION: &str = "93.184.216.34";

    /// The address of the first hop of every scored trace of a test.
    const FIRST_HOP: &str = "10.0.0.1";

    /// The address of the hop where a partial path of a test ends.
    const LAST_ANSWER: &str = "72.14.200.1";

    /// The name that a reverse lookup gives the destination of a test.
    const DESTINATION_NAME: &str = "example.com";

    /// The score of a trace whose rounds a test scripts.
    ///
    /// Every round reaches the screen through the door that the run loop uses,
    /// so the test covers the fold that a real trace drives.
    fn score_of(rounds: &[&[(u8, &str, f64)]]) -> Score {
        scored(rounds, &[])
    }

    /// The score of a trace whose rounds and names a test scripts.
    fn scored(rounds: &[&[(u8, &str, f64)]], names: &[(&str, &str)]) -> Score {
        traced(DESTINATION, RUN, rounds, names)
    }

    /// The score of one destination whose address, run, rounds, and names a
    /// test scripts.
    fn traced(
        destination: &str,
        run: &str,
        rounds: &[&[(u8, &str, f64)]],
        names: &[(&str, &str)],
    ) -> Score {
        let mut recorder = Recorder::default();
        let mut scorer = Scorer::new(
            address(destination),
            RunId::from(run),
            FIRST_TTL,
            &mut recorder,
        );
        for hops in rounds {
            scorer.round(&round(FIRST_TTL, MAX_TTL, hops));
        }
        let records: Vec<NameRecord> = names
            .iter()
            .map(|(addr, host)| NameRecord {
                run: RunId::from(run),
                ts: Utc::now(),
                addr: IpAddr::V4(address(addr)),
                host: (*host).to_owned(),
            })
            .collect();
        scorer.names(&records);
        scorer.score()
    }

    #[test]
    fn a_destination_that_answered_gives_a_reached_path() {
        let score = score_of(&[&[(1, FIRST_HOP, 1.0), (5, DESTINATION, 20.0)]]);
        assert_eq!(score.kind, PathKind::Reached);
    }

    #[test]
    fn the_length_of_a_reached_path_is_the_ttl_that_the_destination_answered_at() {
        let score = score_of(&[&[(1, FIRST_HOP, 1.0), (5, DESTINATION, 20.0)]]);
        assert_eq!(score.length, 5);
    }

    /// A destination that answers at two TTLs takes the smaller one.
    ///
    /// A path that changes under a load balancer reaches the destination at one
    /// TTL in one round and at another TTL in the next. The shorter of the two
    /// is the length of the path, because a packet did reach the destination in
    /// that many hops.
    #[test]
    fn a_destination_that_answered_at_two_ttls_takes_the_shorter_path() {
        let score = score_of(&[
            &[(6, DESTINATION, 20.0)],
            &[(5, DESTINATION, 21.0)],
            &[(6, DESTINATION, 22.0)],
        ]);
        assert_eq!(score.length, 5);
    }

    #[test]
    fn a_destination_that_answered_nothing_gives_a_partial_path() {
        let score = score_of(&[&[(1, FIRST_HOP, 1.0), (4, LAST_ANSWER, 9.0)]]);
        assert_eq!(score.kind, PathKind::Partial);
    }

    #[test]
    fn the_length_of_a_partial_path_is_the_highest_ttl_that_answered() {
        let score = score_of(&[&[(1, FIRST_HOP, 1.0), (4, LAST_ANSWER, 9.0)]]);
        assert_eq!(score.length, 4);
    }

    #[test]
    fn a_destination_that_nothing_answered_for_gives_a_partial_path_of_no_length() {
        let score = score_of(&[&[]]);
        assert_eq!((score.kind, score.length), (PathKind::Partial, 0));
    }

    /// The time of a score is the mean of the last hop that answered.
    ///
    /// The rounds below answer at the destination three times, and the mean of
    /// the three is the number that ranks the fastest path and the slowest one.
    #[test]
    fn the_time_of_a_score_is_the_mean_of_the_last_hop_that_answered() {
        let score = score_of(&[
            &[(5, DESTINATION, 10.0)],
            &[(5, DESTINATION, 20.0)],
            &[(5, DESTINATION, 30.0)],
        ]);
        assert_eq!(score.rtt_ms, Some(20.0));
    }

    #[test]
    fn the_time_of_a_partial_path_is_the_mean_of_the_hop_where_it_ends() {
        let score = score_of(&[
            &[(1, FIRST_HOP, 1.0), (4, LAST_ANSWER, 8.0)],
            &[(1, FIRST_HOP, 3.0), (4, LAST_ANSWER, 12.0)],
        ]);
        assert_eq!(score.rtt_ms, Some(10.0));
    }

    #[test]
    fn a_path_that_no_hop_answered_holds_no_time_and_no_loss() {
        let score = score_of(&[&[]]);
        assert_eq!((score.rtt_ms, score.loss), (None, None));
    }

    /// The loss of a score is the loss of the last hop that answered.
    ///
    /// The destination answers one of the two rounds below, so half of the
    /// probes of that hop were lost.
    #[test]
    fn the_loss_of_a_score_is_the_loss_of_the_last_hop_that_answered() {
        let score = score_of(&[&[(5, DESTINATION, 10.0)], &[(1, FIRST_HOP, 1.0)]]);
        assert_eq!(score.loss, Some(50.0));
    }

    /// The gaps count the TTLs inside the path that answered nothing.
    ///
    /// The round below answers at the TTLs 1 and 5, so the TTLs 2, 3, and 4
    /// stand inside the path and answered nothing.
    #[test]
    fn the_gaps_count_the_ttls_inside_the_path_that_answered_nothing() {
        let score = score_of(&[&[(1, FIRST_HOP, 1.0), (5, DESTINATION, 20.0)]]);
        assert_eq!(score.gaps, 3);
    }

    /// A TTL past the end of the path is no gap.
    ///
    /// The round below probes to TTL 8 and answers to TTL 4. The TTLs 5 through
    /// 8 stand past the end of the path, and the path holds two holes: the TTLs
    /// 2 and 3.
    #[test]
    fn a_ttl_past_the_end_of_the_path_counts_as_no_gap() {
        let score = score_of(&[&[(1, FIRST_HOP, 1.0), (4, LAST_ANSWER, 9.0)]]);
        assert_eq!(score.gaps, 2);
    }

    #[test]
    fn the_name_of_the_destination_reaches_the_score() {
        let score = scored(
            &[&[(5, DESTINATION, 20.0)]],
            &[(DESTINATION, DESTINATION_NAME)],
        );
        assert_eq!(score.host.as_deref(), Some(DESTINATION_NAME));
    }

    /// A run that read no name gives a score of no name.
    ///
    /// `--no-dns` writes no `name` record at all, so nothing reaches the screen
    /// and the row of the summary prints the address alone.
    #[test]
    fn a_score_of_a_run_that_read_no_name_holds_none() {
        let score = score_of(&[&[(5, DESTINATION, 20.0)]]);
        assert_eq!(score.host, None);
    }

    /// The name of a hop is no name of the destination.
    #[test]
    fn the_name_of_another_address_does_not_reach_the_score() {
        let score = scored(&[&[(5, DESTINATION, 20.0)]], &[(FIRST_HOP, "gateway.lan")]);
        assert_eq!(score.host, None);
    }

    #[test]
    fn the_score_names_the_run_that_recorded_the_trace() {
        assert_eq!(score_of(&[&[]]).run, RunId::from(RUN));
    }

    #[test]
    fn the_score_names_the_address_that_the_hunt_traced() {
        assert_eq!(score_of(&[&[]]).addr, address(DESTINATION));
    }

    /// A hunt takes no key of the terminal.
    #[test]
    fn the_screen_of_a_destination_asks_for_no_stop() {
        let mut recorder = Recorder::default();
        let mut scorer = Scorer::new(address(DESTINATION), RunId::from(RUN), FIRST_TTL, &mut recorder);
        assert!(!scorer.poll());
    }

    /// The address of the destination that holds the shortest reached path.
    const NEAR: &str = "93.184.216.34";

    /// The run that recorded the trace of the near destination.
    const NEAR_RUN: &str = "2026-08-18T12:00:00.123Z";

    /// The address of the destination that holds the longest reached path.
    const FAR: &str = "72.14.200.1";

    /// The run that recorded the trace of the far destination.
    const FAR_RUN: &str = "2026-08-18T12:01:00.000Z";

    /// The address of the destination that answered nothing.
    const QUIET: &str = "1.1.1.1";

    /// The run that recorded the trace of the quiet destination.
    const QUIET_RUN: &str = "2026-08-18T12:02:00.000Z";

    /// The time that the hunt of every summary test took.
    const ELAPSED: Duration = Duration::from_secs(192);

    /// The summary of a hunt of three destinations, as the tests read it.
    ///
    /// The near destination answered at TTL 5 and the far one at TTL 18, so the
    /// two of them hold the four rows of a summary that ranks the reached paths
    /// alone. The quiet destination answered nothing past TTL 4, and it holds a
    /// path shorter and faster than either of them, so a summary that lets a
    /// partial path compete gives it the shortest row and the fastest one.
    fn a_hunt(include_partial: bool) -> Summary {
        let scores = vec![
            traced(
                NEAR,
                NEAR_RUN,
                &[&[(1, FIRST_HOP, 1.0), (5, NEAR, 20.0)]],
                &[(NEAR, DESTINATION_NAME)],
            ),
            traced(
                FAR,
                FAR_RUN,
                &[&[(1, FIRST_HOP, 1.0), (18, FAR, 85.0)]],
                &[],
            ),
            traced(
                QUIET,
                QUIET_RUN,
                &[&[(1, FIRST_HOP, 1.0), (4, LAST_ANSWER, 9.0)]],
                &[],
            ),
        ];
        Summary::new(scores, ELAPSED, include_partial)
    }

    /// The row of the summary that carries one label.
    fn row(summary: &Summary, label: &str) -> String {
        summary
            .lines()
            .into_iter()
            .find(|line| line.trim_start().starts_with(label))
            .unwrap_or_else(|| panic!("the summary must hold the `{label}` row"))
    }

    /// The last line of the summary, which counts what the hunt did.
    fn counts(summary: &Summary) -> String {
        summary
            .lines()
            .last()
            .cloned()
            .expect("the summary must hold the line that counts the hunt")
    }

    #[test]
    fn the_shortest_row_names_the_destination_of_the_shortest_reached_path() {
        assert!(row(&a_hunt(false), SHORTEST).contains(NEAR));
    }

    #[test]
    fn the_longest_row_names_the_destination_of_the_longest_reached_path() {
        assert!(row(&a_hunt(false), LONGEST).contains(FAR));
    }

    #[test]
    fn the_fastest_row_names_the_destination_of_the_smallest_mean_time() {
        assert!(row(&a_hunt(false), FASTEST).contains(NEAR));
    }

    #[test]
    fn the_slowest_row_names_the_destination_of_the_largest_mean_time() {
        assert!(row(&a_hunt(false), SLOWEST).contains(FAR));
    }

    /// A partial path competes for no row of a summary that did not ask for it.
    ///
    /// The quiet destination holds the shortest path and the fastest one of the
    /// three, and neither row names it.
    #[test]
    fn a_partial_path_competes_for_no_row_by_default() {
        let summary = a_hunt(false);
        assert!(!row(&summary, SHORTEST).contains(QUIET));
        assert!(!row(&summary, FASTEST).contains(QUIET));
    }

    #[test]
    fn a_partial_path_that_the_hunt_included_takes_the_shortest_row() {
        assert!(row(&a_hunt(true), SHORTEST).contains(QUIET));
    }

    #[test]
    fn a_partial_path_that_the_hunt_included_takes_the_fastest_row() {
        assert!(row(&a_hunt(true), FASTEST).contains(QUIET));
    }

    #[test]
    fn the_row_of_a_partial_path_says_that_the_path_is_partial() {
        assert!(row(&a_hunt(true), SHORTEST).contains(PARTIAL));
    }

    #[test]
    fn the_row_of_a_reached_path_says_that_the_path_is_reached() {
        assert!(row(&a_hunt(false), SHORTEST).contains("reached"));
    }

    /// One destination holds more than one row.
    #[test]
    fn a_destination_that_holds_two_rows_stands_in_both_of_them() {
        let summary = a_hunt(false);
        assert!(row(&summary, SHORTEST).contains(NEAR));
        assert!(row(&summary, FASTEST).contains(NEAR));
    }

    #[test]
    fn a_row_names_the_run_that_recorded_the_trace() {
        assert!(row(&a_hunt(false), SHORTEST).contains(NEAR_RUN));
    }

    #[test]
    fn a_row_names_the_name_of_the_address_when_a_lookup_gave_one() {
        assert!(row(&a_hunt(false), SHORTEST).contains(DESTINATION_NAME));
    }

    /// A row of an address that no lookup named holds the address alone.
    #[test]
    fn a_row_of_an_address_that_no_lookup_named_holds_the_address_alone() {
        let line = row(&a_hunt(false), LONGEST);
        assert!(line.contains(FAR));
        assert!(!line.contains('('), "the row holds no name: {line}");
    }

    #[test]
    fn the_counts_hold_the_rounds_the_reached_the_partial_and_the_wall_time() {
        assert_eq!(
            counts(&a_hunt(false)),
            "3 rounds   2 reached   1 partial   192s"
        );
    }

    /// A hunt of one round writes the singular name of a round.
    #[test]
    fn the_counts_of_one_round_name_that_round_in_the_singular() {
        let scores = vec![traced(NEAR, NEAR_RUN, &[&[(5, NEAR, 20.0)]], &[])];
        let summary = Summary::new(scores, ELAPSED, false);
        assert!(counts(&summary).starts_with("1 round "));
    }

    /// A hunt whose destinations all answered nothing ranks no path.
    ///
    /// The table then says so, and the counts still print: a hunt that reached
    /// nothing still tells the reader how many rounds it spent.
    #[test]
    fn a_summary_of_no_ranked_path_says_so_and_still_counts_the_hunt() {
        let scores = vec![traced(QUIET, QUIET_RUN, &[&[(1, FIRST_HOP, 1.0)]], &[])];
        let summary = Summary::new(scores, ELAPSED, false);
        let lines = summary.lines();
        assert!(
            lines.iter().any(|line| line.contains(NOTHING_TO_RANK)),
            "the summary must say that it ranked nothing: {lines:?}"
        );
        assert_eq!(counts(&summary), "1 round   0 reached   1 partial   192s");
    }

    /// The summary of a hunt reads as the table of the design.
    ///
    /// Every column of the table lines up under its heading, the numbers stand
    /// against the right edge of their columns, and the counts stand under a
    /// blank line.
    #[test]
    fn the_summary_reads_as_the_table_of_the_design() {
        assert_eq!(a_hunt(false).lines(), GOLDEN_SUMMARY);
    }

    /// The summary of a hunt of three destinations, as the design writes it.
    const GOLDEN_SUMMARY: [&str; 7] = [
        " Row       Host                         Len  Path      Avg  Loss%  Gaps  Run",
        " shortest  example.com (93.184.216.34)    5  reached  20.0   0.0%     3  2026-08-18T12:00:00.123Z",
        " longest   72.14.200.1                   18  reached  85.0   0.0%    16  2026-08-18T12:01:00.000Z",
        " fastest   example.com (93.184.216.34)    5  reached  20.0   0.0%     3  2026-08-18T12:00:00.123Z",
        " slowest   72.14.200.1                   18  reached  85.0   0.0%    16  2026-08-18T12:01:00.000Z",
        "",
        "3 rounds   2 reached   1 partial   192s",
    ];
    /// The identifier of the hunt that every test of the loop makes.
    const HUNT_ID: &str = "2026-08-18T11:59:00.000Z";

    /// The build string of the `krt` that makes a test hunt.
    const KRT: &str = "0.1.0 (abc1234, clean)";

    /// The name of the machine that makes a test hunt.
    const HOST: &str = "tims-mac";

    /// The address that the probes of a test hunt leave from.
    const SOURCE: &str = "1.2.3.4";

    /// The longest that one destination of a test hunt takes.
    ///
    /// The value is short, because a test that waited the real timeout would
    /// hold the suite for ten seconds for each destination whose rounds stop
    /// arriving.
    const TARGET_TIMEOUT: Duration = Duration::from_millis(20);

    /// The hops of one round that a test scripts: the TTL of a hop, the
    /// address that answered at it, and the round-trip time of that answer.
    type Hops<'a> = &'a [(u8, &'a str, f64)];

    /// The rounds of one destination that a test scripts.
    type Rounds<'a> = &'a [Hops<'a>];

    /// A source of rounds that a test scripts.
    ///
    /// The fake stamps every scripted round with the run that the hunt made, so
    /// the file groups the rounds of one destination under that run, as a real
    /// tracer does. It keeps every sender alive, so no channel closes and no
    /// run reads a closed channel as a tracer that died.
    struct FakeProbes {
        /// The rounds of each destination, the next destination first.
        scripts: VecDeque<Vec<RoundRecord>>,
        /// The destinations that the hunt asked for, in order.
        asked: Vec<Ipv4Addr>,
        /// The senders of the channels, held so that none of them closes.
        senders: Vec<std::sync::mpsc::Sender<RoundRecord>>,
        /// The reason that a start gives, when the tracer must fail.
        refuses: Option<String>,
        /// The number of destinations that the source serves before it
        /// refuses. A source that refuses nothing never reads it.
        serves: usize,
    }

    impl FakeProbes {
        /// A source that hands each destination the rounds of its script.
        fn of(scripts: &[Rounds]) -> Self {
            Self {
                scripts: scripts
                    .iter()
                    .map(|rounds| {
                        rounds
                            .iter()
                            .map(|hops| round(FIRST_TTL, MAX_TTL, hops))
                            .collect()
                    })
                    .collect(),
                asked: Vec::new(),
                senders: Vec::new(),
                refuses: None,
                serves: 0,
            }
        }

        /// A source whose tracer never starts.
        fn that_refuses(reason: &str) -> Self {
            Self::refuses_after(&[], 0, reason)
        }

        /// A source that serves this many destinations and then refuses.
        ///
        /// The scripts are the rounds of the destinations that it serves. A
        /// hunt of more destinations than that reads the refusal on the one
        /// that follows them.
        fn refuses_after(scripts: &[Rounds], serves: usize, reason: &str) -> Self {
            let mut probes = Self::of(scripts);
            probes.refuses = Some(reason.to_owned());
            probes.serves = serves;
            probes
        }
    }

    impl Probes for FakeProbes {
        fn start(
            &mut self,
            target: Ipv4Addr,
            run: &RunId,
        ) -> Result<std::sync::mpsc::Receiver<RoundRecord>, String> {
            self.asked.push(target);
            if let Some(reason) = &self.refuses {
                // The push above counts this destination, so the length is the
                // number of the destination that stands.
                if self.asked.len() > self.serves {
                    return Err(reason.clone());
                }
            }
            let (sender, receiver) = std::sync::mpsc::channel();
            for mut record in self.scripts.pop_front().unwrap_or_default() {
                record.run = run.clone();
                sender.send(record).expect("the receiver stands");
            }
            self.senders.push(sender);
            Ok(receiver)
        }
    }

    /// What one test hunt wrote, and what it found.
    struct Hunted {
        /// The summary that the hunt printed.
        summary: Summary,
        /// The records that the hunt wrote, as one recorded file reads them.
        recording: Recording,
        /// The destinations that the hunt asked its source for, in order.
        asked: Vec<Ipv4Addr>,
        /// The events that the hunt showed its indicator, in order.
        shown: Vec<Event>,
    }

    /// An indicator that keeps every event the hunt showed it.
    ///
    /// A hunt of a test drives this one, so no test of this module needs a
    /// terminal and every test reads the events as values.
    #[derive(Default)]
    struct Recorder {
        /// The events, in the order the hunt showed them.
        events: Vec<Event>,
    }

    impl Recorder {
        /// The destinations that the hunt named, in order.
        fn targets(&self) -> Vec<Ipv4Addr> {
            self.events
                .iter()
                .filter_map(|event| match event {
                    Event::Target(target) => Some(*target),
                    _ => None,
                })
                .collect()
        }

        /// Whether each destination that the hunt scored answered, in order.
        fn answers(&self) -> Vec<bool> {
            self.events
                .iter()
                .filter_map(|event| match event {
                    Event::Scored { reached } => Some(*reached),
                    _ => None,
                })
                .collect()
        }
    }

    impl Status for Recorder {
        fn show(&mut self, event: &Event) {
            self.events.push(*event);
        }
    }

    /// Runs one hunt over a scripted draw and a scripted source of rounds.
    ///
    /// `stop` answers whether the user asked the hunt to stop, as the flag of
    /// the signal handler does in a run of the command line.
    fn hunted(
        addresses: &[&str],
        scripts: &[Rounds],
        rounds: u64,
        stop: &dyn Fn() -> bool,
    ) -> Result<Hunted, HuntStopped> {
        let mut probes = FakeProbes::of(scripts);
        let mut recorder = Recorder::default();
        let outcome = run_hunt(addresses, &mut probes, rounds, stop, &[], &mut recorder);
        outcome.map(|(summary, recording)| Hunted {
            summary,
            recording,
            asked: probes.asked.clone(),
            shown: recorder.events.clone(),
        })
    }

    /// Runs one hunt into a sink of bytes, and reads back the file it wrote.
    fn run_hunt(
        addresses: &[&str],
        probes: &mut FakeProbes,
        rounds: u64,
        stop: &dyn Fn() -> bool,
        names: &[(&str, &[crate::names::Lookup])],
        status: &mut dyn Status,
    ) -> Result<(Summary, Recording), HuntStopped> {
        let mut sink = Vec::new();
        let summary = {
            let mut writer = Writer::to_sink(&mut sink);
            hunt_into(addresses, probes, rounds, stop, names, &mut writer, status)
        }?;
        Ok((summary, read_back(&sink)))
    }

    /// Runs one hunt into the writer that a test names.
    ///
    /// The sink of that writer is what a test of a write that fails hands in.
    /// Every other test takes [`run_hunt`], which reads its bytes back.
    fn hunt_into<W: std::io::Write>(
        addresses: &[&str],
        probes: &mut FakeProbes,
        rounds: u64,
        stop: &dyn Fn() -> bool,
        names: &[(&str, &[crate::names::Lookup])],
        writer: &mut Writer<W>,
        status: &mut dyn Status,
    ) -> Result<Summary, HuntStopped> {
        let list: Vec<Ipv4Addr> = addresses.iter().copied().map(address).collect();
        let facts = Facts {
            id: HuntId::from(HUNT_ID),
            krt: KRT.to_owned(),
            source: SourceLabel {
                addr: IpAddr::V4(address(SOURCE)),
                kind: SourceKind::Local,
            },
            config: RunConfig {
                interval_ms: 1000,
                protocol: Protocol::Icmp,
                first_ttl: FIRST_TTL,
                max_ttl: MAX_TTL,
                multipath: Multipath::Classic,
                privilege: Privilege::Unprivileged,
                dns: !names.is_empty(),
            },
            host: HOST.to_owned(),
        };
        let plan = Plan {
            rounds,
            probes_per_round: 1,
            target_timeout: TARGET_TIMEOUT,
            name_grace: Duration::ZERO,
            include_partial: false,
        };
        let resolver: Rc<dyn crate::names::Resolver> = if names.is_empty() {
            Rc::new(crate::names::NoLookups)
        } else {
            crate::testing::FakeResolver::new(names)
        };
        let mut sources = Sources {
            draw: Draw::new(Box::new(list.into_iter())),
            probes,
            resolver,
        };
        record(&facts, &plan, &mut sources, stop, writer, status)
    }

    /// The byte that ends every record of a file.
    const NEWLINE: u8 = b'\n';

    /// The reason that a sink which took its fill gives.
    const THE_SINK_IS_FULL: &str = "the sink is full";

    /// A sink that takes a number of records and then fails every write.
    ///
    /// A file fails a write when the disk fills or the device goes away, and no
    /// test makes either one happen. This sink makes that fault on demand, as
    /// the sink of `run.rs` does for one run.
    struct Sink {
        /// The number of records that the sink takes before it fails.
        takes: usize,
        /// The number of whole records that reached the sink. Every record ends
        /// with one newline.
        written: usize,
    }

    impl Sink {
        /// A sink that takes this many records.
        const fn that_takes(takes: usize) -> Self {
            Self { takes, written: 0 }
        }
    }

    impl std::io::Write for Sink {
        /// Takes the bytes of a record, until the sink holds the number of
        /// records that it takes.
        #[allow(
            clippy::naive_bytecount,
            reason = "the sink of a test holds a few hundred bytes, which is no reason to take the bytecount crate as a dependency"
        )]
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.written >= self.takes {
                return Err(std::io::Error::other(THE_SINK_IS_FULL));
            }
            self.written += buf.iter().filter(|byte| **byte == NEWLINE).count();
            Ok(buf.len())
        }

        /// The sink counts the bytes and keeps none, so it flushes nothing.
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Reads the records that a hunt wrote back out of the sink.
    fn read_back(sink: &[u8]) -> Recording {
        let text = String::from_utf8(sink.to_vec()).expect("the file holds text");
        let path = scratch_file();
        std::fs::write(&path, text).expect("the scratch file must take the records");
        let recording = Recording::read(&path).expect("the records must read back");
        std::fs::remove_file(&path).ok();
        recording
    }

    /// The path of a scratch file that no other run of this test touches.
    ///
    /// Two copies of one test run at the same time under `cargo test`, so the
    /// name carries the process and the moment.
    fn scratch_file() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        std::env::temp_dir().join(format!("krt-hunt-{}-{nanos}.jsonl", std::process::id()))
    }

    /// The events that a hunt showed its indicator, as a recorder reads them.
    fn shown(hunted: &Hunted) -> Recorder {
        Recorder {
            events: hunted.shown.clone(),
        }
    }

    /// A hunt that nothing stops.
    fn never_stops() -> impl Fn() -> bool {
        || false
    }

    /// One round that reached the destination at TTL 5.
    const REACHED_AT_FIVE: &[&[(u8, &str, f64)]] = &[&[(1, FIRST_HOP, 1.0), (5, NEAR, 20.0)]];

    /// One round that reached the far destination at TTL 18.
    ///
    /// The hops of a script name the address that answered, and a score reads
    /// the destination it traced. So a hunt of two destinations that both
    /// answer takes one script for each of them.
    const FAR_REACHED_AT_EIGHTEEN: &[&[(u8, &str, f64)]] =
        &[&[(1, FIRST_HOP, 1.0), (18, FAR, 85.0)]];

    /// One round that answered to TTL 4 and no further.
    const PARTIAL_AT_FOUR: &[&[(u8, &str, f64)]] = &[&[(1, FIRST_HOP, 1.0), (4, LAST_ANSWER, 9.0)]];

    #[test]
    fn a_hunt_shows_the_destination_of_each_round_to_its_indicator() {
        let hunted = hunted(
            &[NEAR, FAR],
            &[REACHED_AT_FIVE, FAR_REACHED_AT_EIGHTEEN],
            2,
            &never_stops(),
        )
        .expect("the hunt must finish");
        assert_eq!(
            shown(&hunted).targets(),
            vec![address(NEAR), address(FAR)],
            "the indicator must hear every destination: {:?}",
            hunted.shown
        );
    }

    #[test]
    fn a_hunt_shows_whether_each_destination_answered() {
        let hunted = hunted(
            &[NEAR, QUIET],
            &[REACHED_AT_FIVE, &[&[]]],
            2,
            &never_stops(),
        )
        .expect("the hunt must finish");
        assert_eq!(
            shown(&hunted).answers(),
            vec![true, false],
            "the indicator must hear the answer of every destination: {:?}",
            hunted.shown
        );
    }

    /// The trace of one destination is the heartbeat of the hunt that holds it.
    #[test]
    fn the_screen_of_a_destination_ticks_the_indicator() {
        let mut recorder = Recorder::default();
        {
            let mut scorer =
                Scorer::new(address(DESTINATION), RunId::from(RUN), FIRST_TTL, &mut recorder);
            scorer.poll();
            scorer.round(&round(FIRST_TTL, MAX_TTL, &[(1, FIRST_HOP, 1.0)]));
        }
        assert_eq!(
            recorder.events,
            vec![Event::Tick, Event::Tick],
            "a poll and a round must each tick the indicator"
        );
    }

    #[test]
    fn a_hunt_stops_its_indicator_when_it_ends() {
        let hunted =
            hunted(&[NEAR], &[REACHED_AT_FIVE], 1, &never_stops()).expect("the hunt must finish");
        assert_eq!(
            hunted.shown.last(),
            Some(&Event::Stop),
            "the last event of a hunt must take the line back: {:?}",
            hunted.shown
        );
    }

    #[test]
    fn a_hunt_that_a_fault_stopped_stops_its_indicator() {
        let mut probes = FakeProbes::of(&[REACHED_AT_FIVE, REACHED_AT_FIVE]);
        let mut writer = Writer::to_sink(Sink::that_takes(RECORDS_OF_ONE_DESTINATION));
        let mut recorder = Recorder::default();
        drop(
            hunt_into(
                &[NEAR, FAR],
                &mut probes,
                2,
                &never_stops(),
                &[],
                &mut writer,
                &mut recorder,
            )
            .expect_err("a write that fails stops the hunt"),
        );
        assert_eq!(
            recorder.events.last(),
            Some(&Event::Stop),
            "a fault must take the line back too: {:?}",
            recorder.events
        );
    }

    #[test]
    fn a_hunt_traces_the_addresses_that_the_draw_gives_in_the_order_it_gives_them() {
        let hunted = hunted(
            &[NEAR, FAR],
            &[REACHED_AT_FIVE, REACHED_AT_FIVE],
            2,
            &never_stops(),
        )
        .expect("the hunt must finish");
        assert_eq!(hunted.asked, vec![address(NEAR), address(FAR)]);
    }

    #[test]
    fn a_hunt_stops_after_the_number_of_rounds_that_the_plan_names() {
        let hunted = hunted(
            &[NEAR, FAR, QUIET],
            &[REACHED_AT_FIVE, REACHED_AT_FIVE, REACHED_AT_FIVE],
            2,
            &never_stops(),
        )
        .expect("the hunt must finish");
        assert_eq!(hunted.asked.len(), 2);
    }

    #[test]
    fn a_hunt_stops_when_the_draw_runs_out_of_addresses() {
        let hunted =
            hunted(&[NEAR], &[REACHED_AT_FIVE], 8, &never_stops()).expect("the hunt must finish");
        assert_eq!(hunted.asked.len(), 1);
    }

    #[test]
    fn each_destination_of_a_hunt_writes_one_run_into_the_file() {
        let hunted = hunted(
            &[NEAR, FAR],
            &[REACHED_AT_FIVE, REACHED_AT_FIVE],
            2,
            &never_stops(),
        )
        .expect("the hunt must finish");
        assert_eq!(hunted.recording.run_ids().len(), 2);
    }

    /// The run record of each destination names the hunt that holds it.
    #[test]
    fn the_run_record_of_each_destination_names_the_hunt() {
        let hunted = hunted(
            &[NEAR, FAR],
            &[REACHED_AT_FIVE, REACHED_AT_FIVE],
            2,
            &never_stops(),
        )
        .expect("the hunt must finish");
        let hunts: Vec<Option<HuntId>> = hunted
            .recording
            .records()
            .iter()
            .filter_map(|record| match record {
                Record::Run(start) => Some(start.hunt.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            hunts,
            vec![Some(HuntId::from(HUNT_ID)), Some(HuntId::from(HUNT_ID))]
        );
    }

    /// The run record of a destination names the address that the hunt drew.
    #[test]
    fn the_run_record_of_a_destination_names_the_address_that_the_hunt_drew() {
        let hunted =
            hunted(&[NEAR], &[REACHED_AT_FIVE], 1, &never_stops()).expect("the hunt must finish");
        let start = hunted
            .recording
            .records()
            .iter()
            .find_map(|record| match record {
                Record::Run(start) => Some(start.clone()),
                _ => None,
            })
            .expect("the file holds the record that opens the run");
        assert_eq!(start.target.addr, IpAddr::V4(address(NEAR)));
        assert_eq!(start.target.arg, NEAR);
        assert_eq!(start.target.family, Family::Ipv4);
    }

    /// A destination takes the number of probe rounds that the plan names.
    #[test]
    fn a_destination_takes_the_number_of_probe_rounds_that_the_plan_names() {
        let mut probes =
            FakeProbes::of(&[&[&[(5, NEAR, 20.0)], &[(5, NEAR, 21.0)], &[(5, NEAR, 22.0)]]]);
        let (_, recording) = run_hunt(
            &[NEAR],
            &mut probes,
            1,
            &never_stops(),
            &[],
            &mut Recorder::default(),
        )
        .expect("the hunt must finish");
        let run = recording.last_run().expect("the file holds one run");
        assert_eq!(run.rounds().len(), 1, "the plan names one probe round");
    }

    /// A destination whose rounds stop arriving stops on the deadline.
    ///
    /// The tracer of this destination sends no round at all, so the round limit
    /// of the run never counts down and the deadline is the one limit that
    /// stops it. A real tracer sends the rounds of a quiet destination as it
    /// sends the rounds of any other one, with a lost probe in the place of
    /// each answer, so the round limit stops that destination first. The
    /// deadline covers the tracer that stops giving rounds.
    #[test]
    fn a_destination_whose_rounds_stop_arriving_stops_on_the_deadline() {
        let started = Instant::now();
        let hunted = hunted(&[QUIET], &[&[]], 1, &never_stops()).expect("the hunt must finish");
        assert!(
            started.elapsed() < TARGET_TIMEOUT * 20,
            "the destination held the hunt for {:?}",
            started.elapsed()
        );
        let end = hunted
            .recording
            .last_run()
            .expect("the file holds one run")
            .end()
            .expect("the run closed")
            .reason;
        assert_eq!(end, EndReason::Duration);
    }

    /// `Ctrl-C` stops the hunt, and the summary counts the rounds that
    /// finished.
    #[test]
    fn a_hunt_that_the_user_stopped_counts_the_rounds_that_finished() {
        let hunted = hunted(
            &[NEAR, FAR],
            &[REACHED_AT_FIVE, REACHED_AT_FIVE],
            2,
            &|| true,
        )
        .expect("the hunt must finish");
        assert!(hunted.asked.is_empty(), "the hunt traced no destination");
        assert_eq!(
            counts(&hunted.summary),
            "0 rounds   0 reached   0 partial   0ms"
        );
    }

    /// The summary of a hunt reads the destinations that the hunt traced.
    #[test]
    fn the_summary_of_a_hunt_ranks_the_destinations_that_it_traced() {
        let hunted = hunted(
            &[NEAR, QUIET],
            &[REACHED_AT_FIVE, PARTIAL_AT_FOUR],
            2,
            &never_stops(),
        )
        .expect("the hunt must finish");
        assert!(row(&hunted.summary, SHORTEST).contains(NEAR));
        assert!(counts(&hunted.summary).contains("1 reached"));
        assert!(counts(&hunted.summary).contains("1 partial"));
    }

    /// A tracer that will not start stops the hunt and names the destination.
    #[test]
    fn a_tracer_that_does_not_start_stops_the_hunt_and_names_the_destination() {
        let mut probes = FakeProbes::that_refuses(NO_RAW_SOCKET);
        let stopped = run_hunt(
            &[NEAR],
            &mut probes,
            1,
            &never_stops(),
            &[],
            &mut Recorder::default(),
        )
            .expect_err("a tracer that will not start stops the hunt");
        let reason = stopped.fault.to_string();
        assert!(
            reason.contains(NEAR),
            "the reason names the address: {reason}"
        );
        assert!(
            reason.contains(NO_RAW_SOCKET),
            "the reason names the fault: {reason}"
        );
    }

    /// The reason that the tracer of a test gives when it will not start.
    const NO_RAW_SOCKET: &str = "no raw socket";

    /// The number of records that one destination of a test hunt writes: the
    /// record that opens the run, one round, and the record that closes it.
    const RECORDS_OF_ONE_DESTINATION: usize = 3;

    /// A tracer that will not start keeps the summary of the rounds in front of
    /// it.
    ///
    /// A hunt of 64 destinations whose tracer stops at round 40 measured 39
    /// paths, and the reader who asked for the hunt wants them. The runs stand
    /// in the file, so a reader who loses the summary reads them back with one
    /// `krt replay <file> --run <id>` for each of them.
    #[test]
    fn a_hunt_that_a_tracer_stopped_keeps_the_summary_of_the_rounds_that_finished() {
        let mut probes = FakeProbes::refuses_after(
            &[REACHED_AT_FIVE, FAR_REACHED_AT_EIGHTEEN],
            2,
            NO_RAW_SOCKET,
        );
        let stopped = run_hunt(
            &[NEAR, FAR, QUIET],
            &mut probes,
            3,
            &never_stops(),
            &[],
            &mut Recorder::default(),
        )
            .expect_err("the tracer of the third destination stops the hunt");
        assert!(
            stopped.fault.to_string().contains(QUIET),
            "the fault names the destination that stopped the hunt: {}",
            stopped.fault
        );
        // The wall time of the hunt stays out of the assertion, because the two
        // runs take whatever the machine gives them.
        let counts = counts(&stopped.summary);
        assert!(
            counts.starts_with("2 rounds   2 reached   0 partial"),
            "the summary counts the two rounds that finished: {counts}"
        );
    }

    /// A write that fails keeps the summary of the rounds in front of it.
    ///
    /// The disk that fills is the fault that this covers, and it is the one
    /// where the reader most wants the rounds that the hunt already measured.
    #[test]
    fn a_hunt_that_a_write_stopped_keeps_the_summary_of_the_rounds_that_finished() {
        let mut probes = FakeProbes::of(&[REACHED_AT_FIVE, REACHED_AT_FIVE]);
        let mut writer = Writer::to_sink(Sink::that_takes(RECORDS_OF_ONE_DESTINATION));
        let stopped = hunt_into(
            &[NEAR, FAR],
            &mut probes,
            2,
            &never_stops(),
            &[],
            &mut writer,
            &mut Recorder::default(),
        )
        .expect_err("a write that fails stops the hunt");
        assert!(
            matches!(stopped.fault, HuntError::Run(RunError::Write(_))),
            "the fault is the write that failed: {}",
            stopped.fault
        );
        let counts = counts(&stopped.summary);
        assert!(
            counts.starts_with("1 round   1 reached   0 partial"),
            "the summary counts the one round that finished: {counts}"
        );
    }

    /// The name of an address of a destination reaches the file.
    #[test]
    fn the_name_of_an_address_of_a_destination_reaches_the_file() {
        let mut probes = FakeProbes::of(&[REACHED_AT_FIVE]);
        let (_, recording) = run_hunt(
            &[NEAR],
            &mut probes,
            1,
            &never_stops(),
            &[(NEAR, &[named(DESTINATION_NAME)])],
            &mut Recorder::default(),
        )
        .expect("the hunt must finish");
        let named: Vec<String> = recording
            .records()
            .iter()
            .filter_map(|record| match record {
                Record::Name(name) => Some(name.host.clone()),
                _ => None,
            })
            .collect();
        assert!(
            named.contains(&DESTINATION_NAME.to_owned()),
            "the file holds the name of the destination: {named:?}"
        );
    }

    /// Two destinations of one hunt take two run identifiers, in order.
    ///
    /// A hunt traces two destinations inside one millisecond whenever both of
    /// them answer at once, and a run identifier holds the moment to the
    /// millisecond. Two runs of one identifier would leave `krt replay
    /// <file> --run <id>` unable to fold either one.
    #[test]
    fn every_destination_of_a_hunt_takes_its_own_run_identifier_in_order() {
        let hunted = hunted(
            &[NEAR, FAR, QUIET],
            &[REACHED_AT_FIVE, REACHED_AT_FIVE, REACHED_AT_FIVE],
            3,
            &never_stops(),
        )
        .expect("the hunt must finish");
        let ids = hunted.recording.run_ids();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            ids, sorted,
            "the runs of a hunt read in the order it traced them"
        );
        assert_eq!(ids.len(), 3);
    }

    /// A peek gives the address that the next ask gives.
    #[test]
    fn a_peek_gives_the_address_that_the_next_ask_gives() {
        let mut draw = draw_of(&[ROUTABLE, OTHER_ROUTABLE]);
        assert_eq!(draw.peek(), Some(address(ROUTABLE)));
        assert_eq!(draw.address(), Some(address(ROUTABLE)));
        assert_eq!(draw.address(), Some(address(OTHER_ROUTABLE)));
    }

    /// A peek of a draw that ran out gives no address.
    #[test]
    fn a_peek_of_a_draw_that_ran_out_gives_no_address() {
        assert_eq!(draw_of(&[]).peek(), None);
    }
}
