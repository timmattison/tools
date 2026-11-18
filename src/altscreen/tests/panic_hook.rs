//! The panic hook is process-global, so these tests have their own binary
//! and run one at a time.

use std::io;
use std::panic;
use std::sync::{Arc, Mutex, PoisonError};

use altscreen::{Options, Port, Session};

/// One test at a time: each test installs and removes a global panic hook.
static SERIAL: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    RawOn,
    RawOff,
    Write(Vec<u8>),
    PreviousHook,
}

type Events = Arc<Mutex<Vec<Event>>>;

struct RecordingPort(Events);

fn push(events: &Events, event: Event) {
    events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(event);
}

fn snapshot(events: &Events) -> Vec<Event> {
    events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

impl Port for RecordingPort {
    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        push(&self.0, Event::Write(bytes.to_vec()));
        Ok(())
    }

    fn enable_raw_mode(&self) -> io::Result<()> {
        push(&self.0, Event::RawOn);
        Ok(())
    }

    fn disable_raw_mode(&self) -> io::Result<()> {
        push(&self.0, Event::RawOff);
        Ok(())
    }
}

fn write(text: &str) -> Event {
    Event::Write(text.as_bytes().to_vec())
}

/// The events of one restore, as the hook records them.
fn restore_events() -> Vec<Event> {
    vec![
        write("\x1b[?25h"),
        write("\x1b[?1049l"),
        Event::RawOff,
    ]
}

/// Installs a previous hook that records a marker.
fn install_previous_hook(events: &Events) {
    let events = Arc::clone(events);
    panic::set_hook(Box::new(move |_| push(&events, Event::PreviousHook)));
}

fn enter(events: &Events) -> Session {
    Session::enter_on(Arc::new(RecordingPort(Arc::clone(events))), Options::default())
        .expect("the entry must work")
}

#[test]
fn a_panic_restores_the_terminal_before_the_previous_hook_runs() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let events: Events = Arc::default();
    install_previous_hook(&events);

    let session = enter(&events);
    let entered = snapshot(&events).len();
    assert_eq!(entered, 2, "raw mode on, then the entry write");

    let caught = panic::catch_unwind(|| panic!("boom"));
    assert!(caught.is_err());
    let mut expected = restore_events();
    expected.push(Event::PreviousHook);
    assert_eq!(snapshot(&events)[entered..], expected);

    // Drop the session: the previous hook comes back.
    drop(session);
    let after_drop = snapshot(&events).len();
    let caught = panic::catch_unwind(|| panic!("again"));
    assert!(caught.is_err());
    assert_eq!(snapshot(&events)[after_drop..], [Event::PreviousHook]);

    let _ = panic::take_hook();
}

#[test]
fn a_session_that_drops_while_the_thread_unwinds_does_not_abort() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let events: Events = Arc::default();
    install_previous_hook(&events);

    let inner = Arc::clone(&events);
    let caught = panic::catch_unwind(move || {
        let _session = enter(&inner);
        panic!("boom");
    });
    assert!(caught.is_err());

    let all = snapshot(&events);
    assert_eq!(all[0], Event::RawOn);
    let restores = all.iter().filter(|e| **e == Event::RawOff).count();
    assert!(restores >= 1, "the restore must run: {all:?}");
    assert!(all.contains(&write("\x1b[?1049l")), "leave must be sent: {all:?}");

    let _ = panic::take_hook();
}
