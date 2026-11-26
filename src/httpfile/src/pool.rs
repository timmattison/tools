//! A fixed pool of worker threads that answers the requests of one
//! `tiny_http` server, and tells a dead accept thread from a deliberate
//! shutdown.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

/// The message of the error that [`Pool::join`] returns when a worker
/// panicked and no acceptor failed.
const WORKER_PANICKED: &str = "a worker thread of the pool panicked";

/// A running pool of worker threads answering the requests of one
/// `tiny_http::Server`.
///
/// `tiny_http::Server::recv()` returns `Err` for two unrelated events that
/// carry no reliable distinction of their own: a deliberate [`Pool::shutdown`]
/// (`server.unblock()`) and a dead accept thread reporting a real failure (an
/// `accept()` error, such as EMFILE — see the `tiny_http` source: on such an
/// error it pushes one `Message::Error` and stops accepting). `Pool` tells
/// them apart with a shutdown flag it owns rather than by inspecting the
/// error: [`Pool::shutdown`] sets that flag before it releases anyone, so a
/// worker that wakes to a flag already set knows the release was requested,
/// and a worker that wakes to a flag still clear knows the accept thread died
/// on its own.
///
/// Consumed by whichever of [`Pool::join`] or [`Pool::shutdown`] ends it —
/// a `Pool` that has already given up its workers has nothing left to offer.
pub struct Pool {
    server: Arc<tiny_http::Server>,
    shutdown: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<std::io::Error>>>,
    handles: Vec<JoinHandle<()>>,
}

impl Pool {
    /// Spawns a fixed pool of `workers` threads that pass each request of
    /// `server` to `handler`, and returns the [`Pool`] that owns them.
    ///
    /// At least one worker is always spawned, even when `workers` is `0`.
    ///
    /// `handler` answers one request. Each worker holds a shared reference to
    /// it, so it runs on many threads at the same time. An `Err` from
    /// `handler` is swallowed, because the failure of one request must never
    /// end a worker or the pool. A served file that fails to read part way
    /// through the response is an example. A client that hangs up is not:
    /// `tiny_http` 0.12's `Request::respond` turns `BrokenPipe`,
    /// `ConnectionReset`, `ConnectionAborted` and `ConnectionRefused` into
    /// `Ok`, so a hang-up never reaches the pool as an `Err`. Only an
    /// unexplained `recv()` error ends the pool — see the docs of [`Pool`]
    /// for how a real accept failure is told apart from a deliberate shutdown.
    ///
    /// A panic in `handler` is not swallowed. It ends its worker, and
    /// [`Pool::join`] reports it.
    #[must_use]
    pub fn spawn<F>(server: Arc<tiny_http::Server>, workers: usize, handler: F) -> Self
    where
        F: Fn(tiny_http::Request) -> std::io::Result<()> + Send + Sync + 'static,
    {
        let worker_count = workers.max(1);
        let shutdown = Arc::new(AtomicBool::new(false));
        let failure: Arc<Mutex<Option<std::io::Error>>> = Arc::new(Mutex::new(None));
        let handler = Arc::new(handler);

        let handles = (0..worker_count)
            .map(|_| {
                let server = Arc::clone(&server);
                let handler = Arc::clone(&handler);
                let shutdown = Arc::clone(&shutdown);
                let failure = Arc::clone(&failure);
                std::thread::spawn(move || loop {
                    match server.recv() {
                        Ok(request) => {
                            // The failure of one request (a served file
                            // that fails to read part way through, for
                            // example) is swallowed, so it cannot end a
                            // worker or the pool. `tiny_http` itself turns a
                            // client that hangs up into `Ok`.
                            let _ = handler(request);
                        }
                        Err(e) => {
                            // `recv()` errors for two events tiny_http itself
                            // does not distinguish: a deliberate `unblock()`
                            // (what `Pool::shutdown` uses) and a dead accept
                            // thread reporting a real failure. `shutdown` is
                            // OUR flag, so `swap` tells the two apart from
                            // our own state without looking at `e` at all —
                            // never by matching its message or `ErrorKind`,
                            // both of which are the third-party crate's
                            // implementation detail.
                            let already_shutting_down = shutdown.swap(true, Ordering::SeqCst);
                            if !already_shutting_down {
                                // We are the one worker tiny_http's accept
                                // loop handed the error to — it pushes exactly
                                // one `Message::Error` and then stops
                                // accepting. Record the failure and release
                                // every other worker, which is otherwise
                                // blocked in `recv()` forever: nothing will
                                // ever wake them, because the accept thread
                                // that used to feed them requests has already
                                // exited.
                                *failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(e);
                                for _ in 1..worker_count {
                                    server.unblock();
                                }
                            }
                            break;
                        }
                    }
                })
            })
            .collect();

        Self {
            server,
            shutdown,
            failure,
            handles,
        }
    }

    /// Blocks until every worker has exited, then reports the accept failure
    /// that ended the pool, if a real one — as opposed to a deliberate
    /// [`Pool::shutdown`] — occurred, or else a worker that panicked.
    ///
    /// # Errors
    ///
    /// Returns the `std::io::Error` a dead acceptor reported. If no acceptor
    /// failed but a worker panicked, returns an error that says so. A pool
    /// ended by [`Pool::shutdown`] with no worker that panicked returns
    /// `Ok(())` here.
    pub fn join(self) -> Result<(), std::io::Error> {
        let mut panicked = false;
        for handle in self.handles {
            panicked |= handle.join().is_err();
        }
        let failure = self
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match failure {
            Some(e) => Err(e),
            None if panicked => Err(std::io::Error::other(WORKER_PANICKED)),
            None => Ok(()),
        }
    }

    /// Signals a deliberate shutdown, releases every worker blocked in
    /// `recv()`, and joins them.
    ///
    /// The shutdown flag is set BEFORE any worker is released — that order is
    /// what makes this distinguishable from a real accept failure. A worker
    /// that wakes from `recv()` with an error always checks the flag first: a
    /// flag already set means this shutdown asked for the release, so the
    /// worker exits quietly; a flag still clear means nothing asked for it, so
    /// the worker treats the error as real (see [`Pool`]'s docs).
    ///
    /// # Errors
    ///
    /// Returns an error when a worker panicked, as [`Pool::join`] does. The
    /// deliberate shutdown itself never fails.
    pub fn shutdown(self) -> Result<(), std::io::Error> {
        self.shutdown.store(true, Ordering::SeqCst);
        for _ in 0..self.handles.len() {
            self.server.unblock();
        }
        self.join()
    }
}
