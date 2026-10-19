//! Cancels a run on GitHub on a thread of its own.
//!
//! The request to cancel a run takes some time. The event loop and the worker
//! must never wait for it. A [`Canceller`] starts the request and returns at
//! once. The event loop reads the results on each pass with
//! [`Canceller::finished`].
//!
//! The worker does not send the cancel. A refresh cycle over many repositories
//! can take tens of seconds, and the worker reads its commands only between two
//! cycles. A cancel through the worker would wait for the end of the cycle.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use crate::github::{GhSource, SourceError};
use crate::job::Job;
use crate::run::{Run, RunId};
use crate::target::RepoName;

/// The name of the thread that sends one cancel.
const THREAD_NAME: &str = "ha-cancel";

/// The result of one cancel request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelOutcome {
    /// The repository of the run.
    pub repo: RepoName,
    /// The run that `ha` asked GitHub to cancel.
    pub run: RunId,
    /// `Ok` when GitHub accepted the cancel, or the error of the request.
    pub result: Result<(), SourceError>,
}

/// Starts the cancel of a run and reports the results later.
pub trait Canceller {
    /// Starts to cancel `run` of `repo`, and returns at once.
    fn cancel(&self, repo: &RepoName, run: RunId);

    /// Gives the next result that arrived, or `None`.
    fn finished(&self) -> Option<CancelOutcome>;
}

/// A cancel that waits for the answer of the user.
///
/// It keeps what the box shows, copied from the board at the time of `c`.
/// A later refresh or a later move of the highlight does not change it, thus
/// `y` cancels the run that the box names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    /// The repository that holds the run.
    pub repo: RepoName,
    /// The run to cancel.
    pub run: RunId,
    /// The name of the workflow.
    pub workflow: String,
    /// The head branch. The text is empty when GitHub gives none.
    pub branch: String,
    /// The display title of the run.
    pub title: String,
    /// The name of the job of the selected job row. `None` for a run row.
    pub job: Option<String>,
}

impl Confirm {
    /// Copies the fields that the box shows from `run`, and the name of `job`.
    #[must_use]
    pub fn new(run: &Run, job: Option<&Job>) -> Confirm {
        Confirm {
            repo: run.repo.clone(),
            run: run.id,
            workflow: run.workflow.clone(),
            branch: run.branch.clone(),
            title: run.title.clone(),
            job: job.map(|job| job.name.clone()),
        }
    }
}

/// The canceller that sends each cancel through `gh`.
///
/// Each cancel runs on a thread of its own. The thread ends because the `gh`
/// child of the [`GhSource`] has a time limit.
pub struct GhCanceller {
    /// The source of this canceller. The worker has another one.
    source: Arc<GhSource>,
    /// The end that each thread uses to send its outcome.
    sender: Sender<CancelOutcome>,
    /// The end that [`Canceller::finished`] reads.
    outcomes: Receiver<CancelOutcome>,
}

impl GhCanceller {
    /// Builds a canceller that sends each cancel through `source`.
    #[must_use]
    pub fn new(source: GhSource) -> GhCanceller {
        let (sender, outcomes) = mpsc::channel();
        GhCanceller {
            source: Arc::new(source),
            sender,
            outcomes,
        }
    }
}

impl Canceller for GhCanceller {
    fn cancel(&self, repo: &RepoName, run: RunId) {
        let source = Arc::clone(&self.source);
        let sender = self.sender.clone();
        let thread_repo = repo.clone();
        let spawned = thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || {
                let result = source.cancel_run(&thread_repo, run);
                // The receiver is gone when `ha` ends. Nobody reads the outcome then.
                let _ = sender.send(CancelOutcome {
                    repo: thread_repo,
                    run,
                    result,
                });
            });
        if let Err(error) = spawned {
            let _ = self.sender.send(CancelOutcome {
                repo: repo.clone(),
                run,
                result: Err(SourceError::Failed(format!(
                    "cannot start a thread: {error}"
                ))),
            });
        }
    }

    fn finished(&self) -> Option<CancelOutcome> {
        self.outcomes.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use super::*;

    /// A time limit that no stub that finishes comes near.
    const GENEROUS_LIMIT: Duration = Duration::from_secs(60);

    /// How long a test waits for an outcome before it fails.
    const OUTCOME_DEADLINE: Duration = Duration::from_secs(60);

    /// The answer of GitHub for a cancel that it accepted.
    const ACCEPTED: &str = "HTTP/2.0 202 Accepted\nContent-Type: application/json; charset=utf-8\r\nX-Ratelimit-Limit: 5000\r\nX-Ratelimit-Remaining: 4999\r\nX-Ratelimit-Reset: 1791977652\r\n\r\n{}";

    /// The answer of GitHub for a run that finished.
    const CONFLICT: &str = "HTTP/2.0 409 Conflict\nContent-Type: application/json\r\n\r\n{\"message\":\"Cannot cancel a workflow run that is completed.\"}";

    fn repo(text: &str) -> RepoName {
        RepoName::parse(text).expect("a valid repository")
    }

    /// Writes an executable `gh` script that runs `body` and returns the
    /// canceller that uses it.
    fn canceller(dir: &TempDir, body: &str) -> GhCanceller {
        let path = dir.path().join("gh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        GhCanceller::new(GhSource::with_program(path, GENEROUS_LIMIT))
    }

    /// A script body that prints `answer` from a file and exits with `exit`.
    fn answer_body(dir: &TempDir, answer: &str, exit: u8) -> String {
        std::fs::write(dir.path().join("answer.out"), answer).unwrap();
        format!("cat \"$(dirname \"$0\")/answer.out\"\nexit {exit}")
    }

    /// Reads outcomes every 20 ms until one arrives, for at most 60 s.
    fn wait_for_outcome(canceller: &GhCanceller) -> CancelOutcome {
        let start = Instant::now();
        while start.elapsed() < OUTCOME_DEADLINE {
            if let Some(outcome) = canceller.finished() {
                return outcome;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("no outcome arrived");
    }

    /// A script body that waits for the file `release` next to the script, for
    /// at most 60 s, and then prints the accepted answer. The script reads
    /// both files through its own path, so no path enters the script text.
    fn waiting_body(dir: &TempDir) -> String {
        std::fs::write(dir.path().join("answer.out"), ACCEPTED).unwrap();
        "i=0\nwhile [ ! -e \"$(dirname \"$0\")/release\" ] && [ \"$i\" -lt 600 ]; do sleep 0.1; i=$((i + 1)); done\ncat \"$(dirname \"$0\")/answer.out\"".to_owned()
    }

    #[test]
    fn a_confirm_copies_what_the_box_shows_from_the_run_and_the_job() {
        let time = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH;
        let run = Run {
            id: RunId(39_149_840_084),
            repo: repo("timmattison/tools"),
            workflow: "CI".to_owned(),
            branch: "main".to_owned(),
            title: "fix(ha): show the run ID".to_owned(),
            state: crate::run::RunState::Running,
            started_at: time,
            updated_at: time,
            url: String::new(),
        };
        let job = Job {
            id: crate::job::JobId(5),
            name: "build (ubuntu-latest)".to_owned(),
            state: crate::run::RunState::Running,
            created_at: None,
            started_at: None,
            completed_at: None,
            url: String::new(),
        };

        let with_job = Confirm::new(&run, Some(&job));
        assert_eq!(with_job.repo, repo("timmattison/tools"));
        assert_eq!(with_job.run, RunId(39_149_840_084));
        assert_eq!(with_job.workflow, "CI");
        assert_eq!(with_job.branch, "main");
        assert_eq!(with_job.title, "fix(ha): show the run ID");
        assert_eq!(with_job.job.as_deref(), Some("build (ubuntu-latest)"));
        assert_eq!(Confirm::new(&run, None).job, None);
    }

    #[test]
    fn cancel_returns_before_gh_ends_and_the_outcome_arrives_later() {
        let dir = TempDir::new().unwrap();
        let release = dir.path().join("release");
        let canceller = canceller(&dir, &waiting_body(&dir));

        canceller.cancel(&repo("o/r"), RunId(7));

        assert_eq!(canceller.finished(), None);
        std::fs::write(&release, "").unwrap();
        assert_eq!(
            wait_for_outcome(&canceller),
            CancelOutcome {
                repo: repo("o/r"),
                run: RunId(7),
                result: Ok(())
            }
        );
    }

    #[test]
    fn a_stub_in_a_directory_whose_name_holds_a_quote_still_gives_its_outcome() {
        let dir = tempfile::Builder::new().prefix("it's-").tempdir().unwrap();
        let release = dir.path().join("release");
        let canceller = canceller(&dir, &waiting_body(&dir));

        canceller.cancel(&repo("o/r"), RunId(7));

        std::fs::write(&release, "").unwrap();
        assert_eq!(
            wait_for_outcome(&canceller),
            CancelOutcome {
                repo: repo("o/r"),
                run: RunId(7),
                result: Ok(())
            }
        );
    }

    #[test]
    fn two_cancels_give_two_outcomes() {
        let dir = TempDir::new().unwrap();
        let body = answer_body(&dir, ACCEPTED, 0);
        let canceller = canceller(&dir, &body);

        canceller.cancel(&repo("o/r"), RunId(7));
        canceller.cancel(&repo("o/r"), RunId(8));

        let mut outcomes = vec![wait_for_outcome(&canceller), wait_for_outcome(&canceller)];
        outcomes.sort_by_key(|outcome| outcome.run.0);
        let runs: Vec<(RunId, Result<(), SourceError>)> = outcomes
            .into_iter()
            .map(|outcome| (outcome.run, outcome.result))
            .collect();
        assert_eq!(runs, vec![(RunId(7), Ok(())), (RunId(8), Ok(()))]);
        assert_eq!(canceller.finished(), None);
    }

    #[test]
    fn a_refused_cancel_gives_the_error_in_its_outcome() {
        let dir = TempDir::new().unwrap();
        let body = answer_body(&dir, CONFLICT, 1);
        let canceller = canceller(&dir, &body);

        canceller.cancel(&repo("o/r"), RunId(7));

        let outcome = wait_for_outcome(&canceller);
        assert_eq!(outcome.run, RunId(7));
        assert_eq!(
            outcome.result,
            Err(SourceError::Http {
                status: 409,
                message: "Cannot cancel a workflow run that is completed.".to_owned()
            })
        );
    }
}
