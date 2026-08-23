//! Terminal width detection and monitoring.
//!
//! This module provides utilities for getting the current terminal width
//! and watching for terminal resize events.

use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

use crate::DEFAULT_TERMINAL_WIDTH;

/// Utilities for synchronous terminal width detection.
pub struct TerminalWidth;

impl TerminalWidth {
    /// Get the current terminal width.
    ///
    /// Returns the terminal width in columns if it can be detected,
    /// otherwise returns `None`.
    ///
    /// A terminal that carries no window is a terminal that cannot be detected.
    /// It answers the `TIOCGWINSZ` ioctl with zero columns and zero rows, and
    /// the ioctl succeeds. `termsize` gives `None` for that answer, and the
    /// documentation of that crate says why.
    #[must_use]
    pub fn get() -> Option<u16> {
        Self::columns_of(termsize::controlling_columns())
    }

    /// Get the current terminal width with a fallback.
    ///
    /// Returns the terminal width in columns if it can be detected,
    /// otherwise returns the provided fallback value.
    ///
    /// # Arguments
    ///
    /// * `fallback` - The value to return if terminal width cannot be detected.
    #[must_use]
    pub fn get_or(fallback: u16) -> u16 {
        Self::columns_or(termsize::controlling_columns(), fallback)
    }

    /// Get the current terminal width with the default fallback.
    ///
    /// Returns the terminal width in columns if it can be detected,
    /// otherwise returns [`DEFAULT_TERMINAL_WIDTH`] (80 columns).
    #[must_use]
    pub fn get_or_default() -> u16 {
        Self::get_or(DEFAULT_TERMINAL_WIDTH)
    }

    /// The width to report, from the answer that the terminal gave.
    ///
    /// The read of the terminal stands apart from this decision, so a test
    /// names the answer of a terminal without a terminal to name it with.
    /// [`get`](Self::get) reads the terminal and hands the answer here.
    ///
    /// `None` is a run that measured no terminal, and it stays `None`. A width
    /// of zero becomes `None` as well, because no character of a line prints
    /// into no column. A caller that gets `None` reaches the fallback it
    /// named, and a caller that gets a zero draws a progress bar into nothing.
    ///
    /// `termsize` already gives `None` for a width of zero, so no zero reaches
    /// this function through [`get`](Self::get). The rule stays here because
    /// this function is where `termbar` states what it reports, and the test of
    /// the rule is what keeps it true if the probe of [`get`](Self::get) ever
    /// changes.
    fn columns_of(answer: Option<u16>) -> Option<u16> {
        answer.filter(|columns| *columns > 0)
    }

    /// The width to report with a fallback, from the answer that the terminal
    /// gave.
    ///
    /// This is the decision of [`get_or`](Self::get_or), and it stands apart
    /// from the read of the terminal for the same reason that
    /// [`columns_of`](Self::columns_of) does. An answer that
    /// [`columns_of`](Self::columns_of) refuses gives the fallback.
    fn columns_or(answer: Option<u16>, fallback: u16) -> u16 {
        Self::columns_of(answer).unwrap_or(fallback)
    }
}

/// Watches for terminal width changes and notifies subscribers.
///
/// This struct provides an async mechanism for tracking terminal resize events
/// using a tokio watch channel. It can optionally spawn a SIGWINCH signal handler
/// on Unix systems.
///
/// # Usage
///
/// Use [`with_sigwinch_channel`](Self::with_sigwinch_channel) to create a watcher
/// with automatic resize handling and clean channel-based shutdown.
pub struct TerminalWidthWatcher {
    sender: watch::Sender<u16>,
    receiver: watch::Receiver<u16>,
}

impl TerminalWidthWatcher {
    /// Create a new terminal width watcher.
    ///
    /// Initializes the watcher with the current terminal width.
    /// The watcher does not automatically listen for resize events;
    /// use [`with_sigwinch_channel`](Self::with_sigwinch_channel) for automatic resize detection.
    ///
    /// The watcher reads the terminal through
    /// [`TerminalWidth::get_or_default`], both here and on every resize, so a
    /// terminal that reports no size gives [`DEFAULT_TERMINAL_WIDTH`] to every
    /// subscriber.
    #[must_use]
    pub fn new() -> Self {
        let initial_width = TerminalWidth::get_or_default();
        let (sender, receiver) = watch::channel(initial_width);
        Self { sender, receiver }
    }

    /// Create a new terminal width watcher with SIGWINCH handler using a shutdown channel.
    ///
    /// This is the recommended way to create a watcher with automatic resize handling.
    /// It uses a oneshot channel for clean shutdown instead of polling an `AtomicBool`.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - The watcher instance
    /// - A handle to the background task (await this during cleanup)
    /// - A shutdown sender (drop or send to trigger shutdown)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let (watcher, task, shutdown_tx) = TerminalWidthWatcher::with_sigwinch_channel();
    ///
    /// // Use the watcher...
    /// let width = watcher.current_width();
    ///
    /// // To shutdown:
    /// drop(shutdown_tx);  // or shutdown_tx.send(())
    /// task.await;
    /// ```
    #[must_use]
    pub fn with_sigwinch_channel() -> (Self, JoinHandle<()>, oneshot::Sender<()>) {
        let watcher = Self::new();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = watcher.spawn_sigwinch_handler_with_shutdown(shutdown_rx);
        (watcher, task, shutdown_tx)
    }

    /// Spawn a SIGWINCH signal handler task with a shutdown channel.
    ///
    /// This is the recommended way to spawn the handler. The task will exit
    /// when the shutdown channel is signaled (either by sending a value or
    /// by dropping the sender).
    ///
    /// On Unix systems, this listens for SIGWINCH signals (terminal resize)
    /// and updates the terminal width accordingly.
    ///
    /// On non-Unix systems, this returns a no-op task.
    ///
    /// # Arguments
    ///
    /// * `shutdown_rx` - A oneshot receiver that signals when to stop.
    #[must_use]
    pub fn spawn_sigwinch_handler_with_shutdown(
        &self,
        shutdown_rx: oneshot::Receiver<()>,
    ) -> JoinHandle<()> {
        #[cfg(unix)]
        {
            let sender = self.sender.clone();
            tokio::task::spawn(async move {
                use tokio::signal::unix::{signal, SignalKind};

                let mut sigwinch = match signal(SignalKind::window_change()) {
                    Ok(s) => s,
                    Err(_e) => {
                        // Non-critical: progress bar resize won't work,
                        // but crossterm Event::Resize may still work.
                        #[cfg(debug_assertions)]
                        eprintln!("Debug: SIGWINCH handler setup failed: {_e}");
                        return;
                    }
                };

                // Pin the shutdown receiver for use in select!
                tokio::pin!(shutdown_rx);

                loop {
                    tokio::select! {
                        _ = sigwinch.recv() => {
                            let new_width = TerminalWidth::get_or_default();
                            let _ = sender.send(new_width);
                        }
                        _ = &mut shutdown_rx => {
                            // Shutdown signal received (or sender dropped)
                            break;
                        }
                    }
                }
            })
        }

        #[cfg(not(unix))]
        {
            let _ = shutdown_rx;
            tokio::task::spawn(async {})
        }
    }

    /// Get a receiver for terminal width updates.
    ///
    /// Clone this receiver to get notified of terminal width changes.
    #[must_use]
    pub fn receiver(&self) -> watch::Receiver<u16> {
        self.receiver.clone()
    }

    /// Get the current terminal width from the watcher.
    ///
    /// Returns the most recently observed terminal width.
    #[must_use]
    pub fn current_width(&self) -> u16 {
        *self.receiver.borrow()
    }

    /// Get the sender for manual width updates.
    ///
    /// This is useful for integrating with other resize detection mechanisms
    /// such as crossterm's `Event::Resize`.
    #[must_use]
    pub fn sender(&self) -> &watch::Sender<u16> {
        &self.sender
    }
}

impl Default for TerminalWidthWatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The width of a window that a terminal really holds.
    const REAL: u16 = 120;

    /// A fallback that no terminal reports, so a test that reads it back knows
    /// where the number came from.
    const FALLBACK: u16 = 77;

    #[test]
    fn a_real_width_comes_through() {
        assert_eq!(
            TerminalWidth::columns_of(Some(REAL)),
            Some(REAL),
            "a terminal that reports a window keeps the width of it"
        );
        assert_eq!(
            TerminalWidth::columns_or(Some(REAL), FALLBACK),
            REAL,
            "a terminal that reports a window is what the caller draws in, and the fallback stays out of the way"
        );
    }

    #[test]
    fn a_run_that_measured_no_terminal_falls_back() {
        assert_eq!(
            TerminalWidth::columns_of(None),
            None,
            "a run that measured no terminal holds no width"
        );
        assert_eq!(
            TerminalWidth::columns_or(None, FALLBACK),
            FALLBACK,
            "a run that measured no terminal gets the fallback that the caller named"
        );
    }

    #[test]
    fn a_terminal_that_reports_no_columns_falls_back() {
        assert_eq!(
            TerminalWidth::columns_of(Some(0)),
            None,
            "a terminal that carries no window answers the TIOCGWINSZ ioctl with zero columns, and no character of a line prints into no column"
        );
        assert_eq!(
            TerminalWidth::columns_or(Some(0), FALLBACK),
            FALLBACK,
            "a zero must not defeat the fallback: a progress bar laid out at zero columns is what the user then reads"
        );
        assert_eq!(
            TerminalWidth::columns_or(Some(0), DEFAULT_TERMINAL_WIDTH),
            DEFAULT_TERMINAL_WIDTH,
            "get_or_default names DEFAULT_TERMINAL_WIDTH as its fallback, and a zero from the terminal must reach it"
        );
    }

    #[test]
    fn the_narrowest_window_is_still_a_window() {
        assert_eq!(
            TerminalWidth::columns_of(Some(1)),
            Some(1),
            "one column holds one character, so the rule stops at zero and no higher"
        );
    }

    #[test]
    fn test_terminal_width_get_or() {
        // Should return something reasonable (either detected width or fallback)
        let width = TerminalWidth::get_or(80);
        assert!(width > 0);
    }

    #[test]
    fn test_terminal_width_get_or_default() {
        let width = TerminalWidth::get_or_default();
        assert!(width > 0);
    }

    #[test]
    fn test_watcher_new() {
        let watcher = TerminalWidthWatcher::new();
        let width = watcher.current_width();
        assert!(width > 0);
    }

    #[test]
    fn test_watcher_sender_updates_receiver() {
        let watcher = TerminalWidthWatcher::new();
        let receiver = watcher.receiver();

        // Update via sender
        let _ = watcher.sender().send(120);

        // Receiver should see the update
        assert_eq!(*receiver.borrow(), 120);
        assert_eq!(watcher.current_width(), 120);
    }

    #[tokio::test]
    async fn test_shutdown_channel_exits_on_drop() {
        let (watcher, task, shutdown_tx) = TerminalWidthWatcher::with_sigwinch_channel();

        // Verify watcher works
        let _ = watcher.sender().send(100);
        assert_eq!(watcher.current_width(), 100);

        // Drop the shutdown sender to trigger shutdown
        drop(shutdown_tx);

        // Task should complete quickly (not hang)
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("Task should complete after shutdown signal")
            .expect("Task should not panic");
    }

    #[tokio::test]
    async fn test_shutdown_channel_exits_on_send() {
        let (watcher, task, shutdown_tx) = TerminalWidthWatcher::with_sigwinch_channel();

        // Verify watcher works
        let _ = watcher.sender().send(100);
        assert_eq!(watcher.current_width(), 100);

        // Send shutdown signal explicitly
        let _ = shutdown_tx.send(());

        // Task should complete quickly (not hang)
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("Task should complete after shutdown signal")
            .expect("Task should not panic");
    }
}
