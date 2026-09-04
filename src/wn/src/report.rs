//! What the chain says once every issue in it carries a state.
//!
//! The question the tool answers is one question: which issue do I start now.
//! The answer is the first issue of the chain that is still open. Everything
//! else this module holds is there because the real answer has to survive a
//! chain that is not a clean run of closed issues followed by open ones.
//!
//! Two things break that clean shape, and both are worth saying out loud:
//!
//! * An issue that is closed after the next one. Somebody did a later issue
//!   first. The order still holds, so the answer does not change, but the plan
//!   the reader holds in their head is now wrong and nothing else would say so.
//! * An issue GitHub does not have. A typo in a number is invisible in the
//!   answer, because a chain of five issues with one missing still names an
//!   issue to start. So a missing issue is never the answer, and the tool
//!   reports it.
//!
//! # A step of a plan holds two numbers
//!
//! A plan of parallel work writes a step as a pull request and the issue that
//! pull request closes: `PR#344 (#341)`. Both numbers carry a state, and the
//! two states can disagree. The state of the step is the state of the pull
//! request, because the pull request is the work. The state of the issue
//! stands beside it, because a merged pull request over an open issue is a
//! link nobody wrote, and nothing else would say so.
//!
//! A plan also names one number in two streams. [`States`] holds the answer
//! of GitHub for each number once, so one query answers the whole plan and
//! every stream that names a number reads the same state for it.
//!
//! # One report answers a chain, a stream, and a graph
//!
//! [`Report`] carries the answer of all three shapes of input, and it carries
//! it in one shape. A chain and a stream stand in one line, so each of them
//! names one issue to start. A picture holds two streams that join, so it
//! names a set of them and it says what each blocked step waits for.
//!
//! The three must not part company. A stream is a graph whose nodes stand in
//! one line, so every question a reader asks of a chain is a question a reader
//! asks of a picture: which work is missing, which pair disagrees, and which
//! work somebody closed out of order. Two modules that answer one question
//! drift apart, and the reader then reads two answers to it.
//!
//! So the fields below hold the answer of a graph, and a chain and a stream
//! fill the same fields with the answer they always gave: `ready` holds one
//! position for a chain and a set for a picture, and `waits` is empty at every
//! position of a chain.

use std::collections::HashMap;

use crate::chain::IssueNumber;
use crate::graph::Graph;
use crate::plan::Step;

/// The list a position outside the report gives.
///
/// A named constant, because [`Report::waits_for`] gives a slice and an empty
/// `Vec` of its own would live no longer than the call.
const NO_NUMBERS: &[IssueNumber] = &[];

/// What GitHub says about one issue of the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Open, and thus work that is still to do.
    Open,
    /// Closed, and closed because the work was done. A merged pull request
    /// counts here as well.
    Done,
    /// Closed without the work being done: an issue closed as not planned or
    /// as a duplicate, or a pull request closed without a merge. It is not
    /// work to start, and it is not work that happened.
    Dropped,
    /// The repository holds no issue and no pull request with this number.
    Missing,
}

impl Status {
    /// Is this an issue somebody can start now?
    #[must_use]
    pub fn is_open(self) -> bool {
        self == Self::Open
    }

    /// Is this an issue nobody has to look at again?
    ///
    /// A dropped issue counts, because the chain moves past it. A missing one
    /// does not, because nothing is known about it.
    #[must_use]
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Done | Self::Dropped)
    }
}

/// The issue the pull request of a step closes, with what GitHub says about
/// it.
///
/// It carries no title. A row writes one title, and that title is the title of
/// the work. The number and the state are what the row needs beside it: the
/// number so the reader sees which issue the work finishes, and the state so a
/// pull request that finished nothing shows itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closes {
    /// The number of the issue the work closes.
    pub number: IssueNumber,
    /// What GitHub says about that issue.
    pub status: Status,
}

/// One issue of the chain, with what GitHub says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The number, as the chain wrote it.
    pub number: IssueNumber,
    /// The one-line title. Empty for a [`Status::Missing`] entry, which has no
    /// title to carry.
    pub title: String,
    /// What GitHub says about it.
    pub status: Status,
    /// The issue this work closes, when the step names one. `None` for every
    /// step of a chain, because a chain writes one number for each step.
    pub closes: Option<Closes>,
}

impl Entry {
    /// The number a row writes: `#344` for a step of one number, and
    /// `#344 (#341)` for a pair.
    ///
    /// The width of the number column of a block is the width of the widest of
    /// these, so the text of a pair and the width of the column come out of one
    /// place and can never part company.
    #[must_use]
    pub fn label(&self) -> String {
        match self.closes {
            Some(closes) => format!("{} ({})", self.number, closes.number),
            None => self.number.to_string(),
        }
    }
}

/// What GitHub says about each number of a plan, keyed by the number.
///
/// A plan asks GitHub once for the whole page of text. One number stands in
/// two streams, and two queries for it cost twice and can give two answers. So
/// the answers arrive as one list, and each stream reads the numbers it names
/// out of this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct States {
    /// The answer of GitHub for each number it answered for.
    entries: HashMap<IssueNumber, Entry>,
}

impl States {
    /// Hold what GitHub said about each number of `entries`.
    ///
    /// GitHub answers once for each number the query names, so two answers for
    /// one number cannot arrive. The last of them stands if one ever does.
    #[must_use]
    pub fn of(entries: Vec<Entry>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|entry| (entry.number, entry))
                .collect(),
        }
    }

    /// What GitHub says about `number`.
    ///
    /// A number nobody asked about gives a [`Status::Missing`] entry with no
    /// title, and never a panic. A number arrives here out of text a reader
    /// pasted, so a number the query missed must stay one row of the output
    /// and must not stop the run.
    #[must_use]
    pub fn entry(&self, number: IssueNumber) -> Entry {
        self.entries.get(&number).cloned().unwrap_or_else(|| Entry {
            number,
            title: String::new(),
            status: Status::Missing,
            closes: None,
        })
    }
}

/// The steps of the input, in order, and the answer they give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The steps, in the order the reader wrote them. A picture writes them in
    /// a topological order, so every step stands after the steps it waits for
    /// in this list as well.
    entries: Vec<Entry>,
    /// The positions of the entries somebody can start now.
    ///
    /// A chain and a stream hold one position here, because one line of work
    /// has one issue to start. A picture holds one position for each stream
    /// that is ready, because two streams that join are two people who work at
    /// the same time.
    ready: Vec<usize>,
    /// The positions that are finished with unfinished work before them.
    out_of_order: Vec<usize>,
    /// What each entry waits for, at the position of that entry. Empty at
    /// every position of a chain and of a stream, because a reader of one line
    /// of work reads the line above the row and needs no list.
    waits: Vec<Vec<IssueNumber>>,
}

impl Report {
    /// Read the answer out of the states of the chain.
    ///
    /// One line of work has one issue to start: the first open one. Every
    /// finished step after it is work somebody closed out of order, because
    /// each of them stands after a step that is not finished.
    #[must_use]
    pub fn build(entries: Vec<Entry>) -> Self {
        let next = entries.iter().position(|entry| entry.status.is_open());
        let out_of_order = next.map_or_else(Vec::new, |next| {
            entries
                .iter()
                .enumerate()
                .skip(next + 1)
                .filter(|(_, entry)| entry.status.is_finished())
                .map(|(position, _)| position)
                .collect()
        });
        let waits = vec![Vec::new(); entries.len()];
        Self {
            entries,
            ready: next.into_iter().collect(),
            out_of_order,
            waits,
        }
    }

    /// The report of one stream of a plan.
    ///
    /// The state of a step is the state of the number the step names, because
    /// the pull request of a pair is the work. So the issue to start is the
    /// first step whose pull request is open, and a merged pull request is
    /// walked past even while the issue it names stays open.
    ///
    /// The state of that issue travels with the step, so the reader sees the
    /// two together.
    #[must_use]
    pub fn of_steps(steps: &[Step], states: &States) -> Self {
        Self::build(entries_of(steps, states))
    }

    /// The report of a plan drawn as a picture.
    ///
    /// The entries stand in the order the graph holds them, which is a
    /// topological order with a tie going to the text. Each of them is read
    /// the way a step of a stream is read, because a step is one step
    /// whichever shape wrote it.
    ///
    /// An open step waits for every step before it that is not finished, and
    /// it is ready when it waits for nothing. The two answers come out of one
    /// list, so a row that is marked ready can never name work it waits for.
    ///
    /// A step the repository does not have is not finished, so a step behind a
    /// number nobody can read waits for that number. The note about a missing
    /// number then says why nobody starts the work.
    ///
    /// Only an open step waits for anything. A finished step is work nobody
    /// looks at again, and the note about work closed out of order is what a
    /// reader needs of it.
    ///
    /// A finished step is out of order when a step the wires reach behind it is
    /// not finished, at any distance. A chain answers this question the same
    /// way: it names every finished issue that stands after the first open one,
    /// and not the one issue beside it. The steps stand in a topological order,
    /// so one forward pass carries the answer of each step to the steps after
    /// it.
    #[must_use]
    pub fn of_graph(graph: &Graph, states: &States) -> Self {
        let entries = entries_of(graph.steps(), states);
        let mut ready: Vec<usize> = Vec::new();
        let mut waits: Vec<Vec<IssueNumber>> = vec![Vec::new(); entries.len()];
        let mut out_of_order: Vec<usize> = Vec::new();
        // Is a step the wires reach behind the step at this position not
        // finished? Every such step stands earlier in the list, because the
        // steps stand in a topological order.
        let mut unfinished_behind: Vec<bool> = vec![false; entries.len()];
        for (position, entry) in entries.iter().enumerate() {
            let before = graph.before(position);
            let unfinished: Vec<IssueNumber> = before
                .iter()
                .filter(|&&earlier| !entries[earlier].status.is_finished())
                .map(|&earlier| entries[earlier].number)
                .collect();
            unfinished_behind[position] =
                !unfinished.is_empty() || before.iter().any(|&earlier| unfinished_behind[earlier]);
            if entry.status.is_open() {
                if unfinished.is_empty() {
                    ready.push(position);
                }
                waits[position] = unfinished;
            } else if entry.status.is_finished() && unfinished_behind[position] {
                out_of_order.push(position);
            }
        }
        Self {
            entries,
            ready,
            out_of_order,
            waits,
        }
    }

    /// The chain, in the order it was written.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The first position in [`entries`](Self::entries) somebody can start, or
    /// `None` when nothing is ready.
    ///
    /// A chain and a stream have one such position, so this is their whole
    /// answer. A picture has one for each ready stream, and a caller that
    /// names every one of them reads [`is_ready`](Self::is_ready) instead.
    #[must_use]
    pub fn next(&self) -> Option<usize> {
        self.ready.first().copied()
    }

    /// The first issue somebody can start, or `None` when nothing is ready.
    #[must_use]
    pub fn next_entry(&self) -> Option<&Entry> {
        self.next().map(|position| &self.entries[position])
    }

    /// Can somebody start the entry at `position` now?
    ///
    /// A row asks this of each entry, because a picture marks one row for each
    /// stream that is ready. A position outside the report gives `false`,
    /// because a position nobody has is work nobody starts.
    #[must_use]
    pub fn is_ready(&self, position: usize) -> bool {
        self.ready.contains(&position)
    }

    /// The numbers the entry at `position` waits for, in the order the input
    /// holds them.
    ///
    /// Empty for every entry of a chain and of a stream, and empty for an
    /// entry of a picture that is ready or finished. A position outside the
    /// report gives an empty list for the same reason
    /// [`is_ready`](Self::is_ready) gives `false`.
    #[must_use]
    pub fn waits_for(&self, position: usize) -> &[IssueNumber] {
        self.waits.get(position).map_or(NO_NUMBERS, Vec::as_slice)
    }

    /// The issues that are finished with work before them that is not, in
    /// order. These are the ones somebody did out of order.
    #[must_use]
    pub fn finished_out_of_order(&self) -> Vec<IssueNumber> {
        self.out_of_order
            .iter()
            .map(|&position| self.entries[position].number)
            .collect()
    }

    /// The numbers the repository does not have, each one once, in the order
    /// of its first appearance.
    ///
    /// The number of a pair counts the same as the number of the step itself,
    /// and each one stands at its place: the step first, then the issue it
    /// closes. A pair whose issue the repository does not have is a typo the
    /// same way a step is, and one such number is what turns a green run red.
    ///
    /// A stream names one number as a step and as the issue a pair closes, so
    /// the number arrives twice and is reported once. A note that writes one
    /// number twice reads as a fault of the tool.
    #[must_use]
    pub fn missing(&self) -> Vec<IssueNumber> {
        let mut missing: Vec<IssueNumber> = Vec::new();
        for entry in &self.entries {
            let step = (entry.status == Status::Missing).then_some(entry.number);
            let closes = entry
                .closes
                .filter(|closes| closes.status == Status::Missing)
                .map(|closes| closes.number);
            for number in [step, closes].into_iter().flatten() {
                if !missing.contains(&number) {
                    missing.push(number);
                }
            }
        }
        missing
    }

    /// Every step whose pull request and whose issue say different things.
    ///
    /// A merged pull request over an open issue means the pull request closed
    /// nothing: the link is missing, or the number went to the wrong issue.
    /// Only GitHub knows this, and only this reports it.
    ///
    /// Both states must be known. A pair that holds a number the repository
    /// does not have gives no step here, because the report of that missing
    /// number already tells the reader to look.
    #[must_use]
    pub fn pairs_that_disagree(&self) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.status != Status::Missing
                    && entry.closes.is_some_and(|closes| {
                        closes.status != Status::Missing && closes.status != entry.status
                    })
            })
            .collect()
    }
}

/// One entry for each step, with what GitHub says about the step and about
/// the issue the step closes.
///
/// The state of a step is the state of the number the step names, because the
/// pull request of a pair is the work. The state of the issue travels beside
/// it, so the reader sees the two together.
///
/// A stream and a picture read a step the same way, so both of them read it
/// here. A second reader of a step is a second answer to one question.
fn entries_of(steps: &[Step], states: &States) -> Vec<Entry> {
    steps
        .iter()
        .map(|step| {
            let mut entry = states.entry(step.number());
            entry.closes = step.closes().map(|number| Closes {
                number,
                status: states.entry(number).status,
            });
            entry
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(number: u64) -> IssueNumber {
        IssueNumber::new(number).expect("the test number is an issue number")
    }

    fn entry(number: u64, status: Status) -> Entry {
        Entry {
            number: issue(number),
            title: format!("title of {number}"),
            status,
            closes: None,
        }
    }

    /// The step a plan writes as `#number` or as `#number (#closes)`.
    fn step(number: u64, closes: Option<u64>) -> Step {
        Step::new(issue(number), closes.map(issue))
    }

    fn numbers(list: &[IssueNumber]) -> Vec<u64> {
        list.iter().map(|n| n.get()).collect()
    }

    /// The numbers of the steps a report gives back.
    fn steps_of(entries: &[&Entry]) -> Vec<u64> {
        entries.iter().map(|entry| entry.number.get()).collect()
    }

    /// The paste of issue #418: two streams that join.
    ///
    /// A picture, and not a list of steps, because the test then says what a
    /// reader typed. `#242` and `#246` start their streams, `#249` waits for
    /// both of them, and the graph reads the steps in the order
    /// 242, 247, 246, 248, 249.
    const PASTE: &str = "\
#242 ──→ #247 ──┐
                ├──→ #249  (gallery)
#246 ──→ #248 ──┘";

    /// The graph the picture `text` draws.
    fn graph_of(text: &str) -> Graph {
        crate::graph::read(text)
            .expect("the text draws a graph")
            .expect("the picture reads")
    }

    /// The plan of issue #436, as a table of streams that names what each one
    /// waits for.
    ///
    /// A table, and not a list of steps, because the test then says what a
    /// reader typed. S0 starts alone, S1 waits for it, S2 waits for a step of
    /// S0 and a step of S1, and S3 stands apart. The graph reads the steps in
    /// the order 96, 91, 89, 94, 86.
    const PLAN: &str = "\
| Stream | Order | Waits for | Zone | Notes |
|--------|-------|-----------|------|-------|
| S0 — daemon leak | #96 | | crates/tsm (serve.rs) | Do first, solo. |
| S1 — lifecycle | #91 | #96 | crates/tsm (kill.rs) | |
| S2 — install | #89 → #94 | #96, #91 | crates/tsm (shell-init) | Same hotspot as S1. |
| S3 — keymap | #86 | | packages/web | Disjoint. |";

    /// The graph the plan `text` writes.
    ///
    /// The plan is parsed and then read as a graph, because that is the road
    /// the text takes: a plan that names a blocker is a graph, and the same
    /// report answers it.
    fn graph_of_plan(text: &str) -> Graph {
        crate::graph::of_plan(&crate::plan::parse(text).expect("the text is a plan"))
            .expect("the plan draws a graph")
            .expect("the plan reads")
    }

    /// What GitHub says about each number of `answers`.
    ///
    /// A number nobody names here is a number the repository does not have, so
    /// a test of a missing step names the numbers around it and stops.
    fn states_of(answers: &[(u64, Status)]) -> States {
        States::of(
            answers
                .iter()
                .map(|&(number, status)| entry(number, status))
                .collect(),
        )
    }

    /// The numbers of the entries somebody can start now, in the order of the
    /// rows.
    fn ready_of(report: &Report) -> Vec<u64> {
        report
            .entries()
            .iter()
            .enumerate()
            .filter(|(position, _)| report.is_ready(*position))
            .map(|(_, entry)| entry.number.get())
            .collect()
    }

    #[test]
    fn the_next_issue_is_the_first_open_one() {
        let report = Report::build(vec![
            entry(277, Status::Done),
            entry(278, Status::Open),
            entry(279, Status::Open),
        ]);
        assert_eq!(report.next(), Some(1));
        assert_eq!(
            report.next_entry().map(|e| e.number.get()),
            Some(278_u64),
            "the second issue is the first open one"
        );
    }

    #[test]
    fn a_chain_that_is_finished_names_no_issue_to_start() {
        let report = Report::build(vec![
            entry(277, Status::Done),
            entry(278, Status::Dropped),
            entry(279, Status::Done),
        ]);
        assert_eq!(report.next(), None);
        assert_eq!(report.next_entry(), None);
        assert!(report.finished_out_of_order().is_empty());
    }

    #[test]
    fn a_dropped_issue_is_walked_past() {
        // An issue closed as not planned is not work to start, and the chain
        // moves on to the one after it.
        let report = Report::build(vec![entry(277, Status::Dropped), entry(278, Status::Open)]);
        assert_eq!(report.next_entry().map(|e| e.number.get()), Some(278_u64));
    }

    #[test]
    fn a_missing_issue_is_never_the_one_to_start() {
        let report = Report::build(vec![entry(277, Status::Missing), entry(278, Status::Open)]);
        assert_eq!(report.next_entry().map(|e| e.number.get()), Some(278_u64));
        assert_eq!(numbers(&report.missing()), vec![277]);
    }

    #[test]
    fn work_done_after_the_next_issue_is_reported() {
        let report = Report::build(vec![
            entry(277, Status::Done),
            entry(278, Status::Open),
            entry(279, Status::Open),
            entry(280, Status::Done),
            entry(281, Status::Dropped),
        ]);
        assert_eq!(numbers(&report.finished_out_of_order()), vec![280, 281]);
    }

    #[test]
    fn work_done_before_the_next_issue_is_in_order() {
        let report = Report::build(vec![
            entry(277, Status::Done),
            entry(278, Status::Dropped),
            entry(279, Status::Open),
        ]);
        assert!(report.finished_out_of_order().is_empty());
    }

    #[test]
    fn a_step_writes_its_number_and_a_pair_writes_both() {
        assert_eq!(entry(330, Status::Open).label(), "#330");
        let paired = Entry {
            closes: Some(Closes {
                number: issue(341),
                status: Status::Open,
            }),
            ..entry(344, Status::Done)
        };
        assert_eq!(paired.label(), "#344 (#341)");
    }

    #[test]
    fn states_answer_for_a_number_they_hold() {
        let states = States::of(vec![entry(344, Status::Done)]);
        let answer = states.entry(issue(344));
        assert_eq!(answer.status, Status::Done);
        assert_eq!(answer.title, "title of 344");
        assert_eq!(answer.number, issue(344));
    }

    #[test]
    fn a_number_nobody_asked_about_is_missing_and_not_a_panic() {
        // The number comes out of text a reader pasted. A number the query
        // missed is one row of the output, and it must never stop the run.
        let states = States::of(vec![entry(344, Status::Done)]);
        let answer = states.entry(issue(341));
        assert_eq!(answer.status, Status::Missing);
        assert_eq!(answer.number, issue(341));
        assert!(answer.title.is_empty(), "a missing number carries no title");
    }

    #[test]
    fn a_stream_gives_one_entry_for_each_step() {
        let states = States::of(vec![
            entry(344, Status::Done),
            entry(341, Status::Open),
            entry(330, Status::Open),
        ]);
        let report = Report::of_steps(&[step(344, Some(341)), step(330, None)], &states);
        assert_eq!(report.entries().len(), 2);
        assert_eq!(report.entries()[0].number, issue(344));
        assert_eq!(report.entries()[0].title, "title of 344");
        assert_eq!(
            report.entries()[0].status,
            Status::Done,
            "the state of a step is the state of the pull request"
        );
        assert_eq!(
            report.entries()[0].closes,
            Some(Closes {
                number: issue(341),
                status: Status::Open,
            }),
            "the state of the issue stands beside the state of the work"
        );
        assert_eq!(report.entries()[1].number, issue(330));
        assert_eq!(report.entries()[1].status, Status::Open);
        assert_eq!(report.entries()[1].closes, None);
    }

    #[test]
    fn the_issue_to_start_is_the_first_open_pull_request() {
        // The first step is a merged pull request over an issue that is still
        // open. The work is done, so the answer is the step after it.
        let states = States::of(vec![
            entry(344, Status::Done),
            entry(341, Status::Open),
            entry(343, Status::Open),
            entry(329, Status::Done),
        ]);
        let report = Report::of_steps(&[step(344, Some(341)), step(343, Some(329))], &states);
        assert_eq!(report.next(), Some(1));
        assert_eq!(report.next_entry().map(|e| e.number.get()), Some(343_u64));
    }

    #[test]
    fn missing_holds_the_number_of_a_pair_the_repository_does_not_have() {
        // GitHub answered for 343, 329, 342 and 330 alone. So the pull request
        // 344 and the issue 341 of the first step are both missing, and the
        // issue 328 of the third step is missing on its own.
        let states = States::of(vec![
            entry(343, Status::Done),
            entry(329, Status::Open),
            entry(342, Status::Open),
            entry(330, Status::Open),
        ]);
        let report = Report::of_steps(
            &[
                step(344, Some(341)),
                step(343, Some(329)),
                step(342, Some(328)),
                step(330, None),
            ],
            &states,
        );
        assert_eq!(
            numbers(&report.missing()),
            vec![344, 341, 328],
            "each number stands where the plan wrote it, the step before the issue it closes"
        );
    }

    #[test]
    fn a_missing_number_is_named_once_however_often_a_plan_writes_it() {
        // GitHub answered for 344 and 330 alone. So 341 is missing, and the
        // stream names it twice: once as the issue the first step closes, and
        // once as a step of its own.
        let states = States::of(vec![entry(344, Status::Done), entry(330, Status::Open)]);
        let report = Report::of_steps(
            &[step(344, Some(341)), step(341, None), step(330, None)],
            &states,
        );
        assert_eq!(
            numbers(&report.missing()),
            vec![341],
            "one number the repository does not have is one number to name, at its first place"
        );
    }

    #[test]
    fn a_pair_whose_two_states_differ_is_reported() {
        // 344 is merged and 341 is open, so 344 closed nothing. 342 is closed
        // without the work being done and 328 is open, which differs as well.
        // 343 and 329 agree, and 330 is a step of one number.
        let states = States::of(vec![
            entry(344, Status::Done),
            entry(341, Status::Open),
            entry(343, Status::Done),
            entry(329, Status::Done),
            entry(342, Status::Dropped),
            entry(328, Status::Open),
            entry(330, Status::Open),
        ]);
        let report = Report::of_steps(
            &[
                step(344, Some(341)),
                step(343, Some(329)),
                step(342, Some(328)),
                step(330, None),
            ],
            &states,
        );
        assert_eq!(steps_of(&report.pairs_that_disagree()), vec![344, 342]);
    }

    #[test]
    fn a_pair_the_repository_does_not_have_is_not_a_disagreement() {
        // The report of the missing number already tells the reader to look.
        let states = States::of(vec![entry(344, Status::Done), entry(329, Status::Open)]);
        let report = Report::of_steps(&[step(344, Some(341)), step(343, Some(329))], &states);
        assert!(
            report.pairs_that_disagree().is_empty(),
            "one missing state is no disagreement, in either half of the pair"
        );
    }

    #[test]
    fn a_chain_holds_no_pair_that_disagrees() {
        let report = Report::build(vec![
            entry(277, Status::Done),
            entry(278, Status::Open),
            entry(279, Status::Missing),
        ]);
        assert!(report.pairs_that_disagree().is_empty());
    }

    /// Is `is_ready` true at the answer of the report and nowhere else?
    ///
    /// One line of work has one issue to start, so a reader of a chain and a
    /// reader of a stream must read the same mark on the same row they always
    /// read it on.
    fn ready_only_at_next(report: &Report) -> bool {
        (0..report.entries().len())
            .all(|position| report.is_ready(position) == (report.next() == Some(position)))
    }

    /// The position of the row of `number`.
    fn row_of(report: &Report, number: u64) -> usize {
        report
            .entries()
            .iter()
            .position(|entry| entry.number.get() == number)
            .expect("the report holds a row for the number")
    }

    /// The numbers the row of `number` waits for.
    fn waits_of(report: &Report, number: u64) -> Vec<u64> {
        numbers(report.waits_for(row_of(report, number)))
    }

    #[test]
    fn a_graph_names_every_step_somebody_can_start_now() {
        // Two streams that join are two people who work at the same time, and
        // an answer that names one issue loses that. Nothing of this picture
        // is finished, so both streams start.
        let states = states_of(&[
            (242, Status::Open),
            (246, Status::Open),
            (247, Status::Open),
            (248, Status::Open),
            (249, Status::Open),
        ]);
        let report = Report::of_graph(&graph_of(PASTE), &states);
        assert_eq!(
            ready_of(&report),
            vec![242, 246],
            "a step with no step before it is ready, and each stream has one"
        );
    }

    #[test]
    fn a_blocked_step_names_the_work_it_waits_for() {
        // The top stream is finished, so the bottom stream is the only one to
        // start. The join then waits for the one step of it that is not
        // finished, and the reader who reads the row of `#249` reads why
        // nobody starts it.
        let states = states_of(&[
            (242, Status::Done),
            (247, Status::Done),
            (246, Status::Open),
            (248, Status::Open),
            (249, Status::Open),
        ]);
        let report = Report::of_graph(&graph_of(PASTE), &states);
        assert_eq!(
            ready_of(&report),
            vec![246],
            "the stream that is finished starts nothing, and the other one starts"
        );
        assert_eq!(
            waits_of(&report, 249),
            vec![248],
            "a finished step before a row is no reason to wait"
        );
    }

    #[test]
    fn the_join_of_two_streams_is_ready_once_both_streams_are_finished() {
        // The two streams are done, so the step they join to is the one step
        // to start. A picture that names one answer names it here, and the
        // answer is the step no stream reaches alone.
        let states = states_of(&[
            (242, Status::Done),
            (247, Status::Done),
            (246, Status::Done),
            (248, Status::Done),
            (249, Status::Open),
        ]);
        let report = Report::of_graph(&graph_of(PASTE), &states);
        assert_eq!(ready_of(&report), vec![249]);
        assert_eq!(
            waits_of(&report, 249),
            Vec::<u64>::new(),
            "a step that is ready waits for nothing"
        );
    }

    #[test]
    fn a_step_is_ready_when_the_steps_it_waits_for_directly_are_finished() {
        // `#247` and `#248` are closed while `#242` and `#246` stay open, so
        // somebody worked ahead of the plan. A step is ready when the steps it
        // waits for directly are finished, and work somebody did ahead of the
        // plan does not take that away. So the join is ready beside the two
        // steps that start the streams, and the answer names all three.
        let states = states_of(&[
            (242, Status::Open),
            (247, Status::Done),
            (246, Status::Open),
            (248, Status::Done),
            (249, Status::Open),
        ]);
        let report = Report::of_graph(&graph_of(PASTE), &states);
        assert_eq!(ready_of(&report), vec![242, 246, 249]);
        assert_eq!(
            waits_of(&report, 249),
            Vec::<u64>::new(),
            "a step that is ready waits for nothing"
        );
    }

    #[test]
    fn a_blocked_step_names_every_step_it_waits_for() {
        // Both steps before `#249` are open, and a row that named the first of
        // them would send somebody to `#247` and hide `#248`. The numbers stand
        // in the order the picture holds them, so the reader reads the top
        // stream first.
        let states = states_of(&[
            (242, Status::Open),
            (247, Status::Open),
            (246, Status::Open),
            (248, Status::Open),
            (249, Status::Open),
        ]);
        let report = Report::of_graph(&graph_of(PASTE), &states);
        assert_eq!(waits_of(&report, 249), vec![247, 248]);
    }

    #[test]
    fn a_step_that_is_ready_waits_for_nothing() {
        // `#242` starts a stream, so no step stands before it at all.
        let states = states_of(&[
            (242, Status::Open),
            (247, Status::Open),
            (246, Status::Open),
            (248, Status::Open),
            (249, Status::Open),
        ]);
        let report = Report::of_graph(&graph_of(PASTE), &states);
        assert!(report.is_ready(row_of(&report, 242)));
        assert_eq!(waits_of(&report, 242), Vec::<u64>::new());
    }

    #[test]
    fn a_finished_step_waits_for_nothing_whatever_stands_before_it() {
        // `#249` is closed over two steps that are open. It is work nobody
        // looks at again, so it waits for nothing and the note about work
        // closed out of order is what the reader hears instead.
        let states = states_of(&[
            (242, Status::Open),
            (247, Status::Open),
            (246, Status::Open),
            (248, Status::Open),
            (249, Status::Done),
        ]);
        let report = Report::of_graph(&graph_of(PASTE), &states);
        assert_eq!(waits_of(&report, 249), Vec::<u64>::new());
        assert_eq!(numbers(&report.finished_out_of_order()), vec![249]);
    }

    #[test]
    fn a_step_behind_a_number_the_repository_does_not_have_waits_for_it() {
        // GitHub answered for every number but `#247`, so `#247` is missing. A
        // missing step is not finished, because nothing is known about it. So
        // `#249` waits for it, and the note about the missing number says why
        // nobody can start the work.
        let states = states_of(&[
            (242, Status::Done),
            (246, Status::Done),
            (248, Status::Done),
            (249, Status::Open),
        ]);
        let report = Report::of_graph(&graph_of(PASTE), &states);
        assert_eq!(waits_of(&report, 249), vec![247]);
        assert!(
            !report.is_ready(row_of(&report, 249)),
            "a step behind a number nobody can read is not a step to start"
        );
        assert_eq!(numbers(&report.missing()), vec![247]);
    }

    #[test]
    fn a_picture_that_is_finished_names_no_step_and_no_work_out_of_order() {
        let states = states_of(&[
            (242, Status::Done),
            (247, Status::Done),
            (246, Status::Done),
            (248, Status::Dropped),
            (249, Status::Done),
        ]);
        let report = Report::of_graph(&graph_of(PASTE), &states);
        assert_eq!(ready_of(&report), Vec::<u64>::new());
        assert_eq!(report.next(), None);
        assert!(
            report.finished_out_of_order().is_empty(),
            "a picture with no unfinished step holds no work closed out of order"
        );
    }

    #[test]
    fn a_step_of_a_picture_carries_the_state_of_the_issue_it_closes() {
        // One piece of work is sometimes two numbers, and a picture writes the
        // pair exactly as a table writes it. The pull request is the work and
        // the issue is what the work finishes, so a merged pull request over an
        // open issue is a link nobody wrote. A picture reads that the way a
        // stream reads it, because one module answers both.
        let states = states_of(&[
            (1, Status::Done),
            (2, Status::Done),
            (15, Status::Done),
            (4, Status::Open),
        ]);
        let report = Report::of_graph(
            &graph_of(
                "\
#1 ──┐
     ├──→ #4 (in flight, PR #15)
#2 ──┘",
            ),
            &states,
        );
        let work = &report.entries()[row_of(&report, 15)];
        assert_eq!(
            work.status,
            Status::Done,
            "the state of a step is the state of the pull request"
        );
        assert_eq!(
            work.closes,
            Some(Closes {
                number: issue(4),
                status: Status::Open,
            }),
            "the state of the issue stands beside the state of the work"
        );
        assert_eq!(steps_of(&report.pairs_that_disagree()), vec![15]);
    }

    #[test]
    fn a_finished_step_of_a_picture_with_unfinished_work_before_it_is_out_of_order() {
        // `#242` is open and every other step is done. So `#247` is closed
        // over the step before it, and `#249` is closed over a step two hops
        // back, because `#247` and `#248` are both done. A chain reports every
        // finished issue that stands after the first open one, so a picture
        // reports both of these and not the neighbor alone.
        let states = states_of(&[
            (242, Status::Open),
            (247, Status::Done),
            (246, Status::Done),
            (248, Status::Done),
            (249, Status::Done),
        ]);
        let report = Report::of_graph(&graph_of(PASTE), &states);
        assert_eq!(
            numbers(&report.finished_out_of_order()),
            vec![247, 249],
            "the work before a step is every step the wires reach, and not the one beside it"
        );
    }

    #[test]
    fn a_chain_and_a_stream_wait_for_nothing_and_are_ready_at_their_answer() {
        // One report answers a chain, a stream, and a graph. The list of what
        // a row waits for is what a picture needs, and a chain reads the line
        // above the row instead. So the list is empty everywhere in one line
        // of work, and the mark of a ready row stands where it always stood.
        let chain = Report::build(vec![
            entry(277, Status::Done),
            entry(278, Status::Open),
            entry(279, Status::Open),
        ]);
        assert!(ready_only_at_next(&chain));
        assert!(
            (0..chain.entries().len()).all(|position| chain.waits_for(position).is_empty()),
            "a chain names no step to wait for"
        );

        let states = States::of(vec![
            entry(344, Status::Done),
            entry(341, Status::Open),
            entry(330, Status::Open),
        ]);
        let stream = Report::of_steps(&[step(344, Some(341)), step(330, None)], &states);
        assert_eq!(stream.next(), Some(1));
        assert!(ready_only_at_next(&stream));
        assert!(
            (0..stream.entries().len()).all(|position| stream.waits_for(position).is_empty()),
            "a stream names no step to wait for"
        );

        let finished = Report::build(vec![entry(277, Status::Done)]);
        assert_eq!(finished.next(), None);
        assert!(
            ready_only_at_next(&finished),
            "a report with no issue to start marks no row ready"
        );
    }

    #[test]
    fn a_plan_that_names_a_blocker_names_every_step_somebody_can_start_now() {
        // The first step of a stream that waits for nothing, and no other. S0
        // and S3 wait for nothing, so both of them start. S1 and S2 wait for
        // `#96`, so neither of them does.
        let states = states_of(&[
            (96, Status::Open),
            (91, Status::Open),
            (89, Status::Open),
            (94, Status::Open),
            (86, Status::Open),
        ]);
        let report = Report::of_graph(&graph_of_plan(PLAN), &states);
        assert_eq!(ready_of(&report), vec![96, 86]);
    }

    #[test]
    fn a_stream_starts_once_the_work_of_its_cell_is_finished() {
        // `#96` is done, so S1 starts. S2 waits for `#96` and for `#91`, and
        // `#91` is still open, so S2 waits on. A cell of two blockers is a
        // stream that starts when both of them are finished.
        let states = states_of(&[
            (96, Status::Done),
            (91, Status::Open),
            (89, Status::Open),
            (94, Status::Open),
            (86, Status::Open),
        ]);
        let report = Report::of_graph(&graph_of_plan(PLAN), &states);
        assert_eq!(ready_of(&report), vec![91, 86]);
        assert_eq!(
            waits_of(&report, 89),
            vec![91],
            "a finished blocker is no reason to wait, and the other one still is"
        );
    }

    #[test]
    fn a_stream_behind_two_blockers_starts_once_both_of_them_are_finished() {
        let states = states_of(&[
            (96, Status::Done),
            (91, Status::Done),
            (89, Status::Open),
            (94, Status::Open),
            (86, Status::Open),
        ]);
        let report = Report::of_graph(&graph_of_plan(PLAN), &states);
        assert_eq!(ready_of(&report), vec![89, 86]);
        assert_eq!(
            waits_of(&report, 89),
            Vec::<u64>::new(),
            "a step that is ready waits for nothing"
        );
    }

    #[test]
    fn a_blocked_row_of_a_plan_names_every_step_it_waits_for() {
        // The cell of S2 names two blockers, and a row that named the first of
        // them would send somebody to `#96` and hide `#91`. The numbers stand
        // in the order the plan writes them.
        let states = states_of(&[
            (96, Status::Open),
            (91, Status::Open),
            (89, Status::Open),
            (94, Status::Open),
            (86, Status::Open),
        ]);
        let report = Report::of_graph(&graph_of_plan(PLAN), &states);
        assert_eq!(waits_of(&report, 89), vec![96, 91]);
    }

    #[test]
    fn a_blocker_the_repository_does_not_have_is_a_row_and_a_missing_number() {
        // GitHub answered for `#91` alone, so the blocker `#999` is a number
        // the repository does not have. It is one row of the answer and one
        // number of the note, and the stream behind it starts nothing: a step
        // behind a number nobody can read is not a step to start.
        let text = "\
| Stream | Order | Waits for |
| --- | --- | --- |
| S1 | #91 | #999 |";
        let states = states_of(&[(91, Status::Open)]);
        let report = Report::of_graph(&graph_of_plan(text), &states);
        assert_eq!(report.entries().len(), 2, "the rows still stand");
        assert_eq!(numbers(&report.missing()), vec![999]);
        assert_eq!(waits_of(&report, 91), vec![999]);
        assert!(!report.is_ready(row_of(&report, 91)));
    }

    #[test]
    fn a_number_in_two_streams_gives_one_state_to_both() {
        let states = States::of(vec![
            entry(344, Status::Done),
            entry(341, Status::Open),
            entry(330, Status::Open),
        ]);
        let first = Report::of_steps(&[step(344, Some(341)), step(330, None)], &states);
        let second = Report::of_steps(&[step(330, None)], &states);
        assert_eq!(first.entries()[1].status, Status::Open);
        assert_eq!(
            first.entries()[1],
            second.entries()[0],
            "one query answers every stream that names the number"
        );
    }
}
