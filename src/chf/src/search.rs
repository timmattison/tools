//! The background search.
//!
//! A person types a key and the query changes. Each query gets a generation
//! number. A new key cancels the old search, because its result is stale.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use crate::query::{Found, Query};
use crate::session::Session;

/// One session that matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The index of the session in the catalog.
    pub session: usize,
    /// What matched.
    pub found: Found,
}

/// One search over a batch of sessions.
#[derive(Debug, Clone)]
pub struct Job {
    /// The generation of the query.
    pub generation: u64,
    /// The query.
    pub query: Arc<Query>,
    /// The sessions, each with its catalog index.
    pub sessions: Vec<(usize, Arc<Session>)>,
}

/// The result of one job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hits {
    /// The generation of the job.
    pub generation: u64,
    /// The hits, in the order of the job.
    pub hits: Vec<Hit>,
}

/// Run a job. Gives `None` when `latest` moved past the generation of the job.
#[must_use]
pub fn run_job(job: &Job, latest: &AtomicU64) -> Option<Hits> {
    let _ = latest;
    Some(Hits {
        generation: job.generation,
        hits: Vec::new(),
    })
}

/// A worker thread that runs jobs.
pub struct Searcher {
    _private: (),
}

impl Searcher {
    /// Start the worker.
    #[must_use]
    pub fn spawn() -> Self {
        Searcher { _private: () }
    }

    /// Queue a job and mark its generation as the latest.
    pub fn submit(&self, job: Job) {
        let _ = job;
    }

    /// Take a finished result, when one is ready.
    #[must_use]
    pub fn try_recv(&self) -> Option<Hits> {
        None
    }

    /// Wait for a result for at most `timeout`.
    #[must_use]
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Hits> {
        let _ = timeout;
        None
    }
}
