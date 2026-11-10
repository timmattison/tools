//! The store and the cache. Stub: the real code arrives in the next commit.

use std::path::{Path, PathBuf};

use crate::session::Session;

/// One event of an indexing run.
#[derive(Debug)]
pub enum IndexEvent {
    /// The count of session files found. Sent once, before the first `Session`.
    Total(usize),
    /// One indexed session.
    Session(Box<Session>),
    /// A transcript that chf cannot open or read.
    Unreadable {
        /// The path of the transcript, or of a folder that chf cannot list.
        path: PathBuf,
        /// The text of the io error.
        reason: String,
    },
    /// The last event of a run.
    Done,
}

/// Index the store synchronously. Each event goes to `sink`.
pub fn run(_projects: &Path, _cache: &Path, sink: &(dyn Fn(IndexEvent) + Sync)) {
    sink(IndexEvent::Done);
}

/// Run `run` on a background thread.
#[must_use]
pub fn spawn(projects: PathBuf, cache: PathBuf) -> std::sync::mpsc::Receiver<IndexEvent> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        run(&projects, &cache, &|event| {
            let _ = sender.send(event);
        });
    });
    receiver
}
