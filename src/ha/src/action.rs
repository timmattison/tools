//! Actions that `y` sends to GitHub, each on a thread of its own.
//!
//! The request for an action takes some time. The event loop and the worker
//! must never wait for it. An [`ActionSender`] starts the request and returns
//! at once. The event loop reads the results on each pass with
//! [`ActionSender::finished`].
//!
//! A key press gives a [`Request`]. The request turns into a [`Prompt`]: a
//! question that holds the exact [`Action`], or a refusal. The box, the status
//! line, and the request to GitHub all read the same [`Action`], thus the
//! words and the request cannot disagree.
//!
//! The worker does not send the request. A refresh cycle over many
//! repositories can take tens of seconds, and the worker reads its commands
//! only between two cycles. An action through the worker would wait for the
//! end of the cycle.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use crate::github::{GhSource, SourceError};
use crate::job::{Job, JobId};
use crate::queue::PullNumber;
use crate::run::{Run, RunId};
use crate::target::RepoName;

/// The name of the thread that sends one action.
const THREAD_NAME: &str = "ha-action";

/// The title in the top border of the box of a cancel.
const CANCEL_TITLE: &str = " Cancel run? ";
/// The note in the box when a job is named: GitHub cancels only a whole run.
const CANCEL_JOB_NOTE: &str = "GitHub cannot cancel one job. y cancels every job of this run.";
/// The key that cancels the run.
const CANCEL_KEY: &str = "y cancel the run";

/// The title in the top border of the box of a workflow re-run.
const RERUN_WORKFLOW_TITLE: &str = " Re-run workflow? ";
/// The title in the top border of the box of a job re-run.
const RERUN_JOB_TITLE: &str = " Re-run job? ";
/// The note in the box of a job re-run: GitHub also re-runs the dependent jobs.
const RERUN_JOB_NOTE: &str = "GitHub also re-runs each job that needs this job.";
/// The key that re-runs every job of the run.
const RERUN_WORKFLOW_KEY: &str = "y re-run all jobs";
/// The key that re-runs one job.
const RERUN_JOB_KEY: &str = "y re-run this job";

/// What `y` asks GitHub to do. The box and the status line take their words
/// from this value, thus the words and the request cannot disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Cancel the whole run. `job` names the highlighted job, for the box only.
    Cancel {
        /// The name of the highlighted job. `None` for a run row.
        job: Option<String>,
    },
    /// Re-run every job of the run.
    RerunWorkflow,
    /// Re-run one job and the jobs that need it.
    RerunJob {
        /// The job to re-run, copied at the key press.
        id: JobId,
        /// The name of the job, for the box and the status line.
        name: String,
    },
}

impl Action {
    /// The title in the top border of the box.
    #[must_use]
    pub fn box_title(&self) -> &'static str {
        match self {
            Action::Cancel { .. } => CANCEL_TITLE,
            Action::RerunWorkflow => RERUN_WORKFLOW_TITLE,
            Action::RerunJob { .. } => RERUN_JOB_TITLE,
        }
    }

    /// The name of the job that the box shows as `job: {name}`, or `None`.
    #[must_use]
    pub fn box_job(&self) -> Option<&str> {
        match self {
            Action::Cancel { job } => job.as_deref(),
            Action::RerunJob { name, .. } => Some(name),
            Action::RerunWorkflow => None,
        }
    }

    /// The note that the box shows below the job, or `None` when the box
    /// names no job.
    #[must_use]
    pub fn box_note(&self) -> Option<&'static str> {
        match self {
            Action::Cancel { job } => job.as_ref().map(|_| CANCEL_JOB_NOTE),
            Action::RerunJob { .. } => Some(RERUN_JOB_NOTE),
            Action::RerunWorkflow => None,
        }
    }

    /// The key line of the box that sends the action.
    #[must_use]
    pub fn box_key(&self) -> &'static str {
        match self {
            Action::Cancel { .. } => CANCEL_KEY,
            Action::RerunWorkflow => RERUN_WORKFLOW_KEY,
            Action::RerunJob { .. } => RERUN_JOB_KEY,
        }
    }

    /// The status line for the time when `ha` starts the request.
    #[must_use]
    pub fn started_text(&self, run: RunId) -> String {
        format!("{} {}…", self.gerund(), self.object(run))
    }

    /// The status line for the time when GitHub accepts the request.
    #[must_use]
    pub fn accepted_text(&self, run: RunId) -> String {
        format!("GitHub is {} {}", self.gerund(), self.object(run))
    }

    /// The status line for the time when GitHub refuses the request.
    ///
    /// The run ID comes before the error, because the status line cuts a
    /// long line at its end. Thus a narrow terminal still shows which run the
    /// error is about.
    #[must_use]
    pub fn refused_text(&self, run: RunId, error: &SourceError) -> String {
        format!("could not {} {}: {error}", self.verb(), self.object(run))
    }

    /// The verb of the action in the form that ends in `-ing`.
    fn gerund(&self) -> &'static str {
        match self {
            Action::Cancel { .. } => "cancelling",
            Action::RerunWorkflow | Action::RerunJob { .. } => "re-running",
        }
    }

    /// The verb of the action in the base form.
    fn verb(&self) -> &'static str {
        match self {
            Action::Cancel { .. } => "cancel",
            Action::RerunWorkflow | Action::RerunJob { .. } => "re-run",
        }
    }

    /// What the action works on, in words.
    fn object(&self, run: RunId) -> String {
        match self {
            Action::Cancel { .. } | Action::RerunWorkflow => format!("run {}", run.0),
            Action::RerunJob { name, .. } => format!("job {name} of run {}", run.0),
        }
    }
}

/// The key press that asks for an action on the highlighted row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// The key `c`: cancel the run of the highlighted row.
    Cancel,
}

/// What a key press on the highlighted row gives: a question that holds the
/// exact action, or a refusal. A question without its action cannot exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// A box that asks the user to confirm the action.
    Confirm(Confirm),
    /// The text for the status line: the row cannot take the action.
    Refuse(String),
}

impl Request {
    /// The verb of the request in the base form.
    fn verb(self) -> &'static str {
        match self {
            Request::Cancel => "cancel",
        }
    }

    /// Gives the prompt for this request on the highlighted row, or `None`
    /// when no row is selected.
    ///
    /// `pull` is the pull request of the highlighted row, if it is one. It
    /// wins over `target`. `target` is the run of the highlighted row, with
    /// the highlighted job when there is one. The box keeps a copy of what it
    /// shows, thus a later refresh does not change the run that `y` acts on.
    #[must_use]
    pub fn prompt(
        self,
        pull: Option<PullNumber>,
        target: Option<(&Run, Option<&Job>)>,
    ) -> Option<Prompt> {
        if let Some(pull) = pull {
            return Some(Prompt::Refuse(format!(
                "pull request #{} is not a run: there is nothing to {}",
                pull.0,
                self.verb()
            )));
        }
        let (run, job) = target?;
        Some(match self {
            Request::Cancel if run.state.is_active() => Prompt::Confirm(Confirm::new(
                run,
                Action::Cancel {
                    job: job.map(|job| job.name.clone()),
                },
            )),
            Request::Cancel => Prompt::Refuse(format!(
                "run {} finished: there is nothing to {}",
                run.id.0,
                self.verb()
            )),
        })
    }
}

/// The result of one action request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionOutcome {
    /// The repository of the run.
    pub repo: RepoName,
    /// The run that `ha` asked GitHub to act on.
    pub run: RunId,
    /// The action that `ha` sent.
    pub action: Action,
    /// `Ok` when GitHub accepted the action, or the error of the request.
    pub result: Result<(), SourceError>,
}

/// Starts an action on a run and reports the results later.
pub trait ActionSender {
    /// Starts `action` on `run` of `repo`, and returns at once.
    fn start(&self, repo: &RepoName, run: RunId, action: &Action);

    /// Gives the next result that arrived, or `None`.
    fn finished(&self) -> Option<ActionOutcome>;
}

/// An action that waits for the answer of the user.
///
/// It keeps what the box shows, copied from the board at the time of the key
/// press. A later refresh or a later move of the highlight does not change it,
/// thus `y` sends the action on the run that the box names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    /// The repository that holds the run.
    pub repo: RepoName,
    /// The run to act on.
    pub run: RunId,
    /// The name of the workflow.
    pub workflow: String,
    /// The head branch. The text is empty when GitHub gives none.
    pub branch: String,
    /// The display title of the run.
    pub title: String,
    /// What `y` sends.
    pub action: Action,
}

impl Confirm {
    /// Copies the fields that the box shows from `run`, and holds `action`.
    #[must_use]
    pub fn new(run: &Run, action: Action) -> Confirm {
        Confirm {
            repo: run.repo.clone(),
            run: run.id,
            workflow: run.workflow.clone(),
            branch: run.branch.clone(),
            title: run.title.clone(),
            action,
        }
    }
}

/// The sender that sends each action through `gh`.
///
/// Each action runs on a thread of its own. The thread ends because the `gh`
/// child of the [`GhSource`] has a time limit.
pub struct GhActionSender {
    /// The source of this sender. The worker has another one.
    source: Arc<GhSource>,
    /// The end that each thread uses to send its outcome.
    sender: Sender<ActionOutcome>,
    /// The end that [`ActionSender::finished`] reads.
    outcomes: Receiver<ActionOutcome>,
}

impl GhActionSender {
    /// Builds a sender that sends each action through `source`.
    #[must_use]
    pub fn new(source: GhSource) -> GhActionSender {
        let (sender, outcomes) = mpsc::channel();
        GhActionSender {
            source: Arc::new(source),
            sender,
            outcomes,
        }
    }
}

impl ActionSender for GhActionSender {
    fn start(&self, repo: &RepoName, run: RunId, action: &Action) {
        let source = Arc::clone(&self.source);
        let sender = self.sender.clone();
        let thread_repo = repo.clone();
        let thread_action = action.clone();
        let spawned = thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || {
                let result = match &thread_action {
                    Action::Cancel { .. } => source.cancel_run(&thread_repo, run),
                    Action::RerunWorkflow => source.rerun_run(&thread_repo, run),
                    Action::RerunJob { id, .. } => source.rerun_job(&thread_repo, *id),
                };
                // The receiver is gone when `ha` ends. Nobody reads the outcome then.
                let _ = sender.send(ActionOutcome {
                    repo: thread_repo,
                    run,
                    action: thread_action,
                    result,
                });
            });
        if let Err(error) = spawned {
            let _ = self.sender.send(ActionOutcome {
                repo: repo.clone(),
                run,
                action: action.clone(),
                result: Err(SourceError::Failed(format!(
                    "cannot start a thread: {error}"
                ))),
            });
        }
    }

    fn finished(&self) -> Option<ActionOutcome> {
        self.outcomes.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use super::*;

    /// A time limit that no stub that finishes comes near.
    const GENEROUS_LIMIT: Duration = Duration::from_secs(60);

    /// How long a test waits for an outcome before it fails.
    const OUTCOME_DEADLINE: Duration = Duration::from_secs(60);

    /// The answer of GitHub for a cancel that it accepted.
    const ACCEPTED: &str = "HTTP/2.0 202 Accepted\nContent-Type: application/json; charset=utf-8\r\nX-Ratelimit-Limit: 5000\r\nX-Ratelimit-Remaining: 4999\r\nX-Ratelimit-Reset: 1791977652\r\n\r\n{}";

    /// The answer of GitHub for a re-run that it created.
    const CREATED: &str = "HTTP/2.0 201 Created\nContent-Type: application/json; charset=utf-8\r\nX-Ratelimit-Limit: 5000\r\nX-Ratelimit-Remaining: 4999\r\nX-Ratelimit-Reset: 1791977652\r\n\r\n{}";

    /// The answer of GitHub for a run that finished.
    const CONFLICT: &str = "HTTP/2.0 409 Conflict\nContent-Type: application/json\r\n\r\n{\"message\":\"Cannot cancel a workflow run that is completed.\"}";

    fn repo(text: &str) -> RepoName {
        RepoName::parse(text).expect("a valid repository")
    }

    /// Writes an executable `gh` script that runs `body` and returns the
    /// canceller that uses it.
    fn canceller(dir: &TempDir, body: &str) -> GhActionSender {
        let path = dir.path().join("gh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        GhActionSender::new(GhSource::with_program(path, GENEROUS_LIMIT))
    }

    /// A script body that prints `answer` from a file and exits with `exit`.
    fn answer_body(dir: &TempDir, answer: &str, exit: u8) -> String {
        std::fs::write(dir.path().join("answer.out"), answer).unwrap();
        format!("cat \"$(dirname \"$0\")/answer.out\"\nexit {exit}")
    }

    /// Reads outcomes every 20 ms until one arrives, for at most 60 s.
    fn wait_for_outcome(canceller: &GhActionSender) -> ActionOutcome {
        let start = Instant::now();
        while start.elapsed() < OUTCOME_DEADLINE {
            if let Some(outcome) = canceller.finished() {
                return outcome;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("no outcome arrived");
    }

    /// A script body that appends its arguments to `args.log` beside the
    /// script, and then prints the answer file.
    fn logging_body(dir: &TempDir, answer: &str) -> String {
        std::fs::write(dir.path().join("answer.out"), answer).unwrap();
        "printf '%s\\n' \"$*\" >> \"$(dirname \"$0\")/args.log\"\ncat \"$(dirname \"$0\")/answer.out\"".to_owned()
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
            visibility: crate::run::Visibility::Private,
            id: RunId(39_149_840_084),
            repo: repo("timmattison/tools"),
            workflow: "CI".to_owned(),
            branch: "main".to_owned(),
            title: "fix(ha): show the run ID".to_owned(),
            state: crate::run::RunState::Running,
            started_at: time,
            created_at: time,
            updated_at: time,
            url: String::new(),
        };

        let with_job = Confirm::new(
            &run,
            Action::Cancel {
                job: Some("build (ubuntu-latest)".to_owned()),
            },
        );
        assert_eq!(with_job.repo, repo("timmattison/tools"));
        assert_eq!(with_job.run, RunId(39_149_840_084));
        assert_eq!(with_job.workflow, "CI");
        assert_eq!(with_job.branch, "main");
        assert_eq!(with_job.title, "fix(ha): show the run ID");
        assert_eq!(
            with_job.action,
            Action::Cancel {
                job: Some("build (ubuntu-latest)".to_owned())
            }
        );
        assert_eq!(
            Confirm::new(&run, Action::Cancel { job: None }).action,
            Action::Cancel { job: None }
        );
    }

    #[test]
    fn cancel_returns_before_gh_ends_and_the_outcome_arrives_later() {
        let dir = TempDir::new().unwrap();
        let release = dir.path().join("release");
        let canceller = canceller(&dir, &waiting_body(&dir));

        canceller.start(&repo("o/r"), RunId(7), &Action::Cancel { job: None });

        assert_eq!(canceller.finished(), None);
        std::fs::write(&release, "").unwrap();
        assert_eq!(
            wait_for_outcome(&canceller),
            ActionOutcome {
                repo: repo("o/r"),
                run: RunId(7),
                action: Action::Cancel { job: None },
                result: Ok(())
            }
        );
    }

    #[test]
    fn a_stub_in_a_directory_whose_name_holds_a_quote_still_gives_its_outcome() {
        let dir = tempfile::Builder::new().prefix("it's-").tempdir().unwrap();
        let release = dir.path().join("release");
        let canceller = canceller(&dir, &waiting_body(&dir));

        canceller.start(&repo("o/r"), RunId(7), &Action::Cancel { job: None });

        std::fs::write(&release, "").unwrap();
        assert_eq!(
            wait_for_outcome(&canceller),
            ActionOutcome {
                repo: repo("o/r"),
                run: RunId(7),
                action: Action::Cancel { job: None },
                result: Ok(())
            }
        );
    }

    #[test]
    fn two_cancels_give_two_outcomes() {
        let dir = TempDir::new().unwrap();
        let body = answer_body(&dir, ACCEPTED, 0);
        let canceller = canceller(&dir, &body);

        canceller.start(&repo("o/r"), RunId(7), &Action::Cancel { job: None });
        canceller.start(&repo("o/r"), RunId(8), &Action::Cancel { job: None });

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

        canceller.start(&repo("o/r"), RunId(7), &Action::Cancel { job: None });

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

    #[test]
    fn rerun_workflow_sends_the_request_of_the_run() {
        let dir = TempDir::new().unwrap();
        let sender = canceller(&dir, &logging_body(&dir, CREATED));

        sender.start(&repo("o/r"), RunId(7), &Action::RerunWorkflow);

        assert_eq!(
            wait_for_outcome(&sender),
            ActionOutcome {
                repo: repo("o/r"),
                run: RunId(7),
                action: Action::RerunWorkflow,
                result: Ok(())
            }
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("args.log")).unwrap(),
            "api -i -X POST repos/o/r/actions/runs/7/rerun\n"
        );
    }

    #[test]
    fn rerun_job_sends_the_request_of_the_job() {
        let dir = TempDir::new().unwrap();
        let sender = canceller(&dir, &logging_body(&dir, CREATED));
        let action = Action::RerunJob {
            id: JobId(11),
            name: "build".to_owned(),
        };

        sender.start(&repo("o/r"), RunId(7), &action);

        assert_eq!(
            wait_for_outcome(&sender),
            ActionOutcome {
                repo: repo("o/r"),
                run: RunId(7),
                action,
                result: Ok(())
            }
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("args.log")).unwrap(),
            "api -i -X POST repos/o/r/actions/jobs/11/rerun\n"
        );
    }

    #[test]
    fn the_words_of_a_rerun_workflow_name_the_run() {
        let action = Action::RerunWorkflow;
        let error = SourceError::Failed("boom".to_owned());
        assert_eq!(action.started_text(RunId(7)), "re-running run 7…");
        assert_eq!(action.accepted_text(RunId(7)), "GitHub is re-running run 7");
        assert_eq!(
            action.refused_text(RunId(7), &error),
            format!("could not re-run run 7: {error}")
        );
    }

    #[test]
    fn the_words_of_a_rerun_job_name_the_job_and_the_run() {
        let action = Action::RerunJob {
            id: JobId(11),
            name: "build (ubuntu-latest)".to_owned(),
        };
        let error = SourceError::Failed("boom".to_owned());
        assert_eq!(
            action.started_text(RunId(7)),
            "re-running job build (ubuntu-latest) of run 7…"
        );
        assert_eq!(
            action.accepted_text(RunId(7)),
            "GitHub is re-running job build (ubuntu-latest) of run 7"
        );
        assert_eq!(
            action.refused_text(RunId(7), &error),
            format!("could not re-run job build (ubuntu-latest) of run 7: {error}")
        );
    }

    #[test]
    fn a_refused_rerun_gives_the_error_in_its_outcome() {
        let dir = TempDir::new().unwrap();
        let forbidden = "HTTP/2.0 403 Forbidden\nContent-Type: application/json\r\n\r\n{\"message\":\"Must have admin rights to Repository.\"}";
        let body = answer_body(&dir, forbidden, 1);
        let sender = canceller(&dir, &body);

        sender.start(&repo("o/r"), RunId(7), &Action::RerunWorkflow);

        let outcome = wait_for_outcome(&sender);
        assert_eq!(outcome.action, Action::RerunWorkflow);
        assert_eq!(
            outcome.result,
            Err(SourceError::Http {
                status: 403,
                message: "Must have admin rights to Repository.".to_owned()
            })
        );
    }

    #[test]
    fn the_words_of_a_rerun_job_keep_a_multi_byte_name() {
        let action = Action::RerunJob {
            id: JobId(11),
            name: "日本語 🎉 café".to_owned(),
        };
        let error = SourceError::Failed("boom".to_owned());
        assert_eq!(
            action.started_text(RunId(7)),
            "re-running job 日本語 🎉 café of run 7…"
        );
        assert_eq!(
            action.accepted_text(RunId(7)),
            "GitHub is re-running job 日本語 🎉 café of run 7"
        );
        assert_eq!(
            action.refused_text(RunId(7), &error),
            format!("could not re-run job 日本語 🎉 café of run 7: {error}")
        );
    }
}
