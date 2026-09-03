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
#[allow(
    dead_code,
    reason = "the render of a graph reads waits_for in the slice that draws it"
)]
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
    #[must_use]
    #[allow(
        dead_code,
        reason = "the run of a picture calls this in the slice that answers a graph"
    )]
    pub fn of_graph(graph: &Graph, states: &States) -> Self {
        Self::of_steps(graph.steps(), states)
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
    #[allow(
        dead_code,
        reason = "the render of a graph writes this column in the slice that draws it"
    )]
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
