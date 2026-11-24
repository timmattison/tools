//! The shared test doubles of this crate.
//!
//! The panic hook is process-global, so every test that enters a session holds
//! the one `PANIC_HOOK_LOCK` of this module and uses the one `FakePort`.

use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use crate::Port;

/// The panic hook is process-global, so each test that enters holds this.
pub(crate) static PANIC_HOOK_LOCK: Mutex<()> = Mutex::new(());

/// One thing that a session did to the port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    RawOn,
    RawOff,
    Write(Vec<u8>),
}

/// A port that records each call and fails on request.
#[derive(Default)]
pub(crate) struct FakePort {
    pub(crate) events: Arc<Mutex<Vec<Event>>>,
    pub(crate) fail_write_containing: Option<&'static [u8]>,
    pub(crate) fail_raw_off: bool,
}

impl FakePort {
    fn record(&self, event: Event) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

impl Port for FakePort {
    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        self.record(Event::Write(bytes.to_vec()));
        match self.fail_write_containing {
            Some(marker) if bytes.windows(marker.len()).any(|w| w == marker) => {
                Err(io::Error::other("write failed"))
            }
            _ => Ok(()),
        }
    }

    fn enable_raw_mode(&self) -> io::Result<()> {
        self.record(Event::RawOn);
        Ok(())
    }

    fn disable_raw_mode(&self) -> io::Result<()> {
        self.record(Event::RawOff);
        if self.fail_raw_off {
            Err(io::Error::other("raw off failed"))
        } else {
            Ok(())
        }
    }
}

/// The shared event log of a `FakePort`.
pub(crate) type Events = Arc<Mutex<Vec<Event>>>;

/// Makes a fake port, lets `configure` change it, and returns its event log.
pub(crate) fn fake(configure: impl FnOnce(&mut FakePort)) -> (Arc<FakePort>, Events) {
    let mut port = FakePort::default();
    configure(&mut port);
    let events = Arc::clone(&port.events);
    (Arc::new(port), events)
}

/// A copy of the events so far.
pub(crate) fn snapshot(events: &Events) -> Vec<Event> {
    events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}
