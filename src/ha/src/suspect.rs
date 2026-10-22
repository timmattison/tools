//! The one rule that decides whether `ha` trusts a successful answer of GitHub.
//!
//! GitHub can answer a list request with success and still give a list that is
//! empty or short. On 2026-10-22, `gh repo list` exited 0 with `[]`, and the
//! same command 5 seconds later listed 6 repositories. The rule: an item that
//! the last good answer held, and that the new answer leaves out, needs proof.
//! `ha` reads that item directly. When the item is still there and still
//! matches the query, the answer is suspect. This module is the one place that
//! decides this, thus every list that `ha` reads goes through it.
//!
//! A second rule covers an empty answer. An empty answer has no item to
//! compare, and the first answer after a start has no last good answer. Then
//! `ha` reads the newest run of the repository directly. When that run matches
//! the query that gave the empty answer, GitHub left it out, and the answer is
//! suspect. A failed read of the newest run makes the answer suspect too.
//! [`newest_left_out`] holds this rule.

use std::collections::HashSet;
use std::fmt;

use crate::github::SourceError;
use crate::run::{Run, RunId};
use crate::target::RepoName;

/// An item of a list answer: a run or a repository.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Item {
    /// A workflow run.
    Run(RunId),
    /// A repository.
    Repo(RepoName),
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Item::Run(id) => write!(f, "run {}", id.0),
            Item::Repo(repo) => repo.fmt(f),
        }
    }
}

/// A value that a list answer of GitHub holds.
pub trait Listed {
    /// The identity of this value in a list.
    fn item(&self) -> Item;
}

impl Listed for Run {
    fn item(&self) -> Item {
        Item::Run(self.id)
    }
}

impl Listed for RepoName {
    fn item(&self) -> Item {
        Item::Repo(self.clone())
    }
}

/// What a direct read says about one item that an answer left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    /// The item is still there, and it still matches the query.
    InRange,
    /// The item is gone, or it does not match the query now.
    Gone,
    /// The read failed, thus ha cannot tell.
    Unread(SourceError),
}

/// Why the worker does not trust an answer that GitHub gave with success.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Suspicion {
    /// An item that is still in range is not in the answer.
    #[error("GitHub left out {0}")]
    LeftOut(Item),
    /// An item is not in the answer, and the read of it failed.
    #[error("GitHub left out {0}, and ha cannot read it: {1}")]
    Unread(Item, SourceError),
    /// An answer is empty, and the newest run of the repository matches its query.
    #[error("GitHub listed no runs, but run {} matches", .0.0)]
    NewestLeftOut(RunId),
    /// An answer is empty, and the read of the newest run failed.
    #[error("GitHub listed no runs, and ha cannot read the newest run: {0}")]
    NewestUnread(SourceError),
}

/// The one rule. Walks `last_good` in order. Skips each item that `answer` holds
/// (same `Listed::item`), and each item that `in_range` refuses. Calls `read` for the
/// other items, one at a time: `InRange` gives `Suspicion::LeftOut`, `Unread(e)` gives
/// `Suspicion::Unread(item, e)`, `Gone` goes on to the next item. Returns the first
/// suspicion, and calls `read` no more after it. `None` means the answer is good.
pub fn first_left_out<T: Listed>(
    last_good: &[T],
    answer: &[T],
    in_range: impl Fn(&T) -> bool,
    read: impl FnMut(&T) -> Presence,
) -> Option<Suspicion> {
    let mut read = read;
    let held: HashSet<Item> = answer.iter().map(Listed::item).collect();
    for item in last_good {
        let identity = item.item();
        if held.contains(&identity) || !in_range(item) {
            continue;
        }
        match read(item) {
            Presence::InRange => return Some(Suspicion::LeftOut(identity)),
            Presence::Unread(error) => return Some(Suspicion::Unread(identity, error)),
            Presence::Gone => {}
        }
    }
    None
}

/// The probe rule for an empty answer. `newest` is the direct read of the
/// newest run of the repository. A run that `matches` the query of the empty
/// answer gives [`Suspicion::NewestLeftOut`]. A run that does not match, and no
/// run at all, give `None`. A failed read gives [`Suspicion::NewestUnread`].
pub fn newest_left_out(
    newest: Result<Option<Run>, SourceError>,
    matches: impl Fn(&Run) -> bool,
) -> Option<Suspicion> {
    match newest {
        Ok(Some(run)) if matches(&run) => Some(Suspicion::NewestLeftOut(run.id)),
        Ok(_) => None,
        Err(error) => Some(Suspicion::NewestUnread(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(text: &str) -> RepoName {
        RepoName::parse(text).expect("a valid repository")
    }

    fn run(id: u64) -> Run {
        let time = chrono::DateTime::UNIX_EPOCH;
        Run {
            id: RunId(id),
            repo: repo("acme/a"),
            workflow: "CI".to_owned(),
            branch: "main".to_owned(),
            title: "t".to_owned(),
            state: crate::run::RunState::Passed,
            started_at: time,
            created_at: time,
            updated_at: time,
            url: String::new(),
        }
    }

    #[test]
    fn an_item_that_the_answer_left_out_and_read_finds_in_range_is_a_suspicion() {
        let last_good = [repo("acme/a"), repo("acme/b")];
        let answer = [repo("acme/a")];
        let suspicion = first_left_out(&last_good, &answer, |_| true, |_| Presence::InRange);
        assert_eq!(
            suspicion,
            Some(Suspicion::LeftOut(Item::Repo(repo("acme/b"))))
        );
    }

    #[test]
    fn an_item_that_read_finds_gone_is_not_a_suspicion() {
        let last_good = [repo("acme/a"), repo("acme/b")];
        let answer = [repo("acme/a")];
        let suspicion = first_left_out(&last_good, &answer, |_| true, |_| Presence::Gone);
        assert_eq!(suspicion, None);
    }

    #[test]
    fn a_gone_item_does_not_hide_a_later_item_that_is_in_range() {
        let last_good = [repo("acme/a"), repo("acme/b"), repo("acme/c")];
        let answer = [repo("acme/a")];
        let gone = repo("acme/b");
        let suspicion = first_left_out(
            &last_good,
            &answer,
            |_| true,
            |item| {
                if *item == gone {
                    Presence::Gone
                } else {
                    Presence::InRange
                }
            },
        );
        assert_eq!(
            suspicion,
            Some(Suspicion::LeftOut(Item::Repo(repo("acme/c"))))
        );
    }

    #[test]
    fn an_item_that_in_range_refuses_is_never_read() {
        let last_good = [repo("acme/a"), repo("acme/b")];
        let answer = [];
        let mut reads = 0;
        let suspicion = first_left_out(
            &last_good,
            &answer,
            |_| false,
            |_| {
                reads += 1;
                Presence::InRange
            },
        );
        assert_eq!(suspicion, None);
        assert_eq!(reads, 0);
    }

    #[test]
    fn runs_are_compared_by_their_id() {
        let last_good = [run(1), run(2)];
        let answer = [run(2)];
        let suspicion = first_left_out(&last_good, &answer, |_| true, |_| Presence::InRange);
        assert_eq!(suspicion, Some(Suspicion::LeftOut(Item::Run(RunId(1)))));
    }

    #[test]
    fn a_failed_read_is_a_suspicion_with_its_error() {
        let last_good = [repo("acme/a"), repo("acme/b")];
        let answer = [repo("acme/a")];
        let error = SourceError::TimedOut(std::time::Duration::from_secs(30));
        let suspicion = first_left_out(
            &last_good,
            &answer,
            |_| true,
            |_| Presence::Unread(error.clone()),
        );
        assert_eq!(
            suspicion,
            Some(Suspicion::Unread(Item::Repo(repo("acme/b")), error))
        );
    }

    #[test]
    fn read_is_not_called_after_the_first_suspicion_or_for_a_held_item() {
        let last_good = [
            repo("acme/a"),
            repo("acme/b"),
            repo("acme/c"),
            repo("acme/d"),
        ];
        let answer = [repo("acme/a")];
        let mut read_items = Vec::new();
        let suspicion = first_left_out(
            &last_good,
            &answer,
            |_| true,
            |item| {
                read_items.push(item.clone());
                if *item == repo("acme/c") {
                    Presence::Unread(SourceError::Failed("x".to_owned()))
                } else {
                    Presence::Gone
                }
            },
        );
        assert!(matches!(suspicion, Some(Suspicion::Unread(..))));
        assert_eq!(read_items, [repo("acme/b"), repo("acme/c")]);
    }

    #[test]
    fn items_and_suspicions_have_their_texts() {
        let widgets = Item::Repo(repo("acme/widgets"));
        assert_eq!(Item::Run(RunId(5)).to_string(), "run 5");
        assert_eq!(widgets.to_string(), "acme/widgets");
        assert_eq!(
            Suspicion::LeftOut(Item::Run(RunId(5))).to_string(),
            "GitHub left out run 5"
        );
        let error = SourceError::Failed("boom".to_owned());
        assert_eq!(
            Suspicion::Unread(widgets, error).to_string(),
            "GitHub left out acme/widgets, and ha cannot read it: gh failed: boom"
        );
    }

    fn run_created_at(id: u64, seconds: i64) -> Run {
        let at = chrono::DateTime::UNIX_EPOCH + chrono::TimeDelta::seconds(seconds);
        Run {
            created_at: at,
            ..run(id)
        }
    }

    fn created_since_100(run: &Run) -> bool {
        run.created_at >= chrono::DateTime::UNIX_EPOCH + chrono::TimeDelta::seconds(100)
    }

    #[test]
    fn a_newest_run_that_matches_is_a_suspicion() {
        let suspicion = newest_left_out(Ok(Some(run_created_at(7, 200))), created_since_100);
        assert_eq!(suspicion, Some(Suspicion::NewestLeftOut(RunId(7))));
    }

    #[test]
    fn a_newest_run_that_does_not_match_is_not_a_suspicion() {
        let suspicion = newest_left_out(Ok(Some(run_created_at(7, 50))), created_since_100);
        assert_eq!(suspicion, None);
    }

    #[test]
    fn no_newest_run_is_not_a_suspicion() {
        assert_eq!(newest_left_out(Ok(None), created_since_100), None);
    }

    #[test]
    fn a_failed_read_of_the_newest_run_is_a_suspicion_with_its_error() {
        let error = SourceError::Failed("boom".to_owned());
        let suspicion = newest_left_out(Err(error.clone()), created_since_100);
        assert_eq!(suspicion, Some(Suspicion::NewestUnread(error)));
    }

    #[test]
    fn the_probe_suspicions_have_their_texts() {
        assert_eq!(
            Suspicion::NewestLeftOut(RunId(7)).to_string(),
            "GitHub listed no runs, but run 7 matches"
        );
        assert_eq!(
            Suspicion::NewestUnread(SourceError::Failed("boom".to_owned())).to_string(),
            "GitHub listed no runs, and ha cannot read the newest run: gh failed: boom"
        );
    }
}
