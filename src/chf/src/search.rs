//! The background search.
//!
//! A person types a key and the query changes. Each query has a generation
//! number. A new key cancels the old search, because the result of the old
//! search is stale. A job with an old generation stops at the next session,
//! and the worker drops a queued job with an old generation.
//!
//! The search runs on a rayon pool of its own. The index fills the global
//! pool. Rayon starts a job from outside a pool only when the threads of that
//! pool have no other work, so a search on the global pool waits for the index.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rayon::prelude::*;

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

/// Run a job on the current rayon pool.
///
/// The worker of `Searcher` runs each job on a pool of its own.
///
/// Before each session, the job reads `latest`. When `latest` differs from the
/// generation of the job, the job stops and the result is `None`.
#[must_use]
pub fn run_job(job: &Job, latest: &AtomicU64) -> Option<Hits> {
    let hits: Option<Vec<Option<Hit>>> = job
        .sessions
        .par_iter()
        .map(|(index, session)| {
            if latest.load(Ordering::Acquire) != job.generation {
                return None;
            }
            Some(job.query.find(session).map(|found| Hit {
                session: *index,
                found,
            }))
        })
        .collect();
    Some(Hits {
        generation: job.generation,
        hits: hits?.into_iter().flatten().collect(),
    })
}

/// A worker thread that runs jobs.
///
/// The worker ends when the `Searcher` drops.
pub struct Searcher {
    jobs: Sender<Job>,
    hits: Receiver<Hits>,
    latest: Arc<AtomicU64>,
}

impl Searcher {
    /// Start the worker thread and the rayon pool of the search.
    ///
    /// # Panics
    ///
    /// Panics when the operating system cannot start the threads of the
    /// search, as `std::thread::spawn` does.
    #[must_use]
    pub fn spawn() -> Self {
        let pool = rayon::ThreadPoolBuilder::new()
            .thread_name(|index| format!("chf-search-{index}"))
            .build()
            .expect("the operating system must start the threads of the search");
        let (job_tx, job_rx) = channel::<Job>();
        let (hit_tx, hit_rx) = channel::<Hits>();
        let latest = Arc::new(AtomicU64::new(0));
        let worker_latest = Arc::clone(&latest);
        thread::spawn(move || work(&pool, &job_rx, &hit_tx, &worker_latest));
        Searcher {
            jobs: job_tx,
            hits: hit_rx,
            latest,
        }
    }

    /// Store the generation of `job` as the latest, then queue the job.
    pub fn submit(&self, job: Job) {
        self.latest.store(job.generation, Ordering::Release);
        // A send fails only when the worker has ended. Then no job can run.
        let _ = self.jobs.send(job);
    }

    /// Take a finished result, when one is ready.
    #[must_use]
    pub fn try_recv(&self) -> Option<Hits> {
        self.hits.try_recv().ok()
    }

    /// Wait for a result for at most `timeout`.
    ///
    /// This function serves tests and callers that wait. It is not a
    /// performance bound.
    #[must_use]
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Hits> {
        self.hits.recv_timeout(timeout).ok()
    }
}

/// The loop of the worker. It takes a job and each job already queued, keeps
/// the jobs of the latest generation, and runs them in order.
fn work(pool: &rayon::ThreadPool, jobs: &Receiver<Job>, hits: &Sender<Hits>, latest: &AtomicU64) {
    while let Ok(first) = jobs.recv() {
        let mut batch = vec![first];
        while let Ok(next) = jobs.try_recv() {
            batch.push(next);
        }
        let current = latest.load(Ordering::Acquire);
        for job in batch.iter().filter(|job| job.generation == current) {
            if let Some(result) = pool.install(|| run_job(job, latest)) {
                if hits.send(result).is_err() {
                    return;
                }
            }
        }
    }
}
