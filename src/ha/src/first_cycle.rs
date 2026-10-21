//! The tally of the first refresh cycle. It decides when `ha` must stop
//! because the cycle read no target.
//!
//! A cycle that ends with no request left in the API budget gives no verdict.
//! Each failure of that cycle can be the spent budget. The tally then starts
//! again, and the next cycle decides.
//!
//! A failed target is a repository whose fetch failed or an owner whose
//! repository list failed. An owner list that GitHub cut at the limit is a
//! warning, not a failure.

use std::collections::BTreeMap;
use std::fmt;

use crate::github::SourceError;
use crate::status::{OwnerProblem, RateLimit};
use crate::target::{OwnerName, RepoName, Target};
use crate::worker::Update;

/// The first line of the text of [`NoTargetRead`].
const NO_TARGET_READ: &str = "the first refresh could not read any target";

/// The hint after the failures when one of them is a 404. GitHub gives the
/// same 404 for a repository that does not exist and for a private
/// repository that the token cannot read, thus "Not Found" alone does not
/// tell the person what to do.
const NOT_FOUND_HINT: &str = "GitHub answers 404 for a repository that does not exist, and for a \
                              private repository that this gh account cannot read.\n\
                              Run gh auth status to see the account.";

/// The HTTP status that GitHub gives for a name that it does not show.
const NOT_FOUND: u16 = 404;

/// What the end of a tallied cycle decides, when it does not stop `ha`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The cycle read a target, or no target failed. No later cycle is tallied.
    Read,
    /// No target was read, and the cycle ended with 0 requests remaining. Each
    /// failure can be the spent budget, thus the next cycle is tallied.
    BudgetSpent,
}

/// What the updates of a refresh cycle say about the targets. The tally
/// covers the first cycle, and each next cycle while the budget is spent.
///
/// This type holds the one definition of "the cycle read no target".
/// A good fetch that finds zero runs is a success. A failed target is a
/// repository whose fetch failed or an owner whose list failed. An owner
/// list that GitHub cut is a warning and does not count as a failure.
#[derive(Debug, Default)]
pub struct FirstCycle {
    /// Set when one repository fetch succeeded.
    read_a_repository: bool,
    /// The error of each repository whose fetch failed.
    failed_repositories: BTreeMap<RepoName, SourceError>,
    /// The error of each owner whose list failed. A cut list is not here.
    failed_owners: BTreeMap<OwnerName, SourceError>,
}

impl FirstCycle {
    /// Counts one update of the worker. An [`Update::Runs`] is a good fetch,
    /// also when it holds no run. An [`Update::Failed`] records the
    /// repository and its error. An [`Update::Repos`] replaces the failed
    /// owners with the owners whose list failed. It skips an owner whose
    /// list was cut, because a cut list is a warning, not a failure. Every
    /// other update changes nothing.
    pub fn note(&mut self, update: &Update) {
        match update {
            Update::Runs { .. } => self.read_a_repository = true,
            Update::Failed { repo, error } => {
                self.failed_repositories.insert(repo.clone(), error.clone());
            }
            Update::Repos { owner_errors, .. } => {
                self.failed_owners = owner_errors
                    .iter()
                    .filter_map(|(owner, problem)| match problem {
                        OwnerProblem::Failed(error) => Some((owner.clone(), error.clone())),
                        OwnerProblem::Cut(_) => None,
                    })
                    .collect();
            }
            Update::Jobs { .. } | Update::JobsFailed { .. } | Update::CycleDone { .. } => {}
        }
    }

    /// Gives the verdict at the end of the cycle. `rate` is the API budget
    /// that the end of the cycle states.
    ///
    /// When no repository fetch succeeded and a target failed, the budget
    /// decides. With 0 requests remaining, each failure can be the spent
    /// budget, and the verdict is [`Verdict::BudgetSpent`]. With a budget
    /// that is not spent, or with no rate, the cycle read no target.
    ///
    /// # Errors
    /// Returns [`NoTargetRead`] when no repository fetch succeeded, at least
    /// one target failed, and the budget is not spent. A target is a
    /// repository or an owner. A cut owner list is a warning and does not
    /// count.
    pub fn verdict(self, rate: Option<RateLimit>) -> Result<Verdict, NoTargetRead> {
        let nothing_failed = self.failed_repositories.is_empty() && self.failed_owners.is_empty();
        if self.read_a_repository || nothing_failed {
            return Ok(Verdict::Read);
        }
        if rate.is_some_and(|rate| rate.remaining == 0) {
            return Ok(Verdict::BudgetSpent);
        }
        let repositories = self
            .failed_repositories
            .into_iter()
            .map(|(repo, error)| (Target::Repo(repo), error));
        let owners = self
            .failed_owners
            .into_iter()
            .map(|(owner, error)| (Target::Owner(owner), error));
        Err(NoTargetRead {
            failures: repositories.chain(owners).collect(),
        })
    }
}

/// The first refresh cycle that GitHub answered read no target. The text lists each failed
/// target with its error. When one or more errors are a 404, a hint comes
/// after the list one time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoTargetRead {
    /// Each failed target with its error. The failed repositories
    /// come first, then the failed owners. Each group is sorted by name.
    failures: Vec<(Target, SourceError)>,
}

impl fmt::Display for NoTargetRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{NO_TARGET_READ}:")?;
        for (name, error) in &self.failures {
            write!(f, "\n  {name}: {error}")?;
        }
        let not_found = self.failures.iter().any(|(_, error)| {
            matches!(
                error,
                SourceError::Http {
                    status: NOT_FOUND,
                    ..
                }
            )
        });
        if not_found {
            write!(f, "\n{NOT_FOUND_HINT}")?;
        }
        Ok(())
    }
}

impl std::error::Error for NoTargetRead {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use super::*;

    fn repo(name: &str) -> RepoName {
        RepoName::parse(name).unwrap()
    }

    fn failed(name: &str) -> Update {
        Update::Failed {
            repo: repo(name),
            error: SourceError::TimedOut(Duration::from_secs(30)),
        }
    }

    fn rate(remaining: u64) -> Option<RateLimit> {
        let reset = chrono::DateTime::UNIX_EPOCH;
        Option::from(RateLimit {
            limit: 5000,
            remaining,
            reset,
        })
    }

    #[test]
    fn a_failure_with_a_spent_budget_is_a_verdict_of_budget_spent() {
        let mut cycle = FirstCycle::default();
        cycle.note(&failed("acme/a"));
        assert_eq!(cycle.verdict(rate(0)), Ok(Verdict::BudgetSpent));
    }

    #[test]
    fn a_failure_with_a_low_budget_that_is_not_spent_is_a_verdict_of_error() {
        let mut cycle = FirstCycle::default();
        cycle.note(&failed("acme/a"));
        assert!(cycle.verdict(rate(3)).is_err());
    }

    #[test]
    fn a_good_fetch_with_a_spent_budget_is_a_success() {
        let mut cycle = FirstCycle::default();
        cycle.note(&failed("acme/b"));
        cycle.note(&Update::Runs {
            repo: repo("acme/a"),
            runs: vec![],
        });
        assert_eq!(cycle.verdict(rate(0)), Ok(Verdict::Read));
    }

    #[test]
    fn no_failure_and_no_fetch_with_a_spent_budget_is_a_success() {
        assert_eq!(FirstCycle::default().verdict(rate(0)), Ok(Verdict::Read));
    }

    #[test]
    fn a_cycle_with_no_update_is_fine() {
        assert_eq!(FirstCycle::default().verdict(None), Ok(Verdict::Read));
    }

    #[test]
    fn a_failure_with_no_good_fetch_is_a_verdict_of_error() {
        let mut cycle = FirstCycle::default();
        cycle.note(&failed("acme/b"));
        cycle.note(&failed("acme/a"));
        let error = cycle.verdict(None).unwrap_err();
        assert_eq!(
            error.to_string(),
            "the first refresh could not read any target:\n  \
             acme/a: gh did not finish within 30s\n  \
             acme/b: gh did not finish within 30s"
        );
    }

    #[test]
    fn a_good_fetch_of_zero_runs_is_a_success() {
        let mut cycle = FirstCycle::default();
        cycle.note(&failed("acme/b"));
        cycle.note(&Update::Runs {
            repo: repo("acme/a"),
            runs: vec![],
        });
        assert_eq!(cycle.verdict(None), Ok(Verdict::Read));
    }

    #[test]
    fn a_failed_repository_comes_before_a_failed_owner() {
        let mut cycle = FirstCycle::default();
        cycle.note(&Update::Repos {
            repos: BTreeSet::new(),
            owner_errors: vec![
                (
                    OwnerName::parse("aaa").unwrap(),
                    OwnerProblem::Failed(SourceError::Failed("no such owner".to_owned())),
                ),
                (
                    OwnerName::parse("ccc").unwrap(),
                    OwnerProblem::Cut("only the first 1000 repositories".to_owned()),
                ),
            ],
        });
        cycle.note(&failed("zzz/r"));
        let error = cycle.verdict(None).unwrap_err();
        assert_eq!(
            error.to_string(),
            "the first refresh could not read any target:\n  \
             zzz/r: gh did not finish within 30s\n  \
             aaa: gh failed: no such owner"
        );
    }
}
