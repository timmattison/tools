//! The hunt: many destinations, one file, and the table that ranks them.
//!
//! A hunt draws an address, traces it, scores the path it found, and takes the
//! next round. It stops when it runs out of rounds, and it prints one table of
//! four rows: the shortest path, the longest path, the fastest path, and the
//! slowest path.
//!
//! The source of the addresses is a seam. The draw takes an iterator of
//! candidates, so a test hands it a list and reads what the hunt did with it,
//! and no test sends a packet. The seeded iterator of [`random`] is the source
//! of a real hunt, and `--seed` is what makes one hunt repeat another.
//!
//! The draw rejects every candidate that no packet routes to, and it rejects
//! every candidate it already gave. A hunt therefore traces each destination
//! once, and it spends no round on an address that answers nothing by
//! construction.

use crate::live::Screen;
use crate::record::{NameRecord, RoundRecord, RunId};
use crate::stats::{HopTable, TtlRow};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr};

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
    Block::new(Ipv4Addr::new(0, 0, 0, 0), 8),
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
    Block::new(Ipv4Addr::new(255, 255, 255, 255), 32),
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
}

impl Draw {
    /// Builds the draw over a source of candidates.
    pub(crate) fn new(candidates: Box<dyn Iterator<Item = Ipv4Addr>>) -> Self {
        Self {
            candidates,
            visited: HashSet::new(),
        }
    }

    /// Builds the draw of a real hunt, over the seeded sequence of [`random`].
    pub(crate) fn seeded(seed: u64) -> Self {
        Self::new(Box::new(random(seed)))
    }

    /// The next address to trace.
    ///
    /// The draw reads candidates until one of them routes and is new to this
    /// hunt. A source that ran out, and a source that gave [`ATTEMPTS`]
    /// candidates that the draw rejected, both give no address, and the hunt
    /// then stops.
    pub(crate) fn address(&mut self) -> Option<Ipv4Addr> {
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

#[cfg(test)]
mod tests {
    use super::{random, reserved, Draw, PathKind, Score, Scorer, ATTEMPTS};
    use crate::live::Screen;
    use crate::record::{NameRecord, RunId};
    use crate::testing::round;
    use chrono::Utc;
    use std::collections::HashSet;
    use std::net::{IpAddr, Ipv4Addr};

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
    const MAX_TTL: u8 = 8;

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
        let mut scorer = Scorer::new(address(DESTINATION), RunId::from(RUN), FIRST_TTL);
        for hops in rounds {
            scorer.round(&round(FIRST_TTL, MAX_TTL, hops));
        }
        let records: Vec<NameRecord> = names
            .iter()
            .map(|(addr, host)| NameRecord {
                run: RunId::from(RUN),
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
}
