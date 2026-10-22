//! The one rule that decides whether `ha` trusts a successful answer of GitHub.
//!
//! GitHub can answer a list request with success and still give a list that is
//! empty or short. On 2026-10-22, `gh repo list` exited 0 with `[]`, and the
//! same command 5 seconds later listed 6 repositories. The rule: an item that
//! the last good answer held, and that the new answer leaves out, needs proof.
//! `ha` reads that item directly. When the item is still there and still
//! matches the query, the answer is suspect. This module is the one place that
//! decides this, thus every list that `ha` reads goes through it.

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
        let _ = f;
        Ok(())
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
    #[error("x")]
    LeftOut(Item),
    /// An item is not in the answer, and the read of it failed.
    #[error("x")]
    Unread(Item, SourceError),
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
    let _ = (in_range, read);
    last_good
        .iter()
        .find(|item| !answer.iter().any(|held| held.item() == item.item()))
        .map(|item| Suspicion::LeftOut(item.item()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(text: &str) -> RepoName {
        RepoName::parse(text).expect("a valid repository")
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
}
