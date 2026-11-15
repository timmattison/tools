//! Behavior tests for `chf::search` that need the global rayon pool to themselves.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error"
)]

use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::time::{Duration, SystemTime};

use chf::extract::extract;
use chf::query::Query;
use chf::search::{Job, Searcher};
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

/// Frees the blocked threads of the global pool, also when an assertion fails.
struct Release(Arc<Barrier>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.wait();
    }
}

/// The search does not wait for work on the global rayon pool, which the
/// index fills. The test blocks every thread of that pool, then expects an
/// answer to a job anyway.
#[test]
fn a_search_answers_while_the_global_pool_is_busy() {
    let threads = rayon::current_num_threads();
    let started = Arc::new(Barrier::new(threads + 1));
    let release = Arc::new(Barrier::new(threads + 1));
    for _ in 0..threads {
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        rayon::spawn(move || {
            started.wait();
            release.wait();
        });
    }
    started.wait();
    let _release = Release(release);

    let searcher = Searcher::spawn();
    searcher.submit(Job {
        generation: 1,
        query: Arc::new(Query::parse("needle")),
        sessions: vec![
            (7, session("needle here")),
            (8, session("nothing")),
            (9, session("another needle")),
        ],
    });

    let hits = searcher
        .recv_timeout(BOUND)
        .expect("the search must answer while the global pool is busy");
    assert_eq!(hits.generation, 1);
    let found: Vec<usize> = hits.hits.iter().map(|h| h.session).collect();
    assert_eq!(found, vec![7, 9]);
}
