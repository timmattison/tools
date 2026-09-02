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

use crate::chain::IssueNumber;

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
}

/// The chain, in order, and the answer it gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    entries: Vec<Entry>,
    next: Option<usize>,
}

impl Report {
    /// Read the answer out of the states of the chain.
    #[must_use]
    pub fn build(entries: Vec<Entry>) -> Self {
        let next = entries.iter().position(|entry| entry.status.is_open());
        Self { entries, next }
    }

    /// The chain, in the order it was written.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The position in [`entries`](Self::entries) of the issue to start, or
    /// `None` when no issue of the chain is open.
    #[must_use]
    pub fn next(&self) -> Option<usize> {
        self.next
    }

    /// The issue to start, or `None` when no issue of the chain is open.
    #[must_use]
    pub fn next_entry(&self) -> Option<&Entry> {
        self.next.map(|i| &self.entries[i])
    }

    /// The issues that are finished and stand after the one to start, in
    /// order. These are the ones somebody did out of order.
    #[must_use]
    pub fn finished_out_of_order(&self) -> Vec<IssueNumber> {
        let Some(next) = self.next else {
            return Vec::new();
        };
        self.entries
            .iter()
            .skip(next + 1)
            .filter(|entry| entry.status.is_finished())
            .map(|entry| entry.number)
            .collect()
    }

    /// The issues of the chain the repository does not have, in order.
    #[must_use]
    pub fn missing(&self) -> Vec<IssueNumber> {
        self.entries
            .iter()
            .filter(|entry| entry.status == Status::Missing)
            .map(|entry| entry.number)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(number: u64, status: Status) -> Entry {
        Entry {
            number: IssueNumber::new(number).expect("the test number is an issue number"),
            title: format!("title of {number}"),
            status,
        }
    }

    fn numbers(list: &[IssueNumber]) -> Vec<u64> {
        list.iter().map(|n| n.get()).collect()
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
}
