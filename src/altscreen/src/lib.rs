//! The one entrance to the alternate screen.
//!
//! Some terminal emulators ignore the alternate-screen modes 47, 1047, and
//! 1049. The emulator in the mosh server is one of them. A full-screen tool
//! that only sends the entry code then draws over the old shell text. ratatui
//! sends no clear at startup, and its first frame writes only the cells that
//! are not blank. The old text thus stays visible under the view.
//!
//! This crate enters the alternate screen and clears it in the same write. It
//! gives these guarantees:
//!
//! - The erase and the entry go out in one write.
//! - A failed entry leaves no raw mode behind.
//! - Every exit restores the terminal: the cursor, the screen, and raw mode.
//! - A panic restores the terminal first. The panic message then prints on the
//!   shell screen, where the user can read it.
//! - The session never asks the terminal for the cursor position.
//!
//! # Example
//!
//! ```no_run
//! # fn main() -> std::io::Result<()> {
//! let _session = altscreen::Session::enter(altscreen::Options::default())?;
//! // Draw the view here. The terminal is restored when `_session` drops.
//! # Ok(())
//! # }
//! ```

use std::io;
use std::sync::Arc;

use crossterm::Command;

/// The options of a session.
///
/// Raw mode is on by default. Mouse capture is off by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    raw_mode: bool,
    mouse_capture: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            raw_mode: true,
            mouse_capture: false,
        }
    }
}

impl Options {
    /// Turns raw mode on or off for the session.
    #[must_use]
    pub fn raw_mode(self, on: bool) -> Self {
        Self {
            raw_mode: on,
            ..self
        }
    }

    /// Turns mouse capture on or off for the session.
    #[must_use]
    pub fn mouse_capture(self, on: bool) -> Self {
        Self {
            mouse_capture: on,
            ..self
        }
    }
}

/// The terminal that a session drives.
///
/// Only the tests of this crate implement this trait.
#[doc(hidden)]
pub trait Port: Send + Sync {
    /// Writes all of `bytes` and flushes, as one write.
    ///
    /// # Errors
    ///
    /// Returns the error of the write or of the flush.
    fn write(&self, bytes: &[u8]) -> io::Result<()>;

    /// Turns raw mode on.
    ///
    /// # Errors
    ///
    /// Returns the error of the terminal.
    fn enable_raw_mode(&self) -> io::Result<()>;

    /// Turns raw mode off.
    ///
    /// # Errors
    ///
    /// Returns the error of the terminal.
    fn disable_raw_mode(&self) -> io::Result<()>;
}

/// A live alternate-screen session.
///
/// The terminal is restored when the session drops.
#[must_use = "the terminal is restored when the session drops"]
pub struct Session {
    port: Arc<dyn Port>,
    options: Options,
}

impl Session {
    /// Enters the alternate screen on the real terminal.
    ///
    /// The real terminal is standard output, with crossterm raw mode.
    ///
    /// # Errors
    ///
    /// Returns the error of raw mode or of the entry write. A failed entry
    /// leaves raw mode off.
    pub fn enter(options: Options) -> io::Result<Session> {
        Self::enter_on(Arc::new(StdoutPort), options)
    }

    /// Enters the alternate screen on `port`.
    ///
    /// # Errors
    ///
    /// Returns the error of raw mode or of the entry write.
    #[doc(hidden)]
    pub fn enter_on(port: Arc<dyn Port>, options: Options) -> io::Result<Session> {
        if options.raw_mode {
            port.enable_raw_mode()?;
        }
        if let Err(error) = port.write(entry_bytes(options).as_bytes()) {
            if options.raw_mode {
                let _ = port.disable_raw_mode();
            }
            return Err(error);
        }
        Ok(Session { port, options })
    }
}

impl Drop for Session {
    /// Restores the terminal.
    fn drop(&mut self) {
        restore(self.port.as_ref(), self.options);
    }
}

/// Restores the terminal. Every step runs even when an earlier step fails,
/// and a second call does no harm. Each write is separate, so one failed
/// write does not skip the others.
fn restore(port: &dyn Port, options: Options) {
    if options.mouse_capture {
        send(port, &crossterm::event::DisableMouseCapture);
    }
    send(port, &crossterm::cursor::Show);
    send(port, &crossterm::terminal::LeaveAlternateScreen);
    if options.raw_mode {
        let _ = port.disable_raw_mode();
    }
}

/// Writes one command to the port and ignores the error.
fn send(port: &dyn Port, command: &impl Command) {
    let mut out = String::new();
    // Writing ANSI into a `String` cannot fail.
    let _ = command.write_ansi(&mut out);
    let _ = port.write(out.as_bytes());
}

/// Builds the entry bytes: enter, erase, home, hide, and optional mouse capture.
fn entry_bytes(options: Options) -> String {
    let mut out = String::new();
    // Writing ANSI into a `String` cannot fail.
    let _ = crossterm::terminal::EnterAlternateScreen.write_ansi(&mut out);
    let _ = crossterm::terminal::Clear(crossterm::terminal::ClearType::All).write_ansi(&mut out);
    let _ = crossterm::cursor::MoveTo(0, 0).write_ansi(&mut out);
    let _ = crossterm::cursor::Hide.write_ansi(&mut out);
    if options.mouse_capture {
        let _ = crossterm::event::EnableMouseCapture.write_ansi(&mut out);
    }
    out
}

/// The real terminal: standard output and crossterm raw mode.
struct StdoutPort;

impl Port for StdoutPort {
    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let mut out = io::stdout().lock();
        out.write_all(bytes)?;
        out.flush()
    }

    fn enable_raw_mode(&self) -> io::Result<()> {
        crossterm::terminal::enable_raw_mode()
    }

    fn disable_raw_mode(&self) -> io::Result<()> {
        crossterm::terminal::disable_raw_mode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, PoisonError};

    /// The panic hook is process-global, so each test that enters holds this.
    static PANIC_HOOK_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Event {
        RawOn,
        RawOff,
        Write(Vec<u8>),
    }

    #[derive(Default)]
    struct FakePort {
        events: Arc<Mutex<Vec<Event>>>,
        fail_write_containing: Option<&'static [u8]>,
        fail_raw_off: bool,
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

    type Events = Arc<Mutex<Vec<Event>>>;

    fn fake(configure: impl FnOnce(&mut FakePort)) -> (Arc<FakePort>, Events) {
        let mut port = FakePort::default();
        configure(&mut port);
        let events = Arc::clone(&port.events);
        (Arc::new(port), events)
    }

    fn snapshot(events: &Events) -> Vec<Event> {
        events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn write(bytes: &str) -> Event {
        Event::Write(bytes.as_bytes().to_vec())
    }

    const ENTRY: &str = "\x1b[?1049h\x1b[2J\x1b[1;1H\x1b[?25l";

    #[test]
    fn default_entry_is_raw_mode_then_one_clearing_write() {
        let _lock = PANIC_HOOK_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (port, events) = fake(|_| {});
        let session = Session::enter_on(port, Options::default()).unwrap();
        assert_eq!(snapshot(&events), [Event::RawOn, write(ENTRY)]);
        std::mem::forget(session);
        let _ = std::panic::take_hook();
    }

    #[test]
    fn mouse_capture_joins_the_single_entry_write_after_the_hide() {
        let _lock = PANIC_HOOK_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (port, events) = fake(|_| {});
        let session = Session::enter_on(port, Options::default().mouse_capture(true)).unwrap();
        let mut command = String::new();
        crossterm::event::EnableMouseCapture
            .write_ansi(&mut command)
            .unwrap();
        assert_eq!(
            snapshot(&events),
            [Event::RawOn, write(&format!("{ENTRY}{command}"))]
        );
        std::mem::forget(session);
        let _ = std::panic::take_hook();
    }

    #[test]
    fn raw_mode_off_sends_no_raw_on() {
        let _lock = PANIC_HOOK_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (port, events) = fake(|_| {});
        let session = Session::enter_on(port, Options::default().raw_mode(false)).unwrap();
        assert_eq!(snapshot(&events), [write(ENTRY)]);
        std::mem::forget(session);
        let _ = std::panic::take_hook();
    }

    #[test]
    fn failed_entry_write_undoes_raw_mode_and_returns_the_error() {
        let _lock = PANIC_HOOK_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (port, events) = fake(|p| p.fail_write_containing = Some(b"1049h"));
        let result = Session::enter_on(port, Options::default());
        let error = result.err().expect("the entry must fail");
        assert_eq!(error.to_string(), "write failed");
        assert_eq!(
            snapshot(&events),
            [Event::RawOn, write(ENTRY), Event::RawOff]
        );
        let _ = std::panic::take_hook();
    }

    const DISABLE_MOUSE: &str = "\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l";
    const SHOW: &str = "\x1b[?25h";
    const LEAVE: &str = "\x1b[?1049l";

    #[test]
    fn drop_restores_mouse_cursor_screen_and_raw_mode_in_order() {
        let _lock = PANIC_HOOK_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (port, events) = fake(|_| {});
        let session = Session::enter_on(port, Options::default().mouse_capture(true)).unwrap();
        let entered = snapshot(&events).len();
        drop(session);
        assert_eq!(
            snapshot(&events)[entered..],
            [
                write(DISABLE_MOUSE),
                write(SHOW),
                write(LEAVE),
                Event::RawOff
            ]
        );
    }

    #[test]
    fn a_failed_show_cursor_write_does_not_skip_the_other_steps() {
        let _lock = PANIC_HOOK_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (port, events) = fake(|p| p.fail_write_containing = Some(b"25h"));
        let session = Session::enter_on(port, Options::default()).unwrap();
        let entered = snapshot(&events).len();
        drop(session);
        assert_eq!(
            snapshot(&events)[entered..],
            [write(SHOW), write(LEAVE), Event::RawOff]
        );
    }

    #[test]
    fn a_failed_raw_off_is_ignored_on_drop() {
        let _lock = PANIC_HOOK_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (port, events) = fake(|p| p.fail_raw_off = true);
        let session = Session::enter_on(port, Options::default()).unwrap();
        let entered = snapshot(&events).len();
        drop(session);
        assert_eq!(
            snapshot(&events)[entered..],
            [write(SHOW), write(LEAVE), Event::RawOff]
        );
    }

    #[test]
    fn drop_without_raw_mode_sends_no_raw_off() {
        let _lock = PANIC_HOOK_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (port, events) = fake(|_| {});
        let session = Session::enter_on(port, Options::default().raw_mode(false)).unwrap();
        let entered = snapshot(&events).len();
        drop(session);
        assert_eq!(snapshot(&events)[entered..], [write(SHOW), write(LEAVE)]);
    }
}
