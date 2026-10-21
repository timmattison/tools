//! The tally of the first refresh cycle. It decides when `ha` must stop
//! because the cycle read no target.

use std::collections::BTreeMap;
use std::fmt;

use crate::github::SourceError;
use crate::target::RepoName;
use crate::worker::Update;

/// The first line of the text of [`NoTargetRead`].
const NO_TARGET_READ: &str = "the first refresh could not read any target";

/// What the updates of the first refresh cycle say about the targets.
///
/// This type holds the one definition of "the first cycle read no target".
/// A good fetch that finds zero runs is a success.
#[derive(Debug, Default)]
pub struct FirstCycle {
    /// Set when one repository fetch succeeded.
    read_a_repository: bool,
    /// The error of each repository whose fetch failed.
    failed_repositories: BTreeMap<RepoName, SourceError>,
}

impl FirstCycle {
    /// Counts one update of the worker. An [`Update::Runs`] is a good fetch,
    /// also when it holds no run. An [`Update::Failed`] records the
    /// repository and its error. Every other update changes nothing.
    pub fn note(&mut self, update: &Update) {
        match update {
            Update::Runs { .. } => self.read_a_repository = true,
            Update::Failed { repo, error } => {
                self.failed_repositories.insert(repo.clone(), error.clone());
            }
            _ => {}
        }
    }

    /// Gives the verdict at the end of the cycle.
    ///
    /// # Errors
    /// Returns [`NoTargetRead`] when no repository fetch succeeded and at
    /// least one target failed.
    pub fn verdict(self) -> Result<(), NoTargetRead> {
        if self.read_a_repository || self.failed_repositories.is_empty() {
            return Ok(());
        }
        Err(NoTargetRead {
            failures: self
                .failed_repositories
                .into_iter()
                .map(|(repo, error)| (repo.to_string(), error))
                .collect(),
        })
    }
}

/// The first refresh cycle read no target. The text lists each failed
/// target with its error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoTargetRead {
    /// The name and the error of each failed target, sorted by name.
    failures: Vec<(String, SourceError)>,
}

impl fmt::Display for NoTargetRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{NO_TARGET_READ}:")?;
        for (name, error) in &self.failures {
            write!(f, "\n  {name}: {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for NoTargetRead {}

#[cfg(test)]
mod tests {
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

    #[test]
    fn a_cycle_with_no_update_is_fine() {
        assert_eq!(FirstCycle::default().verdict(), Ok(()));
    }

    #[test]
    fn a_failure_with_no_good_fetch_is_a_verdict_of_error() {
        let mut cycle = FirstCycle::default();
        cycle.note(&failed("acme/b"));
        cycle.note(&failed("acme/a"));
        let error = cycle.verdict().unwrap_err();
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
        assert_eq!(cycle.verdict(), Ok(()));
    }
}
