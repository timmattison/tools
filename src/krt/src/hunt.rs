//! The hunt: many destinations, one file, and the table that ranks them.
//!
//! A hunt draws an address, traces it, scores the path it found, and takes the
//! next round. It stops when it holds the rounds it wants, and it prints one
//! table of four rows: the shortest path, the longest path, the fastest path,
//! and the slowest path.
//!
//! The word `round` carries two meanings around this module, and the two names
//! keep them apart. A round of the hunt is one destination that answered, and
//! `Plan::rounds` counts those. A probe round is one sweep of the TTLs, and
//! `Plan::probes_per_round` counts the probe rounds that each destination
//! takes. The run loop of `run.rs` knows the second meaning alone.
//!
//! A destination that answered nothing costs no round. Most of the address
//! space answers nothing, so a hunt that counted every draw would spend itself
//! on addresses that measure no path at all. The hunt gives up after
//! `Bounds::max_targets` destinations, answered or not, because the draw of a
//! real hunt never runs out.
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
//! into the numbers that the table ranks: a hunt shows one table at the end and
//! no live table. The fold therefore rides on the door that the run loop
//! already knocks on, and the hunt reads no file back.
//!
//! A fault of one destination stops the hunt, and the hunt then writes the
//! record that closes each destination that stood beside it. Every run of the
//! file therefore holds the record that closes it, and a reader tells a hunt
//! that a fault stopped from a file that stops in the middle.
//!
//! The [`Scorer`] also ticks the indicator of `status.rs` on every turn of that
//! run loop, because the turns of one destination are the heartbeat of the
//! whole hunt. The loop of the hunt names the destination of each round, the
//! answer of each one, and the stop. The hunt itself draws nothing and knows
//! nothing about which look the indicator wears.
//!
//! The hunt traces `Plan::concurrency` destinations at one moment. It starts
//! that many at once, and it starts another one each time one of them stops, so
//! the time that a destination which answers nothing costs is time the hunt
//! spends on the other destinations of the pool. Most of the address space
//! answers nothing, so that time is most of the time a hunt takes.
//!
//! Each destination of the pool probes in a lane of its own, which is what
//! keeps two tracers of one process from reading each other's answers.
//! `trace::Lane` holds that fact, and it holds the ceiling of the pool with it.
//!
//! The whole hunt still stands on one thread. Every tracer already runs on a
//! thread of its own and hands its rounds over a channel, so the hunt sweeps
//! the channels of the pool, one turn of each, and the file, the indicator, and
//! the resolver stay where they were.
//!
//! No turn of that sweep waits for anything. A destination that closes still
//! waits for the names of its hops, and it takes one step of that wait on each
//! sweep. It keeps its lane until the wait ends, so the hunt starts the next
//! destination one wait later than it otherwise would. It holds up no
//! destination that already stands: every other flight of the pool records its
//! rounds, reads its own deadline, and reaches the indicator while that one
//! waits.

use crate::live::{Clock, Screen};
use crate::names;
use crate::names::Namer;
use crate::record::{
    EndReason, Family, HuntId, NameRecord, RoundRecord, RunConfig, RunId, RunRecord, SourceLabel,
    Target, Writer,
};
use crate::run;
use crate::run::{RunError, Turn};
use crate::stats::{HopTable, TtlRow};
use crate::status::{Event, Status};
use crate::trace::Lane;
use crate::ui;
use crate::{REACHED, TARGETS};
use chrono::{DateTime, Utc};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr};
use std::num::NonZeroUsize;
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

/// The lowest host number that a mine draws inside one /24.
///
/// `.0` names the network and `.255` is the broadcast address of it, so neither
/// one names a host. `.1` is the gateway of most /24s, and a gateway stands at
/// the border of the network, which is the shallowest point of it: a probe of
/// the gateway measures the path that the mine already holds.
const FIRST_HOST: u8 = 2;

/// The highest host number that a mine draws inside one /24.
const LAST_HOST: u8 = 254;

/// The numbers that bound one mine of the near space of a long path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MinePlan {
    /// The number of addresses that one mine probes.
    pub(crate) depth: NonZeroUsize,
    /// The length of the block that one mine stays inside.
    pub(crate) prefix: u8,
    /// The number of addresses that one mine probes of any one /24.
    pub(crate) per_prefix: NonZeroUsize,
    /// The wait between two addresses of one mine.
    pub(crate) delay: Duration,
}

/// One address that a hunt traces, and where the draw found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pick {
    /// The address to trace.
    pub(crate) addr: Ipv4Addr,
    /// The address of the first hit whose mine drew this address. An
    /// independent draw of the source holds none.
    pub(crate) mine: Option<Ipv4Addr>,
}

/// The number of bits of an address that name the host inside its /24.
const HOST_BITS: u8 = 8;

/// The length of the block that a mine draws at.
///
/// Two addresses of one /24 take the same path to the border of the network
/// that announced it, so a mine that walked address by address would spend its
/// depth to learn nothing.
const MINE_GRAIN: u8 = ADDRESS_BITS - HOST_BITS;

/// The number of draws that one address of a mine takes before the mine ends.
///
/// The bound is for the mine whose block is nearly full: the last free host of
/// a /24 that the hunt already visited 250 times takes about 253 draws to hit,
/// and a bound that stood near that number would end such a mine early often.
/// A mine that reads this many candidates and passes none has no address left
/// to give, and it ends.
const MINE_ATTEMPTS: usize = 10_000;

/// The number that the seed of a hunt shifts by to seed its mines.
///
/// The mine draws on a sequence of its own, so a hunt of one seed reads the
/// same independent addresses whether it mines or not. Two hunts of one seed
/// still mine alike, because this shift is a constant.
const MINE_SEED_SHIFT: u64 = 0x6b72_745f_6d69_6e65;

/// The mine of the near space of the longest path that a hunt measured.
///
/// The value holds the record of the whole hunt and the one mine that stands.
/// A result that beats the record replaces that mine, because the near space of
/// a destination that no longer holds the record is worth less than the near
/// space of the one that does, and one mine at a time is what keeps the caps of
/// [`MinePlan`] meaningful.
struct Mine {
    /// The numbers that bound each mine.
    plan: MinePlan,
    /// The sequence that each mine draws its addresses from.
    rng: StdRng,
    /// The clock that times the wait between two addresses of one mine.
    clock: Box<dyn Clock>,
    /// The length of the longest path that the hunt measured so far.
    record: Option<u8>,
    /// The mine that stands now. A hunt whose last mine ran out holds none.
    dig: Option<Dig>,
}

/// One mine while it runs: where it digs, how much of its depth is left, and
/// how much of each /24 it already took.
struct Dig {
    /// The address of the first hit that started this mine.
    origin: Ipv4Addr,
    /// The block that this mine stays inside, as the bits of its network.
    block: u32,
    /// The /24 that this mine draws in now, as the bits of its network.
    prefix: u32,
    /// The number of addresses that this mine still gives.
    left: usize,
    /// The number of addresses that this mine gave of each /24.
    probed: BTreeMap<u32, usize>,
    /// The moment that this mine gives its next address at. The first address
    /// of a mine waits for nothing.
    ready: Option<Instant>,
}

/// The bits of the network of one block that holds an address.
fn network_of(addr: Ipv4Addr, prefix: u8) -> u32 {
    // Every caller names a prefix of 8 through 32, so the shift stays inside
    // the width of the value.
    let mask = u32::MAX << (ADDRESS_BITS - prefix);
    addr.to_bits() & mask
}

impl Dig {
    /// The mine of one first hit, which gives this many addresses.
    fn at(origin: Ipv4Addr, plan: MinePlan) -> Self {
        Self {
            origin,
            block: network_of(origin, plan.prefix),
            prefix: network_of(origin, MINE_GRAIN),
            left: plan.depth.get(),
            probed: BTreeMap::new(),
            ready: None,
        }
    }

    /// The number of addresses that this mine already gave of one /24.
    fn taken(&self, prefix: u32) -> usize {
        self.probed.get(&prefix).copied().unwrap_or_default()
    }

    /// The next address of this mine, which no packet of the hunt already went
    /// to.
    ///
    /// The mine fills the /24 of the first hit up to the cap of the plan, and
    /// it then draws a sibling /24 inside its block and fills that one the same
    /// way. A mine whose block holds no free address gives none, and it ends.
    fn draw(
        &mut self,
        rng: &mut StdRng,
        plan: MinePlan,
        visited: &HashSet<Ipv4Addr>,
    ) -> Option<Ipv4Addr> {
        for _ in 0..MINE_ATTEMPTS {
            if self.taken(self.prefix) >= plan.per_prefix.get() {
                self.prefix = self.sibling(rng, plan)?;
                continue;
            }
            let host = rng.random_range(FIRST_HOST..=LAST_HOST);
            let addr = Ipv4Addr::from_bits(self.prefix | u32::from(host));
            if reserved(addr).is_none() && !visited.contains(&addr) {
                return Some(addr);
            }
        }
        None
    }

    /// A /24 of this mine's block that still stands below the cap of the plan.
    ///
    /// The draw is at random and not in order, because a walk of the siblings
    /// in order reads as a horizontal scan of the whole block. A block whose
    /// /24s all hold the cap, and a block whose free /24s no packet routes to,
    /// both give none.
    fn sibling(&self, rng: &mut StdRng, plan: MinePlan) -> Option<u32> {
        // The prefix of a plan is 24 at the most, so the block holds one /24 at
        // the least and the shift stays inside the width of the value.
        let span = 1_u32 << (MINE_GRAIN - plan.prefix);
        for _ in 0..MINE_ATTEMPTS {
            let sibling = self.block | (rng.random_range(0..span) << HOST_BITS);
            if self.taken(sibling) < plan.per_prefix.get()
                && reserved(Ipv4Addr::from_bits(sibling)).is_none()
            {
                return Some(sibling);
            }
        }
        None
    }
}

impl Mine {
    /// Builds the mine of one hunt.
    fn new(plan: MinePlan, seed: u64, clock: Box<dyn Clock>) -> Self {
        Self {
            plan,
            rng: StdRng::seed_from_u64(seed ^ MINE_SEED_SHIFT),
            clock,
            record: None,
            dig: None,
        }
    }

    /// Starts a mine at this destination when its path is the longest one the
    /// hunt measured.
    ///
    /// The first result of a hunt is the longest path it measured, so it starts
    /// a mine. Every result after it must beat the record, and a result that
    /// ties it starts none: the hunt already holds a path of that length, and
    /// the near space of the destination that set it was already mined.
    fn scored(&mut self, addr: Ipv4Addr, length: u8) {
        if self.record.is_some_and(|held| length <= held) {
            return;
        }
        self.record = Some(length);
        self.dig = Some(Dig::at(addr, self.plan));
    }

    /// The next address of the mine that stands, when one stands and it is
    /// due.
    ///
    /// A mine that gave every address of its depth, and a mine whose block
    /// holds no free address, both end here and stand no longer.
    fn address(&mut self, visited: &HashSet<Ipv4Addr>) -> Option<Pick> {
        let now = self.clock.now();
        let plan = self.plan;
        let dig = self.dig.as_mut()?;
        if dig.ready.is_some_and(|ready| now < ready) {
            return None;
        }
        let Some(addr) = dig.draw(&mut self.rng, plan, visited) else {
            self.dig = None;
            return None;
        };
        let origin = dig.origin;
        dig.left -= 1;
        *dig.probed.entry(network_of(addr, MINE_GRAIN)).or_default() += 1;
        dig.ready = Some(now + plan.delay);
        if dig.left == 0 {
            self.dig = None;
        }
        Some(Pick {
            addr,
            mine: Some(origin),
        })
    }
}

/// The draw of one hunt: the source of the candidates, the addresses that the
/// hunt already visited, and the mine of the near space.
pub(crate) struct Draw {
    /// The candidates, in the order the draw reads them.
    candidates: Box<dyn Iterator<Item = Ipv4Addr>>,
    /// Every address that this draw already gave.
    visited: HashSet<Ipv4Addr>,
    /// The address that a peek took out of the source and that no ask has
    /// taken yet.
    peeked: Option<Ipv4Addr>,
    /// The mine of the near space, when the hunt asked for one.
    mine: Option<Mine>,
}

impl Draw {
    /// Builds the draw over a source of candidates.
    pub(crate) fn new(candidates: Box<dyn Iterator<Item = Ipv4Addr>>) -> Self {
        Self {
            candidates,
            visited: HashSet::new(),
            peeked: None,
            mine: None,
        }
    }

    /// Builds the draw of a real hunt, over the seeded sequence of [`random`].
    pub(crate) fn seeded(seed: u64) -> Self {
        Self::new(Box::new(random(seed)))
    }

    /// The same draw, which mines the near space of every record it hears.
    ///
    /// The mine draws on a sequence of its own, so the addresses that a mine
    /// probes shift no address of the independent draw. A hunt of one seed
    /// therefore visits the same independent addresses whether it mines or
    /// not.
    pub(crate) fn mining(mut self, plan: MinePlan, seed: u64, clock: Box<dyn Clock>) -> Self {
        self.mine = Some(Mine::new(plan, seed, clock));
        self
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

    /// The next address to trace, from a mine when one is due and from the
    /// source of the candidates when none is.
    pub(crate) fn address(&mut self) -> Option<Pick> {
        if let Some(pick) = self.mined() {
            return Some(pick);
        }
        let addr = self.peeked.take().or_else(|| self.take())?;
        Some(Pick { addr, mine: None })
    }

    /// The next address of the mine that stands, and no address of the source
    /// of the candidates.
    ///
    /// A hunt that already holds the rounds it wants takes this door. The
    /// addresses of a mine cost no round, so such a hunt still finishes the
    /// mine it started and draws no further independent address.
    pub(crate) fn mined(&mut self) -> Option<Pick> {
        let Self { mine, visited, .. } = self;
        let pick = mine.as_mut()?.address(visited)?;
        visited.insert(pick.addr);
        Some(pick)
    }

    /// Tells the draw the length of the path that one destination gave, so a
    /// mine starts when that path is the longest one the hunt measured.
    ///
    /// A draw that mines nothing keeps nothing.
    pub(crate) fn scored(&mut self, addr: Ipv4Addr, length: u8) {
        if let Some(mine) = self.mine.as_mut() {
            mine.scored(addr, length);
        }
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
/// the numbers that the summary ranks. The run loop hands every round and every
/// name to the screen already, so the fold rides on the door that is there and
/// the hunt reads no file back.
///
/// The screen ticks no indicator. A hunt holds many destinations at once, and
/// the sweep of the whole pool is the heartbeat that the indicator reads, not
/// the turn of any one destination in it.
pub(crate) struct Scorer {
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
}

impl Scorer {
    /// Builds the screen of one destination.
    pub(crate) fn new(addr: Ipv4Addr, run: RunId, first_ttl: u8) -> Self {
        Self {
            addr,
            run,
            first_ttl,
            table: HopTable::new(),
            reached_at: None,
            names: BTreeMap::new(),
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

impl Screen for Scorer {
    /// A hunt takes no key of the terminal, so this screen asks for no stop.
    ///
    /// `Ctrl-C` reaches a hunt through the signal flag, which stops every
    /// destination the hunt holds and the hunt itself.
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
    /// The lane is the lane that this tracer probes in. A hunt holds one lane
    /// for each destination it traces at one moment, and no two destinations of
    /// one moment hold one lane, so no tracer of the hunt reads the answers of
    /// another. The lane comes back to the hunt when the destination stops, and
    /// the tracer of the destination that held it before must stop first.
    ///
    /// # Errors
    ///
    /// Returns the reason as text when the tracer does not start.
    fn start(
        &mut self,
        target: Ipv4Addr,
        run: &RunId,
        lane: Lane,
    ) -> Result<Receiver<RoundRecord>, String>;
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

/// The two numbers that stop one hunt.
///
/// A hunt stops on whichever of them it meets first: the rounds it wants, or
/// the destinations it traced looking for them. One value carries both, so the
/// loop of the hunt and the indicator that shows it read the same pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Bounds {
    /// The number of destinations that must answer before the hunt stops
    /// drawing.
    ///
    /// A destination that answered nothing costs no round. Most of the address
    /// space answers nothing, so a count of every draw would spend the hunt on
    /// addresses that measure no path at all.
    ///
    /// The destinations that stood when the last of these answered finish and
    /// count, so a hunt can hold a few more rounds than this. Each of them is a
    /// measurement the hunt already paid for.
    pub(crate) rounds: u64,
    /// The number of destinations that the hunt traces before it gives up.
    ///
    /// The draw of a real hunt never runs out, so this is what stops a hunt
    /// that finds fewer answers than it wants.
    ///
    /// A destination counts against this the moment its tracer starts, whether
    /// it answers or not and whether it finishes or not. The indicator and the
    /// summary count the same destinations, so both lines read one number
    /// against this bound.
    pub(crate) max_targets: u64,
}

/// The numbers that bound one hunt.
pub(crate) struct Plan {
    /// The two numbers that stop the hunt.
    pub(crate) bounds: Bounds,
    /// The number of destinations that the hunt traces at one moment.
    ///
    /// The hunt starts this many destinations at once and starts another one
    /// each time one of them stops, so the time that a destination which
    /// answers nothing costs is a time that the hunt spends on the other
    /// destinations of the pool.
    pub(crate) concurrency: NonZeroUsize,
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

/// One destination that a hunt holds in flight.
///
/// The hunt sweeps every flight it holds, one turn of each, and takes a flight
/// out when its run closes. The lane then goes back to the pool, and the next
/// destination the hunt draws takes it.
struct Flight {
    /// The destination that this flight traces.
    target: Ipv4Addr,
    /// The lane that its tracer probes in.
    lane: Lane,
    /// The run that records it.
    run: run::Run,
    /// The screen that folds its rounds into a score.
    scorer: Scorer,
}

/// One hunt while it runs: the destinations it holds, the scores it took, and
/// the two counts that stop it.
///
/// The value is the loop of the hunt. [`record`] builds one, drives it, and
/// reads the scores off it, so the three steps of a turn — fill the pool, sweep
/// it, sleep — each stand as a method of their own.
struct Hunt<'a, 's, W: Write> {
    /// What every run of this hunt has in common.
    facts: &'a Facts,
    /// The numbers that bound it.
    plan: &'a Plan,
    /// The addresses, the rounds, and the names that it draws on.
    sources: &'a mut Sources<'s>,
    /// Whether the user asked the hunt to stop.
    stop: &'a dyn Fn() -> bool,
    /// The file that it writes.
    writer: &'a mut Writer<W>,
    /// The indicator that it shows itself on.
    status: &'a mut dyn Status,
    /// The destinations that it holds in flight.
    flights: Vec<Flight>,
    /// The lanes that no destination holds.
    free: Vec<Lane>,
    /// The score of each destination that finished.
    scores: Vec<Score>,
    /// The number of destinations that answered.
    reached: u64,
    /// The number of destinations whose tracer started.
    targets: u64,
    /// The moment of the run of the destination that it started last.
    previous: Option<DateTime<Utc>>,
    /// True when the draw gave no further address.
    drawn_out: bool,
}

impl<'a, 's, W: Write> Hunt<'a, 's, W> {
    /// Builds the hunt of these facts, this plan, and these sources.
    fn new(
        facts: &'a Facts,
        plan: &'a Plan,
        sources: &'a mut Sources<'s>,
        stop: &'a dyn Fn() -> bool,
        writer: &'a mut Writer<W>,
        status: &'a mut dyn Status,
    ) -> Self {
        // The lanes go in backwards, so the pop of the first destination gives
        // the first lane and a hunt of one destination at a time reads the lane
        // that a trace of one destination reads.
        let mut free = Lane::pool(plan.concurrency.get());
        free.reverse();
        Self {
            facts,
            plan,
            sources,
            stop,
            writer,
            status,
            flights: Vec::new(),
            free,
            scores: Vec::new(),
            reached: 0,
            targets: 0,
            previous: None,
            drawn_out: false,
        }
    }

    /// Whether the hunt starts another destination now.
    ///
    /// The pool stays full until the rounds the hunt wants answer. It never
    /// shrinks to the rounds that are left, because the tail of such a hunt
    /// runs one destination at a time and that tail is most of the time the
    /// hunt takes.
    fn room(&self) -> bool {
        !self.drawn_out
            && !(self.stop)()
            && self.reached < self.plan.bounds.rounds
            && self.targets < self.plan.bounds.max_targets
            && self.flights.len() < self.plan.concurrency.get()
    }

    /// Draws destinations and starts them, until the pool is full or a bound
    /// stops the hunt from drawing another.
    ///
    /// A destination counts, and reaches the indicator, when its tracer
    /// starts. A tracer that refuses starts no destination, so the address it
    /// refused counts nowhere: not against the cap, and not on the line.
    ///
    /// # Errors
    ///
    /// Returns [`HuntError::Run`] when a record does not reach the file, and
    /// [`HuntError::Tracer`] when the tracer of a destination does not start.
    fn fill(&mut self) -> Result<(), HuntError> {
        while self.room() {
            let Some(lane) = self.free.pop() else {
                return Ok(());
            };
            let Some(pick) = self.sources.draw.address() else {
                self.free.push(lane);
                self.drawn_out = true;
                return Ok(());
            };
            match self.start(pick.addr, lane) {
                Ok(flight) => {
                    self.targets += 1;
                    self.status.show(Event::Target(pick.addr));
                    self.flights.push(flight);
                }
                Err(fault) => {
                    self.free.push(lane);
                    return Err(fault);
                }
            }
        }
        Ok(())
    }

    /// Starts the trace of one destination in one lane.
    ///
    /// The run takes a round limit of the probe rounds of the plan and a
    /// deadline of the target timeout. The deadline bounds every destination,
    /// and not the quiet ones alone: no destination holds its lane for longer
    /// than that timeout. The round limit is what stops a destination that
    /// answers, because `Cli::resolve` refuses a plan whose timeout holds fewer
    /// than one probe round more than the plan asks for. The last round lands
    /// past the time of the rounds, so a timeout of exactly that time would cut
    /// every destination short.
    ///
    /// # Errors
    ///
    /// Returns [`HuntError::Run`] when a record does not reach the file, and
    /// [`HuntError::Tracer`] when the tracer of this destination does not
    /// start.
    fn start(&mut self, target: Ipv4Addr, lane: Lane) -> Result<Flight, HuntError> {
        let moment = next_moment(self.previous, Utc::now());
        self.previous = Some(moment);
        let id = RunId::at(moment);
        let rounds = self
            .sources
            .probes
            .start(target, &id, lane)
            .map_err(|reason| HuntError::Tracer { target, reason })?;
        let record = RunRecord {
            run: id.clone(),
            krt: self.facts.krt.clone(),
            source: self.facts.source.clone(),
            target: Target {
                // The hunt drew the address, so the address is what the user
                // named. A reader of the file thus finds the same text in the
                // field that a trace of that address by hand would write.
                arg: target.to_string(),
                addr: IpAddr::V4(target),
                family: Family::Ipv4,
            },
            config: self.facts.config,
            host: self.facts.host.clone(),
            hunt: Some(self.facts.id.clone()),
        };
        let limits = run::Limits {
            rounds: Some(self.plan.probes_per_round),
            // A limit too large to add to the clock leaves the destination
            // without a moment, and the round limit then stops it.
            deadline: Instant::now().checked_add(self.plan.target_timeout),
            name_grace: self.plan.name_grace,
        };
        let namer = Namer::new(Box::new(Rc::clone(&self.sources.resolver)), id.clone());
        let scorer = Scorer::new(target, id, self.facts.config.first_ttl);
        let run = run::Run::open(&record, rounds, limits, namer, self.writer)?;
        Ok(Flight {
            target,
            lane,
            run,
            scorer,
        })
    }

    /// Takes one turn of every destination in flight, and closes the ones that
    /// finished.
    ///
    /// Each turn waits for nothing, so one destination that answers slowly
    /// holds up no other. A destination that closed and still waits for the
    /// names of its hops answers [`Turn::Draining`], which takes one step of
    /// that wait and moves nothing. It holds its lane until the wait ends, so
    /// the hunt draws no destination in its place, and it holds up no
    /// destination that already stands.
    ///
    /// The sweep answers whether the hunt moved: a turn that recorded a round
    /// and a destination that closed both move it, and a sweep that moved
    /// nothing is what the hunt sleeps after.
    ///
    /// A fault of one destination stops the hunt, and [`Hunt::abandon`] closes
    /// the destinations that stood beside it before the fault reaches the
    /// caller.
    ///
    /// # Errors
    ///
    /// Returns [`RunError::Write`] when a record does not reach the file, and
    /// [`RunError::Tracer`] when the tracer thread of a destination stops
    /// before a limit does.
    fn sweep(&mut self) -> Result<bool, RunError> {
        self.status.show(Event::Tick);
        let mut moved = false;
        let mut place = 0;
        while place < self.flights.len() {
            let taken = {
                let flight = &mut self.flights[place];
                flight
                    .run
                    .turn(Duration::ZERO, self.stop, self.writer, &mut flight.scorer)
            };
            let turn = match taken {
                Ok(turn) => turn,
                Err(fault) => {
                    self.abandon(place);
                    return Err(fault);
                }
            };
            match turn {
                Turn::Round => {
                    moved = true;
                    place += 1;
                }
                Turn::Quiet | Turn::Draining => place += 1,
                Turn::Closed(outcome) => {
                    self.close(place, outcome.reason);
                    moved = true;
                }
            }
        }
        Ok(moved)
    }

    /// Closes every destination that stood when a fault stopped the hunt.
    ///
    /// The destination at `place` is the one that met the fault, and
    /// [`run::Run::turn`] already wrote the `end` record of that run. So this
    /// takes that destination out and writes nothing for it. Every other
    /// destination of the pool holds a `run` record, the rounds it recorded,
    /// and nothing that closes it, and [`run::Run::abandon`] is the record that
    /// closes it.
    ///
    /// The hunt drops the answer of each of those writes. A write that fails is
    /// what stops most hunts here, and it fails these writes too. The fault
    /// that stopped the hunt is the first one, and the caller reads that one.
    ///
    /// An abandoned destination takes no score, no row of the table, and no
    /// answer on the indicator. It stood in the air and it did not finish, and
    /// the table holds the rounds that finished. It still counts among the
    /// destinations the hunt started, because its tracer started.
    fn abandon(&mut self, place: usize) {
        drop(self.flights.remove(place));
        for flight in self.flights.drain(..) {
            drop(flight.run.abandon(self.writer));
        }
    }

    /// Takes the destination at this place out of the pool and scores it.
    ///
    /// A destination that the user cut short takes no row of the table, no
    /// round, and no answer on the indicator: a round that stopped in the
    /// middle measured a path that the tool never finished measuring. It still
    /// counts among the destinations the hunt started. The lane goes back to
    /// the pool either way.
    ///
    /// This is the path of a destination whose run closed. A destination that
    /// a fault of another destination cut short takes [`Hunt::abandon`], and it
    /// takes no score either.
    fn close(&mut self, place: usize, reason: EndReason) {
        let flight = self.flights.remove(place);
        self.free.push(flight.lane);
        if reason == EndReason::Quit {
            return;
        }
        let score = flight.scorer.score();
        let answered = score.kind == PathKind::Reached;
        if answered {
            self.reached += 1;
        }
        self.status.show(Event::Scored {
            target: flight.target,
            reached: answered,
        });
        self.scores.push(score);
    }

    /// The longest that the hunt sleeps after a sweep that moved nothing.
    ///
    /// The sleep ends at the nearest deadline of the destinations in flight, so
    /// a destination stops at the moment of its target timeout and not one
    /// sleep after it. A destination that waits for the names of its hops names
    /// the end of that wait in the place of its deadline, so the next step of
    /// the wait comes at the moment it is due.
    fn nap(&self) -> Duration {
        self.flights
            .iter()
            .map(|flight| flight.run.wait())
            .min()
            .unwrap_or_default()
    }
}

/// Records one hunt: one run for each destination, and the summary of them all.
///
/// The hunt holds `Plan::concurrency` destinations in flight. It starts that
/// many at once, and it starts another one each time one of them stops, so the
/// time that a destination which answers nothing costs is time the hunt spends
/// on the other destinations of the pool. Most of the address space answers
/// nothing, so that time is most of the time a hunt takes.
///
/// The hunt stops drawing when `Bounds::rounds` destinations answered. A
/// destination that answered nothing costs no round, so the hunt keeps drawing
/// until it holds the paths that the user asked for. It gives up at
/// `Bounds::max_targets` destinations, which is what stops a hunt that finds
/// fewer answers than it wants: the draw never runs out on its own.
///
/// The pool stays full until that moment and never shrinks to the rounds that
/// are left. The destinations that stood when the last round answered finish
/// and count, so a hunt can hold a few more rounds than it asked for, and each
/// of them is a measurement the hunt already paid for.
///
/// Each destination takes one run of `run.rs`, and every one of them writes
/// into the one file. The records of two destinations therefore stand between
/// each other, and the records of one destination stay in order, which is what
/// `krt replay <file> --run <id>` folds.
///
/// `stop` answers whether the user asked the hunt to stop, and it reaches both
/// this loop and every run in flight. A destination that the user cut short
/// takes no row of the table and no round: the table holds the rounds that
/// finished. It counts among the destinations the hunt started, because the
/// hunt started it and the indicator named it.
///
/// A tracer that does not start stops the hunt, and the destinations that stood
/// at that moment finish first. A tracer that dies stops the hunt where it
/// stands, and so does a fault of the file. The destinations that stood at that
/// moment take the `end` record of a fault, so every run of the file holds the
/// record that closes it. A fault of the file fails those records too, because
/// a file that takes no record takes none of theirs either.
///
/// All three keep the summary of the rounds that finished: the rounds in front
/// of the fault measured what they measured, and the caller prints their table
/// before it prints the reason. A destination that a fault cut short takes no
/// row of that table and no round, as a destination that the user cut short
/// takes none, and it counts among the destinations the hunt started as that
/// one does.
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
    let mut hunt = Hunt::new(facts, plan, sources, stop, writer, status);
    let mut fault = None;
    loop {
        if fault.is_none() {
            if let Err(refused) = hunt.fill() {
                fault = Some(refused);
            }
        }
        if hunt.flights.is_empty() {
            break;
        }
        match hunt.sweep() {
            Ok(true) => {}
            Ok(false) => std::thread::sleep(hunt.nap()),
            Err(broke) => {
                fault = Some(HuntError::Run(broke));
                break;
            }
        }
    }
    // The line goes back before the caller prints the table of the rounds that
    // finished and, when a fault stopped the hunt, the reason.
    hunt.status.show(Event::Stop);
    let summary = Summary::new(
        hunt.scores,
        started.elapsed(),
        hunt.targets,
        plan.bounds,
        plan.include_partial,
    );
    match fault {
        Some(fault) => Err(HuntStopped { summary, fault }),
        None => Ok(summary),
    }
}

/// The moment that names the run of the next destination.
///
/// A run identifier holds the moment of the start to the millisecond, and a
/// hunt starts two destinations inside one millisecond whenever its pool fills.
/// Two runs of one identifier would leave a reader unable to tell them apart,
/// and `krt replay <file> --run <id>` unable to fold either one. So the moment
/// of a destination stands at least one millisecond after the moment of the
/// destination in front of it.
///
/// The shift is below the resolution that the identifier states, and the
/// identifier stays a moment that sorts, so the runs of one hunt still read in
/// the order the hunt started them.
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
///
/// The count of the destinations comes off the hunt and not off those scores. A
/// destination that started and took no score still cost the hunt one of the
/// destinations its bounds let it trace, and the indicator already named it.
#[derive(Debug)]
pub(crate) struct Summary {
    /// The score of each destination, in the order the hunt traced them.
    scores: Vec<Score>,
    /// The time that the whole hunt took.
    elapsed: Duration,
    /// The number of destinations whose tracer started.
    ///
    /// The indicator counts the same destinations, so the last line of a hunt
    /// and the summary under it hold one number.
    targets: u64,
    /// The two numbers that stopped the hunt.
    ///
    /// The counts stand against them, so a reader tells a hunt that held every
    /// round it wanted from one that gave up on its targets.
    bounds: Bounds,
    /// True when a partial path competes for a row of the table.
    include_partial: bool,
}

impl Summary {
    /// Builds the summary of one hunt.
    ///
    /// `targets` is the number of destinations whose tracer started, and
    /// `bounds` holds the number it could start. The two stand against each
    /// other in the counts, as `scores` stands against `Bounds::rounds`.
    pub(crate) fn new(
        scores: Vec<Score>,
        elapsed: Duration,
        targets: u64,
        bounds: Bounds,
        include_partial: bool,
    ) -> Self {
        Self {
            scores,
            elapsed,
            targets,
            bounds,
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
    /// The two counts stand against the two bounds that stopped the hunt, as
    /// the line of the indicator holds them. A reader thus tells a hunt that
    /// held every round it wanted from one that gave up on its targets, which
    /// the bare counts never said.
    ///
    /// The three counts of a hunt that stopped early do not add up. The reached
    /// count and the partial count read the scores, and the targets count reads
    /// the destinations that started, so a destination that started and took no
    /// score stands in the third number alone. `Ctrl-C` leaves such
    /// destinations, and so does a fault, because neither one lets the
    /// destinations in flight finish. A hunt that ran to a bound of its own
    /// leaves none, and its three counts do add up.
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
            format!("{reached}/{} {REACHED}", self.bounds.rounds),
            format!("{}/{} {TARGETS}", self.targets, self.bounds.max_targets),
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
        random, record, reserved, Block, Bounds, Draw, Facts, HuntError, HuntStopped, MinePlan,
        PathKind, Pick, Plan, Probes, RunError, Score, Scorer, Sources, Summary, ATTEMPTS, FASTEST,
        FIRST_HOST, LAST_HOST, LONGEST, NOTHING_TO_RANK, PARTIAL, SHORTEST, SLOWEST,
    };
    use crate::live::Screen;
    use crate::names::Lookup;
    use crate::record::{
        EndReason, Family, HuntId, NameRecord, Privilege, Record, Recording, RoundRecord,
        RunConfig, RunId, SourceKind, SourceLabel, Writer,
    };
    use crate::status::{Event, Status};
    use crate::testing::{named, round, FakeClock, FakeResolver};
    use crate::trace::Lane;
    use crate::{Multipath, Protocol};
    use chrono::Utc;
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::collections::HashSet;
    use std::collections::VecDeque;
    use std::net::{IpAddr, Ipv4Addr};
    use std::num::NonZeroUsize;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};
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

    /// The draw over a scripted list that turns a flag over when it runs out.
    ///
    /// A hunt whose pool holds more destinations than the list asks the draw
    /// one time more than the list answers. Every address of the list stands in
    /// flight at that moment, so a stop closure that reads this flag stops a
    /// hunt whose pool is full.
    ///
    /// The trigger is what the hunt does to the draw, and not a count of the
    /// calls to the closure. A change to the order of the tests inside
    /// [`Hunt::room`] therefore leaves it alone.
    fn draw_that_signals(candidates: &[&str], ran_out: &Rc<Cell<bool>>) -> Draw {
        let list: Vec<Ipv4Addr> = candidates.iter().copied().map(address).collect();
        let flag = Rc::clone(ran_out);
        Draw::new(Box::new(list.into_iter().chain(std::iter::from_fn(
            move || {
                flag.set(true);
                None
            },
        ))))
    }

    /// The address that a draw gives, after it reads the rejected candidates
    /// that stand in front of one routable address.
    ///
    /// Every test of a block reads this, so each of them proves the rejection
    /// through the door that the hunt uses and not through the guard alone.
    fn drawn_past(rejected: &[&str]) -> Option<Ipv4Addr> {
        let mut candidates = rejected.to_vec();
        candidates.push(ROUTABLE);
        drawn(&mut draw_of(&candidates))
    }

    /// The address of the next pick of a draw, without the mine beside it.
    ///
    /// Every test of the guard and of the visited set reads the address alone,
    /// and the mine of each of those draws is absent.
    fn drawn(draw: &mut Draw) -> Option<Ipv4Addr> {
        draw.address().map(|pick| pick.addr)
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
        assert_eq!(drawn(&mut draw_of(&[ROUTABLE])), Some(address(ROUTABLE)));
    }

    #[test]
    fn the_draw_of_a_source_that_ran_out_gives_no_address() {
        assert_eq!(drawn(&mut draw_of(&[])), None);
    }

    #[test]
    fn the_draw_rejects_an_address_that_this_hunt_already_visited() {
        let mut draw = draw_of(&[ROUTABLE, ROUTABLE, OTHER_ROUTABLE]);
        assert_eq!(drawn(&mut draw), Some(address(ROUTABLE)));
        assert_eq!(
            drawn(&mut draw),
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
        assert_eq!(drawn(&mut draw), None);
    }

    #[test]
    fn the_draw_reads_no_more_candidates_than_the_bound_of_the_attempts() {
        let mut read = 0;
        let counted = std::iter::repeat_with(move || {
            read += 1;
            assert!(read <= ATTEMPTS, "the draw read {read} candidates");
            address("10.0.0.1")
        });
        assert_eq!(drawn(&mut Draw::new(Box::new(counted))), None);
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
            .map(|_| drawn(&mut draw).expect("the seeded draw never runs out"))
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
        let mut scorer = Scorer::new(address(destination), RunId::from(run), FIRST_TTL);
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
        let mut scorer = Scorer::new(address(DESTINATION), RunId::from(RUN), FIRST_TTL);
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

    /// The address of a second destination that answered nothing.
    const ANOTHER_QUIET: &str = "8.8.8.8";

    /// The run that recorded the trace of the quiet destination.
    const QUIET_RUN: &str = "2026-08-18T12:02:00.000Z";

    /// The time that the hunt of every summary test took.
    const ELAPSED: Duration = Duration::from_secs(192);

    /// The bounds of the hunt of every summary test.
    const SUMMARY_BOUNDS: Bounds = Bounds {
        rounds: 8,
        max_targets: 128,
    };

    /// The number of destinations that a summary test of one score names.
    const ONE_TARGET: u64 = 1;

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
        Summary::new(
            scores,
            ELAPSED,
            THREE_TARGETS,
            SUMMARY_BOUNDS,
            include_partial,
        )
    }

    /// The number of destinations that the hunt of [`a_hunt`] started.
    ///
    /// Every one of the three finished and took a score, so this hunt is one
    /// whose three counts add up.
    const THREE_TARGETS: u64 = 3;

    /// The row of the summary that carries one label.
    fn row(summary: &Summary, label: &str) -> String {
        summary
            .lines()
            .into_iter()
            .find(|line| line.trim_start().starts_with(label))
            .unwrap_or_else(|| panic!("the summary must hold the `{label}` row"))
    }

    /// The three counts of the summary, without the wall time beside them.
    ///
    /// A hunt that runs takes whatever time the machine gives it, so a test of
    /// a hunt that runs pins the three counts and leaves the fourth field out.
    /// A test of a summary that a test built reads [`counts`] instead, because
    /// such a summary holds a wall time that the test named.
    fn measured(summary: &Summary) -> String {
        let line = counts(summary);
        let (measured, _) = line
            .rsplit_once(crate::ui::FIELD_SEPARATOR)
            .expect("the counts hold four fields");
        measured.to_owned()
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

    /// The counts stand against the bounds that stopped the hunt.
    ///
    /// A reader who sees `2/8 reached` knows that the hunt stopped short of
    /// what it wanted, and `3/128 targets` says what it spent looking.
    #[test]
    fn the_counts_hold_the_rounds_the_targets_the_partial_and_the_wall_time() {
        assert_eq!(
            counts(&a_hunt(false)),
            "2/8 reached   3/128 targets   1 partial   192s"
        );
    }

    /// A hunt whose destinations all answered nothing ranks no path.
    ///
    /// The table then says so, and the counts still print: a hunt that reached
    /// nothing still tells the reader how many rounds it spent.
    #[test]
    fn a_summary_of_no_ranked_path_says_so_and_still_counts_the_hunt() {
        let scores = vec![traced(QUIET, QUIET_RUN, &[&[(1, FIRST_HOP, 1.0)]], &[])];
        let summary = Summary::new(scores, ELAPSED, ONE_TARGET, SUMMARY_BOUNDS, false);
        let lines = summary.lines();
        assert!(
            lines.iter().any(|line| line.contains(NOTHING_TO_RANK)),
            "the summary must say that it ranked nothing: {lines:?}"
        );
        assert_eq!(
            counts(&summary),
            "0/8 reached   1/128 targets   1 partial   192s"
        );
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
        "2/8 reached   3/128 targets   1 partial   192s",
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
    /// run reads a closed channel as a tracer that died. The source that
    /// [`FakeProbes::that_drops_the_sender_of`] builds keeps every sender but
    /// one.
    struct FakeProbes {
        /// The rounds of each destination, the next destination first.
        scripts: VecDeque<Vec<RoundRecord>>,
        /// The destinations that the hunt asked for, in order.
        asked: Vec<Ipv4Addr>,
        /// The lane of each of those destinations, in the same order.
        lanes: Vec<Lane>,
        /// The senders of the channels, held so that none of them closes.
        senders: Vec<std::sync::mpsc::Sender<RoundRecord>>,
        /// The reason that a start gives, when the tracer must fail.
        refuses: Option<String>,
        /// The number of destinations that the source serves before it
        /// refuses. A source that refuses nothing never reads it.
        serves: usize,
        /// The place of the destination whose sender the source drops. A
        /// source that drops no sender holds no place.
        drops: Option<usize>,
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
                lanes: Vec::new(),
                senders: Vec::new(),
                refuses: None,
                serves: 0,
                drops: None,
            }
        }

        /// A source that drops the sender of one destination.
        ///
        /// The channel of the destination at `place` closes as soon as the run
        /// reads the rounds of its script, and that run then reads a tracer
        /// that died. Every other channel stays open, so the destinations
        /// beside it stand in flight at that moment.
        fn that_drops_the_sender_of(scripts: &[Rounds], place: usize) -> Self {
            let mut probes = Self::of(scripts);
            probes.drops = Some(place);
            probes
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
            lane: Lane,
        ) -> Result<std::sync::mpsc::Receiver<RoundRecord>, String> {
            self.asked.push(target);
            self.lanes.push(lane);
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
            // The push above counts this destination, so the length names the
            // place of it.
            if self.drops == Some(self.asked.len() - 1) {
                // The sender goes out of scope here, and the channel of this
                // destination closes as soon as the run reads the rounds above.
                // A tracer thread that dies closes its channel the same way.
                drop(sender);
            } else {
                self.senders.push(sender);
            }
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
        /// The lane of each of those destinations, in the same order.
        lanes: Vec<Lane>,
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

        /// The greatest number of destinations that the hunt held at one
        /// moment.
        ///
        /// A `Target` event names a destination that the hunt started, and a
        /// `Scored` event names one that it finished. The difference between
        /// the two counts is the number that stood at that point of the hunt.
        fn most_at_once(&self) -> usize {
            let mut standing: usize = 0;
            let mut most: usize = 0;
            for event in &self.events {
                match event {
                    Event::Target(_) => {
                        standing += 1;
                        most = most.max(standing);
                    }
                    Event::Scored { .. } => standing = standing.saturating_sub(1),
                    Event::Tick | Event::Stop => {}
                }
            }
            most
        }

        /// Whether each destination that the hunt scored answered, in order.
        fn answers(&self) -> Vec<bool> {
            self.events
                .iter()
                .filter_map(|event| match event {
                    Event::Scored { reached, .. } => Some(*reached),
                    _ => None,
                })
                .collect()
        }
    }

    impl Status for Recorder {
        fn show(&mut self, event: Event) {
            self.events.push(event);
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
        hunted_bounded(addresses, scripts, wanting(rounds), stop)
    }

    /// The number of destinations that a test hunt traces before it gives up.
    ///
    /// Every test above hands the draw fewer addresses than this, so the cap
    /// stops no hunt but the one that names its own.
    const A_GENEROUS_CAP: u64 = 64;

    /// The number of destinations that a test hunt traces at one moment.
    ///
    /// The number stands above one, so every test that names no other number
    /// reads the loop of a hunt that holds a pool. A test of the pool itself
    /// names the number it needs.
    const TEST_CONCURRENCY: usize = 2;

    /// The number of probe rounds that each destination of a test hunt takes.
    ///
    /// One round is enough for every test that reads a score, because a score
    /// of one round holds every column of the table. A test that needs one
    /// destination to close while another stands names a greater number.
    const TEST_PROBES_PER_ROUND: u64 = 1;

    /// The shape of one test hunt: the two bounds that stop it, the number of
    /// destinations it traces at one moment, and the three numbers of its plan
    /// that a test of the pool names.
    #[derive(Debug, Clone, Copy)]
    struct Shape {
        /// The two numbers that stop the hunt.
        bounds: Bounds,
        /// The number of destinations that the hunt traces at one moment.
        concurrency: usize,
        /// The number of probe rounds that each destination takes.
        probes_per_round: u64,
        /// The longest that one destination takes, whether it answers or not.
        target_timeout: Duration,
        /// The longest that a destination waits, after its last round, for the
        /// names that its lookups have not given yet.
        name_grace: Duration,
    }

    impl Shape {
        /// The same hunt, tracing this many destinations at one moment.
        const fn at_once(self, concurrency: usize) -> Self {
            Self {
                concurrency,
                ..self
            }
        }

        /// The same hunt, tracing one destination at a time.
        const fn serial(self) -> Self {
            self.at_once(1)
        }

        /// The same hunt, where each destination takes this many probe rounds.
        const fn probing(self, probes_per_round: u64) -> Self {
            Self {
                probes_per_round,
                ..self
            }
        }

        /// The same hunt, where a destination that closes waits this long for
        /// the names of its hops, and where no destination takes longer than
        /// this whatever it answers.
        const fn waiting_for_names(self, name_grace: Duration, target_timeout: Duration) -> Self {
            Self {
                target_timeout,
                name_grace,
                ..self
            }
        }
    }

    /// The shape of a test hunt that wants this many rounds, and that traces as
    /// many destinations as it must to find them.
    const fn wanting(rounds: u64) -> Shape {
        Shape {
            bounds: Bounds {
                rounds,
                max_targets: A_GENEROUS_CAP,
            },
            concurrency: TEST_CONCURRENCY,
            probes_per_round: TEST_PROBES_PER_ROUND,
            target_timeout: TARGET_TIMEOUT,
            name_grace: Duration::ZERO,
        }
    }

    /// The shape of a test hunt that gives up after this many destinations.
    const fn giving_up_after(rounds: u64, max_targets: u64) -> Shape {
        Shape {
            bounds: Bounds {
                rounds,
                max_targets,
            },
            ..wanting(rounds)
        }
    }

    /// Runs one hunt over a scripted draw, a scripted source of rounds, and the
    /// shape that the test names.
    fn hunted_bounded(
        addresses: &[&str],
        scripts: &[Rounds],
        shape: Shape,
        stop: &dyn Fn() -> bool,
    ) -> Result<Hunted, HuntStopped> {
        let mut probes = FakeProbes::of(scripts);
        let mut recorder = Recorder::default();
        let outcome = run_hunt(
            addresses,
            &mut probes,
            shape,
            stop,
            &Names::None,
            &mut recorder,
        );
        outcome.map(|(summary, recording)| Hunted {
            summary,
            recording,
            asked: probes.asked.clone(),
            lanes: probes.lanes.clone(),
            shown: recorder.events.clone(),
        })
    }

    /// What a test hunt looks the addresses of its hops up with.
    ///
    /// A test that reads the asks of the resolver builds the fake itself and
    /// keeps a share of it, so it reads the log after the hunt ends.
    enum Names {
        /// The hunt looks nothing up, as `--no-dns` gives.
        None,
        /// The hunt asks this resolver.
        Of(Rc<FakeResolver>),
    }

    impl Names {
        /// A resolver of these answers, and the hunt that asks it.
        fn of(answers: &[(&str, &[crate::names::Lookup])]) -> Self {
            Self::Of(FakeResolver::new(answers))
        }

        /// The resolver that this hunt asks.
        ///
        /// # Panics
        ///
        /// Panics on a hunt that looks nothing up. Such a call is a mistake in
        /// the test, not an answer the code under test can give.
        fn resolver(&self) -> Rc<FakeResolver> {
            match self {
                Self::None => panic!("the hunt of this test must look its addresses up"),
                Self::Of(fake) => Rc::clone(fake),
            }
        }
    }

    /// Runs one hunt into a sink of bytes, and reads back the file it wrote.
    fn run_hunt(
        addresses: &[&str],
        probes: &mut FakeProbes,
        shape: Shape,
        stop: &dyn Fn() -> bool,
        names: &Names,
        status: &mut dyn Status,
    ) -> Result<(Summary, Recording), HuntStopped> {
        let mut sink = Vec::new();
        let summary = {
            let mut writer = Writer::to_sink(&mut sink);
            hunt_into(
                draw_of(addresses),
                probes,
                shape,
                stop,
                names,
                &mut writer,
                status,
            )
        }?;
        Ok((summary, read_back(&sink)))
    }

    /// Runs one hunt over the draw that a test names, into the writer that it
    /// names.
    ///
    /// The sink of that writer is what a test of a write that fails hands in,
    /// and the draw is what a test of a hunt that runs out of addresses hands
    /// in. Every other test takes [`run_hunt`], which builds a draw of a list
    /// and reads its bytes back.
    fn hunt_into<W: std::io::Write>(
        draw: Draw,
        probes: &mut FakeProbes,
        shape: Shape,
        stop: &dyn Fn() -> bool,
        names: &Names,
        writer: &mut Writer<W>,
        status: &mut dyn Status,
    ) -> Result<Summary, HuntStopped> {
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
                dns: matches!(names, Names::Of(_)),
            },
            host: HOST.to_owned(),
        };
        let plan = Plan {
            bounds: shape.bounds,
            concurrency: NonZeroUsize::new(shape.concurrency)
                .expect("a test hunt traces at least one destination at a time"),
            probes_per_round: shape.probes_per_round,
            target_timeout: shape.target_timeout,
            name_grace: shape.name_grace,
            include_partial: false,
        };
        let resolver: Rc<dyn crate::names::Resolver> = match names {
            Names::None => Rc::new(crate::names::NoLookups),
            Names::Of(fake) => Rc::clone(fake) as Rc<dyn crate::names::Resolver>,
        };
        let mut sources = Sources {
            draw,
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

    /// The number of scratch files that this process already named.
    ///
    /// The process and the clock are not enough on their own. `cargo test` runs
    /// the tests of one binary on many threads, and two of those threads read
    /// the clock inside one nanosecond. The two then write one path, and each
    /// of them reads the records of the other or reads a file that the other
    /// already removed.
    static SCRATCH_FILES: AtomicU64 = AtomicU64::new(0);

    /// The path of a scratch file that no other run of this test touches.
    ///
    /// Two copies of one test run at the same time under `cargo test`, so the
    /// name carries the process, the moment, and the count of the files that
    /// the process named.
    fn scratch_file() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let count = SCRATCH_FILES.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "krt-hunt-{}-{nanos}-{count}.jsonl",
            std::process::id()
        ))
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

    /// A third destination that answers, for a test that fills a pool of three.
    const ANOTHER_NEAR: &str = "9.9.9.9";

    /// One round that reached that third destination at TTL 7.
    const ANOTHER_NEAR_REACHED_AT_SEVEN: &[&[(u8, &str, f64)]] =
        &[&[(1, FIRST_HOP, 1.0), (7, ANOTHER_NEAR, 30.0)]];

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

    /// The sweep of the pool is the heartbeat of the hunt that holds it.
    ///
    /// A hunt holds many destinations at once, so no one of them can be the
    /// heartbeat. The sweep of the whole pool is what turns the spinner, and it
    /// turns at the pace of the hunt whatever the destinations of the pool do.
    #[test]
    fn the_sweep_of_the_pool_ticks_the_indicator() {
        let hunted =
            hunted(&[NEAR], &[REACHED_AT_FIVE], 1, &never_stops()).expect("the hunt must finish");
        assert!(
            hunted.shown.contains(&Event::Tick),
            "a sweep must tick the indicator: {:?}",
            hunted.shown
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
                draw_of(&[NEAR, FAR]),
                &mut probes,
                wanting(2),
                &never_stops(),
                &Names::None,
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

    /// A hunt starts the destinations of its pool at once.
    ///
    /// This is what makes a hunt fast. Most of the address space answers
    /// nothing, and a destination that answers nothing costs the whole target
    /// timeout, so a hunt that waited out one such destination before it drew
    /// the next spent that time on nothing at all.
    #[test]
    fn a_hunt_starts_the_destinations_of_its_pool_at_once() {
        let hunted = hunted_bounded(
            &[QUIET, ANOTHER_QUIET, NEAR],
            &[&[&[]], &[&[]], &[&[]]],
            wanting(3).at_once(3),
            &never_stops(),
        )
        .expect("the hunt must finish");
        assert_eq!(
            shown(&hunted).most_at_once(),
            3,
            "the hunt held one destination at a time: {:?}",
            hunted.shown
        );
    }

    /// A hunt holds no more destinations at once than its pool.
    #[test]
    fn a_hunt_holds_no_more_destinations_at_once_than_its_pool() {
        let hunted = hunted_bounded(
            &[QUIET, ANOTHER_QUIET, NEAR, FAR],
            &[&[&[]], &[&[]], &[&[]], &[&[]]],
            wanting(4).at_once(2),
            &never_stops(),
        )
        .expect("the hunt must finish");
        assert_eq!(
            shown(&hunted).most_at_once(),
            2,
            "the hunt held more destinations than its pool: {:?}",
            hunted.shown
        );
    }

    /// A hunt holds its pool full until the rounds it wants answer.
    ///
    /// The pool never shrinks to the rounds that are left, because the tail of
    /// such a hunt runs one destination at a time and that tail is most of the
    /// time the hunt takes. The destinations that stood when the last round
    /// answered finish and count, so a hunt can hold a few more rounds than it
    /// asked for, and each of them is a measurement the hunt already paid for.
    #[test]
    fn a_hunt_holds_its_pool_full_until_the_rounds_it_wants_answer() {
        let hunted = hunted_bounded(
            &[NEAR, FAR, ANOTHER_NEAR],
            &[
                REACHED_AT_FIVE,
                FAR_REACHED_AT_EIGHTEEN,
                ANOTHER_NEAR_REACHED_AT_SEVEN,
            ],
            wanting(1).at_once(3),
            &never_stops(),
        )
        .expect("the hunt must finish");
        assert_eq!(
            shown(&hunted).most_at_once(),
            3,
            "the hunt shrank its pool to the one round it wanted: {:?}",
            hunted.shown
        );
        assert!(
            counts(&hunted.summary).starts_with("3/1 reached"),
            "every destination that stood counts: {}",
            counts(&hunted.summary)
        );
    }

    /// No two destinations that a hunt holds at one moment hold one lane.
    ///
    /// Two tracers of one lane read each other's answers, so a hop of one
    /// destination would land in the path of another.
    #[test]
    fn no_two_destinations_of_one_moment_hold_one_lane() {
        let hunted = hunted_bounded(
            &[QUIET, ANOTHER_QUIET, NEAR],
            &[&[&[]], &[&[]], &[&[]]],
            wanting(3).at_once(3),
            &never_stops(),
        )
        .expect("the hunt must finish");
        let held: HashSet<Lane> = hunted.lanes.iter().copied().collect();
        assert_eq!(
            held.len(),
            3,
            "two destinations of one moment held one lane: {:?}",
            hunted.lanes
        );
    }

    /// A hunt takes the lane of a destination back when that destination stops.
    ///
    /// A hunt of a hundred destinations holds no hundred lanes. It holds the
    /// lanes of its pool, and each of them serves one destination after
    /// another.
    #[test]
    fn a_hunt_takes_the_lane_of_a_destination_back_when_it_stops() {
        let hunted = hunted_bounded(
            &[NEAR, FAR],
            &[REACHED_AT_FIVE, FAR_REACHED_AT_EIGHTEEN],
            wanting(2).serial(),
            &never_stops(),
        )
        .expect("the hunt must finish");
        assert_eq!(
            hunted.lanes.len(),
            2,
            "the hunt traced two destinations: {:?}",
            hunted.lanes
        );
        assert_eq!(
            hunted.lanes[0], hunted.lanes[1],
            "a pool of one lane gives that lane to every destination in turn"
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

    /// A destination that answered nothing costs the hunt no round.
    ///
    /// `Plan::rounds` counts the destinations that answered. Most of the
    /// address space answers nothing, so a hunt that counted every draw spent
    /// its rounds on addresses that measured no path at all.
    #[test]
    fn a_destination_that_answered_nothing_costs_no_round() {
        let hunted = hunted(
            &[QUIET, NEAR, ANOTHER_QUIET, FAR],
            &[&[&[]], REACHED_AT_FIVE, &[&[]], FAR_REACHED_AT_EIGHTEEN],
            2,
            &never_stops(),
        )
        .expect("the hunt must finish");
        assert_eq!(
            hunted.asked,
            vec![
                address(QUIET),
                address(NEAR),
                address(ANOTHER_QUIET),
                address(FAR),
            ],
            "the hunt traces destinations until two of them answer"
        );
    }

    /// A hunt gives up after the destinations that its bounds let it trace.
    ///
    /// The draw of a real hunt never runs out, so the cap is the bound that
    /// stops a hunt whose destinations answer nothing. Without it, such a hunt
    /// draws forever.
    #[test]
    fn a_hunt_gives_up_after_the_targets_that_its_bounds_let_it_trace() {
        let hunted = hunted_bounded(
            &[QUIET, ANOTHER_QUIET, NEAR, FAR],
            &[&[&[]], &[&[]], REACHED_AT_FIVE, FAR_REACHED_AT_EIGHTEEN],
            giving_up_after(2, 2),
            &never_stops(),
        )
        .expect("the hunt must finish");
        assert_eq!(
            hunted.asked,
            vec![address(QUIET), address(ANOTHER_QUIET)],
            "the hunt gives up after two destinations, answered or not"
        );
    }

    #[test]
    fn a_hunt_stops_after_the_number_of_destinations_that_answered() {
        let hunted = hunted(
            &[NEAR, FAR, QUIET],
            &[REACHED_AT_FIVE, FAR_REACHED_AT_EIGHTEEN, REACHED_AT_FIVE],
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
            wanting(1),
            &never_stops(),
            &Names::None,
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
            "0/2 reached   0/64 targets   0 partial   0ms"
        );
    }

    /// The targets count of the summary reads the destinations that the hunt
    /// started.
    ///
    /// A destination that the user stopped takes no score, and the indicator
    /// already named it. So a count of the scores stands below the count that
    /// the last line of the indicator holds, and below the number of runs in
    /// the file.
    ///
    /// The hunt of this test holds three destinations in a pool of four. The
    /// draw runs out at that moment, and the stop closure answers true from
    /// then on, so every one of the three closes on the stop and none of them
    /// takes a score.
    #[test]
    fn the_summary_counts_the_destinations_that_the_hunt_started() {
        let ran_out = Rc::new(Cell::new(false));
        let flag = Rc::clone(&ran_out);
        let stop = move || flag.get();
        let mut probes = FakeProbes::of(&[&[], &[], &[]]);
        let mut sink = Vec::new();
        let summary = {
            let mut writer = Writer::to_sink(&mut sink);
            hunt_into(
                draw_that_signals(&[NEAR, FAR, ANOTHER_NEAR], &ran_out),
                &mut probes,
                wanting(8).at_once(4),
                &stop,
                &Names::None,
                &mut writer,
                &mut Recorder::default(),
            )
        }
        .expect("the hunt must finish");
        assert_eq!(measured(&summary), "0/8 reached   3/64 targets   0 partial");
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
        assert!(counts(&hunted.summary).contains("1/2 reached"));
        assert!(counts(&hunted.summary).contains("1 partial"));
    }

    /// A tracer that will not start stops the hunt and names the destination.
    #[test]
    fn a_tracer_that_does_not_start_stops_the_hunt_and_names_the_destination() {
        let mut probes = FakeProbes::that_refuses(NO_RAW_SOCKET);
        let stopped = run_hunt(
            &[NEAR],
            &mut probes,
            wanting(1),
            &never_stops(),
            &Names::None,
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
            wanting(3),
            &never_stops(),
            &Names::None,
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
            counts.starts_with("2/3 reached   2/64 targets   0 partial"),
            "the summary counts the two rounds that finished: {counts}"
        );
    }

    /// A tracer that refuses lets the destinations in flight finish first.
    ///
    /// The refusal stops the hunt from drawing another destination. It takes
    /// nothing away from the ones that already probe: those measured what they
    /// measured, and the file already holds the record that opened each of
    /// them.
    #[test]
    fn a_tracer_that_refuses_lets_the_destinations_in_flight_finish() {
        let mut probes = FakeProbes::refuses_after(
            &[REACHED_AT_FIVE, FAR_REACHED_AT_EIGHTEEN],
            2,
            NO_RAW_SOCKET,
        );
        let stopped = run_hunt(
            &[NEAR, FAR, QUIET],
            &mut probes,
            wanting(3).at_once(3),
            &never_stops(),
            &Names::None,
            &mut Recorder::default(),
        )
        .expect_err("the tracer of the third destination stops the hunt");
        let counts = counts(&stopped.summary);
        assert!(
            counts.starts_with("2/3 reached   2/64 targets   0 partial"),
            "the two destinations that stood when the tracer refused still count: {counts}"
        );
    }

    /// A tracer that refuses shows the indicator no destination.
    ///
    /// The indicator names the destination that started last, and it counts
    /// every destination the hunt showed it. A tracer that refuses starts
    /// nothing, so a line that named that address would name a destination the
    /// hunt never probed, and the count of the destinations in flight would
    /// stand one too high for the rest of the run.
    #[test]
    fn a_tracer_that_refuses_shows_the_indicator_no_destination() {
        let mut probes = FakeProbes::refuses_after(&[REACHED_AT_FIVE], 1, NO_RAW_SOCKET);
        let mut recorder = Recorder::default();
        run_hunt(
            &[NEAR, FAR],
            &mut probes,
            wanting(2).serial(),
            &never_stops(),
            &Names::None,
            &mut recorder,
        )
        .expect_err("the tracer of the second destination stops the hunt");
        assert_eq!(
            recorder.targets(),
            vec![address(NEAR)],
            "the indicator names the destination that started, and no other"
        );
    }

    /// A tracer that stops leaves the flights beside it closed.
    ///
    /// The run of the destination whose tracer died writes its own `end`
    /// record. The other destinations of the pool hold a `run` record, the
    /// rounds they recorded, and nothing that closes them. A reader of such a
    /// file reads those runs as a file that stops in the middle, so the hunt
    /// closes them itself.
    #[test]
    fn a_tracer_that_stops_leaves_the_flights_beside_it_closed() {
        let mut probes = FakeProbes::that_drops_the_sender_of(&[&[], FAR_REACHED_AT_EIGHTEEN], 0);
        let mut sink = Vec::new();
        let stopped = {
            let mut writer = Writer::to_sink(&mut sink);
            hunt_into(
                draw_of(&[NEAR, FAR]),
                &mut probes,
                wanting(2),
                &never_stops(),
                &Names::None,
                &mut writer,
                &mut Recorder::default(),
            )
        }
        .expect_err("the tracer of the first destination stops the hunt");
        assert!(
            matches!(stopped.fault, HuntError::Run(RunError::Tracer { .. })),
            "the fault is the tracer that stopped: {}",
            stopped.fault
        );
        let recording = read_back(&sink);
        let ids = recording.run_ids();
        assert_eq!(ids.len(), 2, "the file holds both runs: {ids:?}");
        let open: Vec<&str> = ids
            .iter()
            .filter(|id| {
                recording
                    .run(id)
                    .expect("the file holds every run it names")
                    .end()
                    .is_none()
            })
            .map(RunId::as_str)
            .collect();
        assert!(
            open.is_empty(),
            "every run of the file takes an end record, and these took none: {open:?}"
        );
    }

    /// A write that fails keeps the summary of the rounds in front of it.
    ///
    /// The disk that fills is the fault that this covers, and it is the one
    /// where the reader most wants the rounds that the hunt already measured.
    ///
    /// The hunt traces one destination at a time here, so the sink takes the
    /// records of exactly one destination and fails on the record that opens
    /// the next. A hunt of a pool writes the records of its destinations
    /// between each other, and a count of records would then name no
    /// destination in particular.
    #[test]
    fn a_hunt_that_a_write_stopped_keeps_the_summary_of_the_rounds_that_finished() {
        let mut probes = FakeProbes::of(&[REACHED_AT_FIVE, REACHED_AT_FIVE]);
        let mut writer = Writer::to_sink(Sink::that_takes(RECORDS_OF_ONE_DESTINATION));
        let stopped = hunt_into(
            draw_of(&[NEAR, FAR]),
            &mut probes,
            wanting(2).serial(),
            &never_stops(),
            &Names::None,
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
            counts.starts_with("1/2 reached   1/64 targets   0 partial"),
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
            wanting(1),
            &never_stops(),
            &Names::of(&[(NEAR, &[named(DESTINATION_NAME)])]),
            &mut Recorder::default(),
        )
        .expect("the hunt must finish");
        let named = names_in(&recording);
        assert!(
            named.contains(&DESTINATION_NAME.to_owned()),
            "the file holds the name of the destination: {named:?}"
        );
    }

    /// The name of every `name` record of a file, in order.
    fn names_in(recording: &Recording) -> Vec<String> {
        recording
            .records()
            .iter()
            .filter_map(|record| match record {
                Record::Name(name) => Some(name.host.clone()),
                _ => None,
            })
            .collect()
    }

    /// The number of probe rounds that each destination of the drain test
    /// takes.
    ///
    /// The near destination takes both of them and closes on the round limit.
    /// The far destination holds one round alone, so its channel runs dry and
    /// it stands in flight while the near one waits for its name.
    const TWO_PROBE_ROUNDS: u64 = 2;

    /// The two rounds of the destination that closes first.
    const NEAR_TWICE: &[&[(u8, &str, f64)]] = &[&[(5, NEAR, 20.0)], &[(5, NEAR, 21.0)]];

    /// The one round of the destination that stands beside it.
    const FAR_ONCE: &[&[(u8, &str, f64)]] = &[&[(18, FAR, 85.0)]];

    /// The answers that the near destination takes: nothing for four asks, and
    /// its name on the fifth.
    ///
    /// The two rounds of the run take the first two asks. The three that
    /// follow stand in the wait for the name, so that wait takes three asks
    /// whatever the speed of the machine.
    fn name_on_the_fifth_ask() -> [Lookup; 5] {
        [
            Lookup::Pending,
            Lookup::Pending,
            Lookup::Pending,
            Lookup::Pending,
            named(DESTINATION_NAME),
        ]
    }

    /// The longest that a destination of the drain test waits for its names,
    /// and the longest that one of them takes.
    ///
    /// The wait of this test ends at the moment the name arrives, and not at
    /// the moment the grace runs out. The value therefore stands well above
    /// the time that three steps of a wait take, and the destination beside
    /// the wait closes on it.
    const A_NAME_GRACE: Duration = Duration::from_millis(400);

    /// The number of asks about one address that one sweep gives.
    const ONE_ASK_A_SWEEP: usize = 1;

    /// The greatest number of asks about this address that stand in a row,
    /// with no ask about another address between them.
    fn asks_in_a_row(asked: &[IpAddr], addr: IpAddr) -> usize {
        let mut most = 0;
        let mut run = 0;
        for ask in asked {
            if *ask == addr {
                run += 1;
                most = most.max(run);
            } else {
                run = 0;
            }
        }
        most
    }

    /// A destination that waits for the names of its hops holds up no other.
    ///
    /// The near destination closes on the round limit and then waits for the
    /// name of its hop. The far destination still stands, because its channel
    /// holds one round of the two that the plan asks for.
    ///
    /// A sweep gives each destination of the pool one turn, so an ask of the
    /// far destination stands between every two asks of the near one. A wait
    /// that ran inside one turn would hold the sweep, and the three asks of
    /// that wait would then stand in a row.
    #[test]
    fn a_destination_that_waits_for_its_names_holds_up_no_other() {
        let names = Names::of(&[(NEAR, &name_on_the_fifth_ask()), (FAR, &[Lookup::Pending])]);
        let resolver = names.resolver();
        let mut probes = FakeProbes::of(&[NEAR_TWICE, FAR_ONCE]);
        let (_, recording) = run_hunt(
            &[NEAR, FAR],
            &mut probes,
            wanting(2)
                .probing(TWO_PROBE_ROUNDS)
                .waiting_for_names(A_NAME_GRACE, A_NAME_GRACE),
            &never_stops(),
            &names,
            &mut Recorder::default(),
        )
        .expect("the hunt must finish");
        let asked = resolver.asked();
        assert_eq!(
            asks_in_a_row(&asked, IpAddr::V4(address(NEAR))),
            ONE_ASK_A_SWEEP,
            "the destination beside the wait must take a turn between two asks of it: {asked:?}"
        );
        let hosts = names_in(&recording);
        assert!(
            hosts.contains(&DESTINATION_NAME.to_owned()),
            "the wait must still put the name of the destination in the file: {hosts:?}"
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
        assert_eq!(drawn(&mut draw), Some(address(ROUTABLE)));
        assert_eq!(drawn(&mut draw), Some(address(OTHER_ROUTABLE)));
    }

    /// A peek of a draw that ran out gives no address.
    #[test]
    fn a_peek_of_a_draw_that_ran_out_gives_no_address() {
        assert_eq!(draw_of(&[]).peek(), None);
    }

    /// The address of the first hit that every mine of a test digs around.
    ///
    /// The block of `93.184.0.0/16` around it holds no reserved block, so a
    /// mine of `--mine-prefix 16` there draws no address that the guard
    /// rejects.
    const HIT: &str = "93.184.216.34";

    /// The /24 that holds the first hit of a mine test.
    const HIT_PREFIX: &str = "93.184.216.0";

    /// The block that bounds a mine of the first hit at a prefix of 16.
    const HIT_BLOCK: &str = "93.184.0.0";

    /// The address of a first hit whose /16 holds two reserved /24s.
    ///
    /// `192.0.0.0/24` and `192.0.2.0/24` both stand inside `192.0.0.0/16`, and
    /// this address stands inside neither one.
    const HIT_BESIDE_RESERVED: &str = "192.0.1.5";

    /// The length of the path that the first hit of a mine test measured.
    const HIT_LENGTH: u8 = 20;

    /// The number of addresses that one mine of a test probes.
    const MINE_DEPTH: usize = 8;

    /// The length of the block that one mine of a test stays inside.
    const MINE_PREFIX: u8 = 16;

    /// The number of addresses that one mine of a test probes of any one /24.
    const MINE_PER_PREFIX: usize = 2;

    /// The number of addresses that the mine of the reserved test probes.
    ///
    /// The count is large, so the mine draws in many of the 256 /24s that its
    /// block holds and meets the two reserved ones among them.
    const A_DEEP_MINE: usize = 200;

    /// The plan of one mine that a test names.
    fn mine_plan(depth: usize, prefix: u8, per_prefix: usize, delay: Duration) -> MinePlan {
        MinePlan {
            depth: NonZeroUsize::new(depth).expect("a mine of a test probes one address at least"),
            prefix,
            per_prefix: NonZeroUsize::new(per_prefix)
                .expect("a mine of a test probes one address of one prefix at least"),
            delay,
        }
    }

    /// The plan of a mine of the defaults of the design, which waits for
    /// nothing between two addresses.
    fn a_mine() -> MinePlan {
        mine_plan(MINE_DEPTH, MINE_PREFIX, MINE_PER_PREFIX, Duration::ZERO)
    }

    /// A draw that mines the near space, over a scripted list of candidates.
    fn mining_draw(candidates: &[&str], plan: MinePlan) -> Draw {
        seeded_mining_draw(candidates, plan, SEED)
    }

    /// A draw that mines the near space, over the seed that a test names.
    fn seeded_mining_draw(candidates: &[&str], plan: MinePlan, seed: u64) -> Draw {
        draw_of(candidates).mining(plan, seed, Box::new(FakeClock::new()))
    }

    /// The addresses that one mine gives, after one first hit started it.
    fn mined_addresses(hit: &str, plan: MinePlan) -> Vec<Ipv4Addr> {
        seeded_mined_addresses(hit, plan, SEED)
    }

    /// The addresses that one mine of one seed gives.
    fn seeded_mined_addresses(hit: &str, plan: MinePlan, seed: u64) -> Vec<Ipv4Addr> {
        let mut draw = seeded_mining_draw(&[], plan, seed);
        draw.scored(address(hit), HIT_LENGTH);
        std::iter::from_fn(|| draw.mined())
            .map(|pick| pick.addr)
            .collect()
    }

    /// The number of addresses that the mine of a draw still holds.
    fn drained(draw: &mut Draw) -> usize {
        std::iter::from_fn(|| draw.mined()).count()
    }

    /// The /24 that holds one address.
    fn prefix_of(addr: Ipv4Addr) -> Ipv4Addr {
        Ipv4Addr::from_bits(addr.to_bits() & 0xffff_ff00)
    }

    #[test]
    fn a_mine_gives_no_more_addresses_than_its_depth() {
        assert_eq!(mined_addresses(HIT, a_mine()).len(), MINE_DEPTH);
    }

    #[test]
    fn a_mine_stays_inside_the_block_that_its_prefix_names() {
        let inside = Block::new(address(HIT_BLOCK), MINE_PREFIX);
        let addresses = mined_addresses(HIT, a_mine());
        assert!(!addresses.is_empty(), "the mine drew no address at all");
        for addr in addresses {
            assert!(inside.holds(addr), "the mine drew {addr}, outside {inside}");
        }
    }

    #[test]
    fn a_mine_probes_no_more_addresses_of_one_prefix_than_its_cap() {
        let mut counted: BTreeMap<Ipv4Addr, usize> = BTreeMap::new();
        for addr in mined_addresses(HIT, a_mine()) {
            *counted.entry(prefix_of(addr)).or_default() += 1;
        }
        assert!(!counted.is_empty(), "the mine drew no address at all");
        for (prefix, count) in counted {
            assert!(
                count <= MINE_PER_PREFIX,
                "the mine probed {count} addresses of {prefix}"
            );
        }
    }

    #[test]
    fn a_mine_starts_in_the_prefix_of_the_first_hit() {
        let addresses = mined_addresses(HIT, a_mine());
        let first = *addresses.first().expect("the mine gives an address");
        assert_eq!(prefix_of(first), address(HIT_PREFIX));
    }

    /// A mine draws a sibling /24 once the first one holds its cap.
    ///
    /// The depth of this mine is four times its cap, so the mine fills four
    /// /24s: the one of the first hit, and three siblings of it.
    #[test]
    fn a_mine_draws_a_sibling_prefix_once_the_first_one_holds_its_cap() {
        let addresses = mined_addresses(HIT, a_mine());
        let prefixes: HashSet<Ipv4Addr> = addresses.iter().copied().map(prefix_of).collect();
        assert_eq!(prefixes.len(), MINE_DEPTH / MINE_PER_PREFIX);
    }

    #[test]
    fn a_mine_gives_no_address_twice() {
        let addresses = mined_addresses(HIT, a_mine());
        assert!(!addresses.is_empty(), "the mine drew no address at all");
        let distinct: HashSet<Ipv4Addr> = addresses.iter().copied().collect();
        assert_eq!(distinct.len(), addresses.len());
    }

    #[test]
    fn a_mine_draws_no_network_no_broadcast_and_no_gateway() {
        let addresses = mined_addresses(HIT, a_mine());
        assert!(!addresses.is_empty(), "the mine drew no address at all");
        for addr in addresses {
            let host = addr.octets()[3];
            assert!(
                (FIRST_HOST..=LAST_HOST).contains(&host),
                "the mine drew {addr}"
            );
        }
    }

    #[test]
    fn a_mine_rejects_an_address_that_no_packet_routes_to() {
        let plan = mine_plan(A_DEEP_MINE, MINE_PREFIX, MINE_PER_PREFIX, Duration::ZERO);
        let addresses = mined_addresses(HIT_BESIDE_RESERVED, plan);
        assert_eq!(addresses.len(), A_DEEP_MINE, "the mine drew too few addresses");
        for addr in addresses {
            assert_eq!(
                reserved(addr),
                None,
                "the mine drew {addr}, which no packet routes to"
            );
        }
    }

    /// A mine gives no address that the hunt already visited.
    ///
    /// The mine below stands inside one /24, and the draw already gave every
    /// host of that /24 but one. The mine therefore holds one address to give,
    /// and it must be that one.
    #[test]
    fn a_mine_gives_no_address_that_the_hunt_already_visited() {
        let free = address("93.184.216.7");
        let taken: Vec<String> = (FIRST_HOST..=LAST_HOST)
            .map(|host| format!("93.184.216.{host}"))
            .filter(|text| *text != free.to_string())
            .collect();
        let names: Vec<&str> = taken.iter().map(String::as_str).collect();
        let mut draw = mining_draw(&names, mine_plan(4, 24, 4, Duration::ZERO));
        while drawn(&mut draw).is_some() {}
        draw.scored(address(HIT), HIT_LENGTH);
        let mined: Vec<Ipv4Addr> = std::iter::from_fn(|| draw.mined())
            .map(|pick| pick.addr)
            .collect();
        assert_eq!(mined, vec![free]);
    }

    #[test]
    fn a_mine_of_one_seed_gives_the_same_addresses() {
        let addresses = mined_addresses(HIT, a_mine());
        assert!(!addresses.is_empty(), "the mine drew no address at all");
        assert_eq!(addresses, mined_addresses(HIT, a_mine()));
    }

    #[test]
    fn a_mine_of_another_seed_gives_other_addresses() {
        assert_ne!(
            seeded_mined_addresses(HIT, a_mine(), SEED),
            seeded_mined_addresses(HIT, a_mine(), OTHER_SEED)
        );
    }

    #[test]
    fn the_address_of_a_mine_names_the_first_hit_that_started_it() {
        let mut draw = mining_draw(&[], a_mine());
        draw.scored(address(HIT), HIT_LENGTH);
        let pick = draw.mined().expect("the mine gives an address");
        assert_eq!(pick.mine, Some(address(HIT)));
    }

    #[test]
    fn an_independent_address_names_no_mine() {
        assert_eq!(
            draw_of(&[ROUTABLE]).address(),
            Some(Pick {
                addr: address(ROUTABLE),
                mine: None,
            })
        );
    }

    /// A draw that the hunt did not ask to mine gives no mined address.
    #[test]
    fn a_draw_that_mines_nothing_gives_no_mined_address() {
        let mut draw = draw_of(&[ROUTABLE]);
        draw.scored(address(HIT), HIT_LENGTH);
        assert_eq!(draw.mined(), None);
    }

    #[test]
    fn a_mine_that_gave_every_address_of_its_depth_stands_no_longer() {
        let mut draw = mining_draw(&[], a_mine());
        draw.scored(address(HIT), HIT_LENGTH);
        for place in 0..MINE_DEPTH {
            assert!(
                draw.mined().is_some(),
                "the mine must give address {place} of {MINE_DEPTH}"
            );
        }
        assert_eq!(draw.mined(), None);
    }

    /// The first result of a hunt is the longest path it measured, so it starts
    /// a mine.
    #[test]
    fn the_first_result_of_a_hunt_starts_a_mine() {
        let mut draw = mining_draw(&[], a_mine());
        draw.scored(address(HIT), HIT_LENGTH);
        assert!(draw.mined().is_some());
    }

    #[test]
    fn a_result_no_longer_than_the_record_starts_no_mine() {
        let mut draw = mining_draw(&[], a_mine());
        draw.scored(address(HIT), HIT_LENGTH);
        assert_eq!(drained(&mut draw), MINE_DEPTH);
        draw.scored(address(FAR), HIT_LENGTH);
        assert_eq!(draw.mined(), None);
    }

    #[test]
    fn a_result_shorter_than_the_record_starts_no_mine() {
        let mut draw = mining_draw(&[], a_mine());
        draw.scored(address(HIT), HIT_LENGTH);
        assert_eq!(drained(&mut draw), MINE_DEPTH);
        draw.scored(address(FAR), HIT_LENGTH - 1);
        assert_eq!(draw.mined(), None);
    }

    #[test]
    fn a_result_longer_than_the_record_starts_a_mine() {
        let mut draw = mining_draw(&[], a_mine());
        draw.scored(address(HIT), HIT_LENGTH);
        assert_eq!(drained(&mut draw), MINE_DEPTH);
        draw.scored(address(FAR), HIT_LENGTH + 1);
        let pick = draw.mined().expect("the mine of the new record gives an address");
        assert_eq!(pick.mine, Some(address(FAR)));
    }

    /// A new record replaces the mine that stands.
    ///
    /// The near space of a destination that no longer holds the record is
    /// worth less than the near space of the one that does, and one mine at a
    /// time is what keeps the caps of the design meaningful.
    #[test]
    fn a_new_record_replaces_the_mine_that_stands() {
        let mut draw = mining_draw(&[], a_mine());
        draw.scored(address(HIT), HIT_LENGTH);
        draw.scored(address(FAR), HIT_LENGTH + 1);
        let mines: HashSet<Option<Ipv4Addr>> = std::iter::from_fn(|| draw.mined())
            .map(|pick| pick.mine)
            .collect();
        assert_eq!(mines, HashSet::from([Some(address(FAR))]));
    }
}
