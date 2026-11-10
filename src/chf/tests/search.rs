//! Behavior tests for `chf::search`.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error"
)]

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chf::extract::extract;
use chf::query::{Found, Query};
use chf::search::{run_job, Hit, Hits, Job, Searcher};
use chf::session::{MessageSpan, Role, Session};
use claude_transcript::SessionId;

const UUID: &str = "4733ee2a-1ad6-4619-a01a-11840b8e1901";
const BOUND: Duration = Duration::from_secs(60);

fn session(message: &str) -> Arc<Session> {
    let mut base = extract(
        SessionId::parse(UUID).unwrap(),
        PathBuf::from("/x/session.jsonl"),
        b"",
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    base.text = message.to_string();
    base.messages = vec![MessageSpan {
        role: Role::User,
        start: 0,
        end: message.len(),
    }];
    Arc::new(base)
}

fn job(generation: u64, query: &str) -> Job {
    Job {
        generation,
        query: Arc::new(Query::parse(query)),
        sessions: vec![
            (7, session("needle here")),
            (8, session("nothing")),
            (9, session("another needle")),
        ],
    }
}

fn sessions_of(hits: &Hits) -> Vec<usize> {
    hits.hits.iter().map(|h| h.session).collect()
}

#[test]
fn run_job_gives_hits_in_job_order_with_catalog_indices() {
    let latest = AtomicU64::new(5);
    let hits = run_job(&job(5, "needle"), &latest).unwrap();
    assert_eq!(hits.generation, 5);
    assert_eq!(sessions_of(&hits), vec![7, 9]);
    assert!(hits.hits.iter().all(|h| matches!(h.found, Found::At(_))));
}

#[test]
fn run_job_gives_none_when_latest_moved_past() {
    let latest = AtomicU64::new(6);
    assert_eq!(run_job(&job(5, "needle"), &latest), None);
}

#[test]
fn run_job_with_an_empty_query_hits_each_session() {
    let latest = AtomicU64::new(1);
    let hits = run_job(&job(1, ""), &latest).unwrap();
    assert_eq!(
        hits.hits,
        vec![
            Hit {
                session: 7,
                found: Found::Any
            },
            Hit {
                session: 8,
                found: Found::Any
            },
            Hit {
                session: 9,
                found: Found::Any
            },
        ]
    );
}

#[test]
fn searcher_runs_a_job_and_keeps_the_latest() {
    let searcher = Searcher::spawn();
    searcher.submit(job(1, "needle"));
    let first = searcher.recv_timeout(BOUND).unwrap();
    assert_eq!(first.generation, 1);
    assert_eq!(sessions_of(&first), vec![7, 9]);

    searcher.submit(job(2, "nothing"));
    searcher.submit(job(3, "another"));
    loop {
        let hits = searcher.recv_timeout(BOUND).unwrap();
        if hits.generation == 3 {
            assert_eq!(sessions_of(&hits), vec![9]);
            break;
        }
        assert_eq!(hits.generation, 2);
    }
    assert!(searcher.try_recv().is_none());
}
