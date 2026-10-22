//! The merge queue of one repository: the pull requests in it now, and the
//! pull requests that left it.
//!
//! GitHub gives the live queue only through GraphQL, and the history of the
//! queue only as issue events. This module holds the types that the source,
//! the worker, and the board share. It also holds the one rule that turns the
//! issue events of a repository into the departures from its queue
//! ([`read_events`]).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::target::RepoName;

/// The number of one pull request in its repository.
///
/// The page of the pull request is `/pull/<number>`. The board shows the
/// number as `#<number>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PullNumber(pub u64);

/// What a pull request in the merge queue waits for, as GitHub states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryState {
    /// The entry waits for its turn. GitHub did not start its checks yet
    /// (`QUEUED`).
    Queued,
    /// The checks of the merge group of the entry run (`AWAITING_CHECKS`).
    AwaitingChecks,
    /// The checks passed, and the entry merges when its turn comes
    /// (`MERGEABLE`).
    Mergeable,
    /// GitHub merges the entry now (`LOCKED`).
    Locked,
    /// The entry cannot merge, and GitHub removes it soon (`UNMERGEABLE`).
    Unmergeable,
    /// A value that GitHub does not document.
    Other,
}

impl EntryState {
    /// Maps the `state` of a GraphQL `MergeQueueEntry` to a state.
    ///
    /// A value that GitHub does not document gives [`EntryState::Other`].
    /// This function never fails.
    #[must_use]
    pub fn from_github(state: &str) -> EntryState {
        let _ = state;
        EntryState::Other
    }
}

/// One pull request in the merge queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry {
    /// The repository that holds the queue.
    pub repo: RepoName,
    /// The number of the pull request.
    pub pull: PullNumber,
    /// The title of the pull request.
    pub title: String,
    /// The head branch of the pull request.
    pub branch: String,
    /// The place of the entry in the queue. The entry at position 1 merges
    /// first.
    pub position: u32,
    /// What the entry waits for.
    pub state: EntryState,
    /// When the pull request joined the queue.
    pub enqueued_at: DateTime<Utc>,
    /// The address of the page of the pull request.
    pub url: String,
}

/// How a pull request left the merge queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The queue merged the pull request.
    Merged,
    /// The queue removed the pull request, for example because its checks
    /// failed, because of a merge conflict, or because the checks did not
    /// finish in time.
    Failed,
    /// A person removed the pull request from the queue.
    Removed,
}

/// One time that a pull request left the merge queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Departure {
    /// The repository that holds the queue.
    pub repo: RepoName,
    /// The number of the pull request.
    pub pull: PullNumber,
    /// The title of the pull request.
    pub title: String,
    /// The head branch of the pull request. The text is empty while it is
    /// not known. [`read_events`] leaves it empty, because an issue event
    /// does not name the branch.
    pub branch: String,
    /// How the pull request left the queue.
    pub outcome: Outcome,
    /// When the pull request joined the queue. The value is `None` when the
    /// event that added it is older than the events that `ha` read.
    pub enqueued_at: Option<DateTime<Utc>>,
    /// When the pull request left the queue.
    pub left_at: DateTime<Utc>,
    /// The address of the page of the pull request.
    pub url: String,
}

/// What one issue event says about the merge queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// Somebody added the pull request to the queue
    /// (`added_to_merge_queue`).
    Added,
    /// The pull request left the queue (`removed_from_merge_queue`).
    Removed {
        /// True when the queue itself removed it ([`QUEUE_BOT`]).
        by_queue: bool,
    },
    /// The pull request merged (`merged`). The queue merges a pull request,
    /// and a person with the right to bypass the queue can also merge one.
    Merged,
}

impl EventKind {
    /// The order of two kinds at the same second: an addition, then a
    /// removal, then a merge.
    fn rank(self) -> u8 {
        match self {
            EventKind::Added => 0,
            EventKind::Removed { .. } => 1,
            EventKind::Merged => 2,
        }
    }
}

/// One issue event of a pull request that is about the merge queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEvent {
    /// The number of the pull request.
    pub pull: PullNumber,
    /// The title of the pull request. GitHub gives the title that the pull
    /// request has now, not the title at the time of the event.
    pub title: String,
    /// The address of the page of the pull request.
    pub url: String,
    /// When the event occurred.
    pub at: DateTime<Utc>,
    /// What the event says.
    pub kind: EventKind,
}

/// The login of the GitHub app that runs the merge queue. It writes the
/// removal of each pull request that leaves the queue without a person.
pub const QUEUE_BOT: &str = "github-merge-queue[bot]";

/// The longest time between a removal by the queue and the `merged` event of
/// one merge.
///
/// When the queue merges a pull request, [`QUEUE_BOT`] removes it from the
/// queue, and GitHub writes a `merged` event. On 2026-10-22, the two events of
/// each of the 7 merges in `rust-lang/rust-clippy` and `bevyengine/bevy` were
/// 0 or 1 second apart. A merge that comes later is a merge by hand.
pub const MERGE_GAP: Duration = Duration::from_secs(60);

/// What the issue events of one repository say about its merge queue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueLog {
    /// The pull requests that the events show in the queue now: the last
    /// queue event of each one is an addition.
    pub open: BTreeSet<PullNumber>,
    /// Each time that a pull request left the queue, the oldest first.
    pub departures: Vec<Departure>,
}

/// Turns the merge queue events of `repo` into the queue log.
///
/// The events can come in any order. The function reads them oldest first,
/// and at the same second it reads an addition, then a removal, then a merge.
/// It reads each pull request as a series of stays in the queue:
///
/// - An addition starts a stay. A second addition while a stay is open
///   changes nothing.
/// - A removal ends the open stay as [`Outcome::Failed`] when the queue
///   removed it, and as [`Outcome::Removed`] when a person removed it. A
///   removal with no open stay is a stay whose addition is older than the
///   events, and it has no enqueue time.
/// - A merge ends the open stay as [`Outcome::Merged`]. A merge up to
///   [`MERGE_GAP`] after a removal by the queue makes that stay a merge. A
///   removal by the queue up to [`MERGE_GAP`] after a merge is part of that
///   merge. Any other merge did not come through the queue, and the log
///   ignores it.
///
/// Each departure takes the title and the address of the events of its pull
/// request, and an empty branch.
#[must_use]
pub fn read_events(repo: &RepoName, events: Vec<QueueEvent>) -> QueueLog {
    let _ = (repo, events);
    QueueLog::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoName {
        RepoName::parse("rust-lang/rust-clippy").expect("a valid repository")
    }

    /// A time `seconds` after 2026-10-22T12:00:00Z.
    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-22T12:00:00Z")
            .expect("a valid time")
            .with_timezone(&Utc)
            + chrono::TimeDelta::seconds(seconds)
    }

    fn event(pull: u64, seconds: i64, kind: EventKind) -> QueueEvent {
        QueueEvent {
            pull: PullNumber(pull),
            title: format!("pull {pull}"),
            url: format!("https://github.com/rust-lang/rust-clippy/pull/{pull}"),
            at: at(seconds),
            kind,
        }
    }

    fn added(pull: u64, seconds: i64) -> QueueEvent {
        event(pull, seconds, EventKind::Added)
    }

    fn removed_by_queue(pull: u64, seconds: i64) -> QueueEvent {
        event(pull, seconds, EventKind::Removed { by_queue: true })
    }

    fn removed_by_person(pull: u64, seconds: i64) -> QueueEvent {
        event(pull, seconds, EventKind::Removed { by_queue: false })
    }

    fn merged(pull: u64, seconds: i64) -> QueueEvent {
        event(pull, seconds, EventKind::Merged)
    }

    /// Each departure as `(pull, outcome, enqueued second, left second)`.
    fn summary(log: &QueueLog) -> Vec<(u64, Outcome, Option<i64>, i64)> {
        log.departures
            .iter()
            .map(|departure| {
                (
                    departure.pull.0,
                    departure.outcome,
                    departure
                        .enqueued_at
                        .map(|time| (time - at(0)).num_seconds()),
                    (departure.left_at - at(0)).num_seconds(),
                )
            })
            .collect()
    }

    #[test]
    fn each_documented_state_maps_to_its_own_state() {
        assert_eq!(EntryState::from_github("QUEUED"), EntryState::Queued);
        assert_eq!(
            EntryState::from_github("AWAITING_CHECKS"),
            EntryState::AwaitingChecks
        );
        assert_eq!(EntryState::from_github("MERGEABLE"), EntryState::Mergeable);
        assert_eq!(EntryState::from_github("LOCKED"), EntryState::Locked);
        assert_eq!(
            EntryState::from_github("UNMERGEABLE"),
            EntryState::Unmergeable
        );
    }

    #[test]
    fn an_unknown_state_maps_to_other() {
        assert_eq!(EntryState::from_github("SOMETHING_NEW"), EntryState::Other);
        assert_eq!(EntryState::from_github(""), EntryState::Other);
    }

    #[test]
    fn a_merge_by_the_queue_is_one_merged_departure() {
        // The shape of PR #17828 of rust-lang/rust-clippy: the removal by the
        // queue and the merge have the same second.
        let log = read_events(
            &repo(),
            vec![added(1, 0), removed_by_queue(1, 600), merged(1, 600)],
        );
        assert_eq!(summary(&log), [(1, Outcome::Merged, Some(0), 600)]);
        assert!(log.open.is_empty());
    }

    #[test]
    fn a_merge_a_second_after_the_removal_is_the_same_merge() {
        // The shape of PR #17898 of rust-lang/rust-clippy.
        let log = read_events(
            &repo(),
            vec![added(1, 0), removed_by_queue(1, 378), merged(1, 379)],
        );
        assert_eq!(summary(&log), [(1, Outcome::Merged, Some(0), 379)]);
    }

    #[test]
    fn a_removal_by_the_queue_after_the_merge_is_part_of_the_merge() {
        let log = read_events(
            &repo(),
            vec![added(1, 0), merged(1, 300), removed_by_queue(1, 301)],
        );
        assert_eq!(summary(&log), [(1, Outcome::Merged, Some(0), 300)]);
    }

    #[test]
    fn a_removal_by_the_queue_with_no_merge_is_a_failure() {
        let log = read_events(&repo(), vec![added(1, 0), removed_by_queue(1, 396)]);
        assert_eq!(summary(&log), [(1, Outcome::Failed, Some(0), 396)]);
        assert!(log.open.is_empty());
    }

    #[test]
    fn a_removal_by_a_person_is_a_removal() {
        let log = read_events(&repo(), vec![added(1, 0), removed_by_person(1, 90)]);
        assert_eq!(summary(&log), [(1, Outcome::Removed, Some(0), 90)]);
    }

    #[test]
    fn a_merge_right_after_a_removal_by_a_person_did_not_come_through_the_queue() {
        let log = read_events(
            &repo(),
            vec![added(1, 0), removed_by_person(1, 90), merged(1, 91)],
        );
        assert_eq!(summary(&log), [(1, Outcome::Removed, Some(0), 90)]);
    }

    #[test]
    fn a_merge_long_after_a_failure_did_not_come_through_the_queue() {
        let gap = i64::try_from(MERGE_GAP.as_secs()).expect("a small gap");
        let log = read_events(
            &repo(),
            vec![
                added(1, 0),
                removed_by_queue(1, 100),
                merged(1, 100 + gap + 1),
            ],
        );
        assert_eq!(summary(&log), [(1, Outcome::Failed, Some(0), 100)]);
    }

    #[test]
    fn a_merge_at_the_end_of_the_gap_is_still_the_merge_of_the_queue() {
        let gap = i64::try_from(MERGE_GAP.as_secs()).expect("a small gap");
        let log = read_events(
            &repo(),
            vec![added(1, 0), removed_by_queue(1, 100), merged(1, 100 + gap)],
        );
        assert_eq!(summary(&log), [(1, Outcome::Merged, Some(0), 100 + gap)]);
    }

    #[test]
    fn a_merge_with_no_stay_in_the_queue_is_not_queue_history() {
        let log = read_events(&repo(), vec![merged(1, 0)]);
        assert_eq!(log, QueueLog::default());
    }

    #[test]
    fn an_addition_with_no_end_is_open() {
        let log = read_events(&repo(), vec![added(7, 0), added(8, 5), merged(8, 0)]);
        assert!(log.departures.is_empty());
        assert_eq!(log.open, BTreeSet::from([PullNumber(7), PullNumber(8)]));
    }

    #[test]
    fn a_removal_with_no_addition_has_no_enqueue_time() {
        let log = read_events(&repo(), vec![removed_by_queue(1, 50), merged(1, 50)]);
        assert_eq!(summary(&log), [(1, Outcome::Merged, None, 50)]);
    }

    #[test]
    fn a_pull_request_that_failed_and_then_merged_has_two_departures() {
        // The shape of PR #17051 of rust-lang/rust-clippy, with a merge at the end.
        let log = read_events(
            &repo(),
            vec![
                added(1, 0),
                removed_by_queue(1, 396),
                added(1, 1000),
                removed_by_queue(1, 1400),
                merged(1, 1400),
            ],
        );
        assert_eq!(
            summary(&log),
            [
                (1, Outcome::Failed, Some(0), 396),
                (1, Outcome::Merged, Some(1000), 1400),
            ]
        );
    }

    #[test]
    fn the_order_of_the_events_does_not_matter() {
        let mut events = vec![
            added(1, 0),
            removed_by_queue(1, 100),
            merged(1, 100),
            added(2, 10),
            removed_by_person(2, 20),
            added(3, 30),
        ];
        let oldest_first = read_events(&repo(), events.clone());
        events.reverse();
        assert_eq!(read_events(&repo(), events), oldest_first);
        assert_eq!(
            summary(&oldest_first),
            [
                (2, Outcome::Removed, Some(10), 20),
                (1, Outcome::Merged, Some(0), 100),
            ]
        );
        assert_eq!(oldest_first.open, BTreeSet::from([PullNumber(3)]));
    }

    #[test]
    fn a_second_addition_while_open_keeps_the_first_start() {
        let log = read_events(
            &repo(),
            vec![added(1, 0), added(1, 40), removed_by_person(1, 90)],
        );
        assert_eq!(summary(&log), [(1, Outcome::Removed, Some(0), 90)]);
    }

    #[test]
    fn a_departure_takes_the_repository_title_and_address_and_no_branch() {
        let log = read_events(&repo(), vec![added(5, 0), removed_by_queue(5, 9)]);
        let departure = &log.departures[0];
        assert_eq!(departure.repo, repo());
        assert_eq!(departure.title, "pull 5");
        assert_eq!(
            departure.url,
            "https://github.com/rust-lang/rust-clippy/pull/5"
        );
        assert_eq!(departure.branch, "");
    }
}
