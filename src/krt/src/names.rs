//! The reverse DNS fold of one run: every address that a round reports, in one
//! `name` record each.
//!
//! A run sees the same address at many TTLs and in many rounds, and a reverse
//! lookup of an address takes a time that no round waits for. This module holds
//! the two sets that answer both of those facts: the addresses whose lookups
//! have not finished, and the addresses that need no further ask. A turn of the
//! run hands the hops of a round to the fold, and the fold answers with one
//! record for each name that arrived since the turn before it.
//!
//! The fold asks a resolver and reads nothing else, so a test drives it without
//! a network and without a name server.

use crate::record::{Hop, NameRecord, RunId};
use chrono::{DateTime, Utc};
use std::collections::{BTreeSet, HashSet};
use std::net::IpAddr;

/// What a reverse lookup of one address holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Lookup {
    /// The lookup finished, and the address carries this name.
    Named(String),
    /// The lookup finished, and the address carries no name. A lookup that
    /// failed and a lookup that timed out both land here, because the run shows
    /// the raw address for each of them and asks no second time.
    Nameless,
    /// The lookup has not finished. The caller asks again on a later turn.
    Pending,
}

/// A reverse resolver that never blocks its caller.
pub(crate) trait Resolver {
    /// Asks for the name of one address, and answers with whatever the resolver
    /// holds now.
    ///
    /// The first ask starts the lookup, and every ask after it reads the
    /// answer. No ask waits for the network.
    fn lookup(&self, addr: IpAddr) -> Lookup;
}

/// A resolver that looks nothing up.
///
/// `--no-dns` becomes this resolver, so no branch of the run loop reads the
/// switch and no lookup leaves the machine.
pub(crate) struct NoLookups;

impl Resolver for NoLookups {
    fn lookup(&self, _addr: IpAddr) -> Lookup {
        Lookup::Nameless
    }
}

/// Turns the addresses that a run reports into `name` records.
pub(crate) struct Namer {
    /// The resolver that every ask of this namer goes to.
    resolver: Box<dyn Resolver>,
    /// The identifier that every record of this namer carries.
    run: RunId,
    /// The addresses whose lookups have not finished. The set is ordered, so
    /// the records of one turn come out in one order whatever the run.
    waiting: BTreeSet<IpAddr>,
    /// The addresses that need no further ask, whether they carry a name or
    /// not.
    settled: HashSet<IpAddr>,
}

impl Namer {
    /// A namer that asks this resolver and stamps every record with this run.
    pub(crate) fn new(resolver: Box<dyn Resolver>, run: RunId) -> Self {
        Self {
            resolver,
            run,
            waiting: BTreeSet::new(),
            settled: HashSet::new(),
        }
    }

    /// Reads the addresses that a round reported, asks the resolver about every
    /// address whose lookup has not finished, and answers with one record for
    /// each name that arrived.
    ///
    /// A turn that saw no round passes an empty slice, so a lookup that
    /// finishes between two rounds still lands.
    ///
    /// One address takes one record in one run, whatever the number of TTLs it
    /// answers at and whatever the number of rounds it appears in.
    pub(crate) fn names(&mut self, hops: &[Hop], now: DateTime<Utc>) -> Vec<NameRecord> {
        // An address that settled needs no further ask, so no round puts it
        // back into the set of the addresses that wait. One address arrives at
        // many TTLs of one round and in many rounds of one run, and the two
        // sets together hold one entry for it.
        for hop in hops {
            if !self.settled.contains(&hop.addr) {
                self.waiting.insert(hop.addr);
            }
        }

        let mut records = Vec::new();
        let mut settled_now = Vec::new();
        for addr in &self.waiting {
            match self.resolver.lookup(*addr) {
                Lookup::Named(host) => {
                    records.push(NameRecord {
                        run: self.run.clone(),
                        ts: now,
                        addr: *addr,
                        host,
                    });
                    settled_now.push(*addr);
                }
                Lookup::Nameless => settled_now.push(*addr),
                Lookup::Pending => {}
            }
        }

        // The loop above reads the set of the addresses that wait, so the moves
        // between the two sets come after it.
        for addr in settled_now {
            self.waiting.remove(&addr);
            self.settled.insert(addr);
        }

        records
    }
}

#[cfg(test)]
mod tests {
    use super::{Lookup, Namer, NoLookups, Resolver};
    use crate::record::{Hop, NameRecord, RunId};
    use crate::testing::{address, round};
    use chrono::{DateTime, Utc};
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, VecDeque};
    use std::net::IpAddr;
    use std::rc::Rc;

    /// The identifier of the run that every test of this module folds.
    const RUN: &str = "2026-08-23T12:00:00.000Z";

    /// The moment of the first turn of a test.
    const FIRST_MOMENT: &str = "2026-08-23T12:00:01.000Z";

    /// The moment of the turn after the first one.
    const LATER_MOMENT: &str = "2026-08-23T12:00:02.000Z";

    /// The first TTL that every test round probes.
    const FIRST_TTL: u8 = 1;

    /// The last TTL that every test round probes.
    const LAST_TTL: u8 = 3;

    /// The round-trip time of a hop that no test of this module reads.
    const ANY_RTT: f64 = 1.5;

    /// The address of the first router of the test path.
    const FIRST_HOP: &str = "192.168.1.1";

    /// The name of the first router of the test path.
    const FIRST_HOP_NAME: &str = "gateway.example.com";

    /// The address of one more router of the test path.
    ///
    /// The address stands below `TARGET` in address order, and a test of the
    /// order of the records reads that.
    const LEFT_ROUTER: &str = "10.0.0.1";

    /// The name of that router.
    const LEFT_ROUTER_NAME: &str = "left.example.com";

    /// The address of the target of the test path.
    const TARGET: &str = "93.184.216.34";

    /// The name of the target of the test path.
    const TARGET_NAME: &str = "example.com";

    /// Reads a moment that a test names.
    fn moment(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("the test moment must parse")
            .with_timezone(&Utc)
    }

    /// The hops of one round: each one a TTL, and the address that answered at
    /// that TTL.
    fn hops(answers: &[(u8, &str)]) -> Vec<Hop> {
        let answers: Vec<(u8, &str, f64)> = answers
            .iter()
            .map(|(ttl, addr)| (*ttl, *addr, ANY_RTT))
            .collect();
        round(FIRST_TTL, LAST_TTL, &answers).hops
    }

    /// The answer of a lookup that finished with a name.
    fn named(host: &str) -> Lookup {
        Lookup::Named(host.to_owned())
    }

    /// The record that the name of one address takes at one moment.
    fn record(ts: &str, addr: &str, host: &str) -> NameRecord {
        NameRecord {
            run: RunId::from(RUN),
            ts: moment(ts),
            addr: address(addr),
            host: host.to_owned(),
        }
    }

    /// A resolver that a test programs: one answer for each ask of one address,
    /// and the last answer of the list for every ask after the list runs out.
    ///
    /// An address that the test named no answer for answers `Nameless`.
    ///
    /// The counts and the answers sit behind a `Cell` and a `RefCell`, because
    /// [`Resolver::lookup`] takes the resolver by reference. The fake stays on
    /// one thread.
    struct FakeResolver {
        /// The answers that each address holds, the next answer first.
        answers: RefCell<HashMap<IpAddr, VecDeque<Lookup>>>,
        /// The number of asks that the resolver took.
        asks: Cell<usize>,
    }

    impl FakeResolver {
        /// A resolver that answers each address with the answers of its list.
        fn new(answers: &[(&str, &[Lookup])]) -> Rc<Self> {
            Rc::new(Self {
                answers: RefCell::new(
                    answers
                        .iter()
                        .map(|(addr, list)| (address(addr), list.iter().cloned().collect()))
                        .collect(),
                ),
                asks: Cell::new(0),
            })
        }

        /// The number of asks that the resolver took.
        fn asks(&self) -> usize {
            self.asks.get()
        }

        /// The answer of one ask, and one step along the list of that address.
        fn answer(&self, addr: IpAddr) -> Lookup {
            self.asks.set(self.asks.get() + 1);
            let mut answers = self.answers.borrow_mut();
            let Some(queue) = answers.get_mut(&addr) else {
                return Lookup::Nameless;
            };
            // The last answer of a list stands for every ask after it, so the
            // list keeps that answer in the place of a step.
            let answer = if queue.len() > 1 {
                queue.pop_front()
            } else {
                queue.front().cloned()
            };
            answer.unwrap_or(Lookup::Nameless)
        }
    }

    impl Resolver for Rc<FakeResolver> {
        fn lookup(&self, addr: IpAddr) -> Lookup {
            self.answer(addr)
        }
    }

    /// A namer of the test run that asks this resolver.
    fn namer(resolver: &Rc<FakeResolver>) -> Namer {
        Namer::new(Box::new(Rc::clone(resolver)), RunId::from(RUN))
    }

    #[test]
    fn an_address_that_resolves_gives_one_record_of_the_run() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[named(FIRST_HOP_NAME)])]);
        let mut namer = namer(&resolver);
        let records = namer.names(&hops(&[(1, FIRST_HOP)]), moment(FIRST_MOMENT));
        assert_eq!(
            records,
            vec![record(FIRST_MOMENT, FIRST_HOP, FIRST_HOP_NAME)],
            "the record carries the run, the moment, the address, and the name"
        );
    }

    #[test]
    fn an_address_with_no_name_gives_no_record_and_the_run_goes_on() {
        let resolver = FakeResolver::new(&[
            (FIRST_HOP, &[Lookup::Nameless]),
            (TARGET, &[named(TARGET_NAME)]),
        ]);
        let mut namer = namer(&resolver);
        let first = namer.names(&hops(&[(1, FIRST_HOP)]), moment(FIRST_MOMENT));
        assert!(first.is_empty(), "an address with no name takes no record");
        let later = namer.names(&hops(&[(3, TARGET)]), moment(LATER_MOMENT));
        assert_eq!(
            later,
            vec![record(LATER_MOMENT, TARGET, TARGET_NAME)],
            "the run names the addresses that follow an address with no name"
        );
    }

    #[test]
    fn an_address_that_answers_late_gives_its_record_on_the_later_turn() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[Lookup::Pending, named(FIRST_HOP_NAME)])]);
        let mut namer = namer(&resolver);
        let first = namer.names(&hops(&[(1, FIRST_HOP)]), moment(FIRST_MOMENT));
        assert!(
            first.is_empty(),
            "a lookup that has not finished takes no record"
        );
        let later = namer.names(&hops(&[(1, FIRST_HOP)]), moment(LATER_MOMENT));
        assert_eq!(
            later,
            vec![record(LATER_MOMENT, FIRST_HOP, FIRST_HOP_NAME)],
            "the record carries the moment that the name arrived"
        );
    }

    #[test]
    fn an_address_in_two_rounds_gives_one_record() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[named(FIRST_HOP_NAME)])]);
        let mut namer = namer(&resolver);
        let first = namer.names(&hops(&[(1, FIRST_HOP)]), moment(FIRST_MOMENT));
        assert_eq!(
            first,
            vec![record(FIRST_MOMENT, FIRST_HOP, FIRST_HOP_NAME)],
            "the first round of the address takes the record"
        );
        let later = namer.names(&hops(&[(1, FIRST_HOP)]), moment(LATER_MOMENT));
        assert!(
            later.is_empty(),
            "the second round of the address takes no second record"
        );
    }

    #[test]
    fn an_address_at_two_ttls_of_one_round_gives_one_record() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[named(FIRST_HOP_NAME)])]);
        let mut namer = namer(&resolver);
        let records = namer.names(
            &hops(&[(1, FIRST_HOP), (2, FIRST_HOP)]),
            moment(FIRST_MOMENT),
        );
        assert_eq!(
            records,
            vec![record(FIRST_MOMENT, FIRST_HOP, FIRST_HOP_NAME)],
            "one address takes one record, whatever the number of TTLs it answers at"
        );
    }

    #[test]
    fn an_address_that_holds_a_name_is_never_asked_again() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[named(FIRST_HOP_NAME)])]);
        let mut namer = namer(&resolver);
        namer.names(&hops(&[(1, FIRST_HOP)]), moment(FIRST_MOMENT));
        let asks = resolver.asks();
        assert_eq!(asks, 1, "the first turn asked the resolver once");
        namer.names(&hops(&[(1, FIRST_HOP)]), moment(LATER_MOMENT));
        assert_eq!(
            resolver.asks(),
            asks,
            "an address that holds a name takes no further ask"
        );
    }

    #[test]
    fn an_address_that_holds_no_name_is_never_asked_again() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[Lookup::Nameless])]);
        let mut namer = namer(&resolver);
        namer.names(&hops(&[(1, FIRST_HOP)]), moment(FIRST_MOMENT));
        let asks = resolver.asks();
        assert_eq!(asks, 1, "the first turn asked the resolver once");
        namer.names(&hops(&[(1, FIRST_HOP)]), moment(LATER_MOMENT));
        assert_eq!(
            resolver.asks(),
            asks,
            "an address that holds no name takes no further ask"
        );
    }

    #[test]
    fn a_turn_that_saw_no_round_still_asks_about_the_addresses_that_wait() {
        let resolver = FakeResolver::new(&[(FIRST_HOP, &[Lookup::Pending, named(FIRST_HOP_NAME)])]);
        let mut namer = namer(&resolver);
        let first = namer.names(&hops(&[(1, FIRST_HOP)]), moment(FIRST_MOMENT));
        assert!(
            first.is_empty(),
            "a lookup that has not finished takes no record"
        );
        let later = namer.names(&[], moment(LATER_MOMENT));
        assert_eq!(
            later,
            vec![record(LATER_MOMENT, FIRST_HOP, FIRST_HOP_NAME)],
            "a lookup that finishes between two rounds lands on the turn that saw no round"
        );
    }

    #[test]
    fn two_addresses_that_resolve_on_one_turn_give_two_records_in_address_order() {
        let resolver = FakeResolver::new(&[
            (TARGET, &[named(TARGET_NAME)]),
            (LEFT_ROUTER, &[named(LEFT_ROUTER_NAME)]),
        ]);
        let mut namer = namer(&resolver);
        // The target answers at the lower TTL, so the hops name it first. The
        // records must still run in address order, because `10.0.0.1` stands
        // below `93.184.216.34`.
        let records = namer.names(
            &hops(&[(1, TARGET), (2, LEFT_ROUTER)]),
            moment(FIRST_MOMENT),
        );
        assert_eq!(
            records,
            vec![
                record(FIRST_MOMENT, LEFT_ROUTER, LEFT_ROUTER_NAME),
                record(FIRST_MOMENT, TARGET, TARGET_NAME),
            ],
            "the records of one turn run in address order"
        );
    }

    #[test]
    fn the_resolver_that_looks_nothing_up_gives_no_record_for_any_round() {
        assert_eq!(
            NoLookups.lookup(address(FIRST_HOP)),
            Lookup::Nameless,
            "the resolver that looks nothing up names no address"
        );
        let mut namer = Namer::new(Box::new(NoLookups), RunId::from(RUN));
        assert!(
            namer
                .names(&hops(&[(1, FIRST_HOP), (3, TARGET)]), moment(FIRST_MOMENT))
                .is_empty(),
            "no address of the first round takes a record"
        );
        assert!(
            namer
                .names(&hops(&[(1, FIRST_HOP)]), moment(LATER_MOMENT))
                .is_empty(),
            "no address of a later round takes a record"
        );
    }
}
