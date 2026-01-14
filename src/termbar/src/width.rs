//! Terminal width detection and monitoring.
//!
//! This module provides utilities for getting the current terminal width
//! and watching for terminal resize events.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::DEFAULT_TERMINAL_WIDTH;

/// Utilities for synchronous terminal width detection.
pub struct TerminalWidth;

impl TerminalWidth {
    /// Get the current terminal width.
    ///
    /// Returns the terminal width in columns if it can be detected,
    /// otherwise returns `None`.
    #[must_use]
    pub fn get() -> Option<u16> {
        crossterm::terminal::size().map(|(w, _)| w).ok()
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
        Self::get().unwrap_or(fallback)
    }

    /// Get the current terminal width with the default fallback.
    ///
    /// Returns the terminal width in columns if it can be detected,
    /// otherwise returns [`DEFAULT_TERMINAL_WIDTH`] (80 columns).
    #[must_use]
    pub fn get_or_default() -> u16 {
        Self::get_or(DEFAULT_TERMINAL_WIDTH)
    }
}

/// Watches for terminal width changes and notifies subscribers.
///
/// This struct provides an async mechanism for tracking terminal resize events
/// using a tokio watch channel. It can optionally spawn a SIGWINCH signal handler
/// on Unix systems.
pub struct TerminalWidthWatcher {
    sender: watch::Sender<u16>,
    receiver: watch::Receiver<u16>,
}

impl TerminalWidthWatcher {
    /// Create a new terminal width watcher.
    ///
    /// Initializes the watcher with the current terminal width.
    /// The watcher does not automatically listen for resize events;
    /// use [`with_sigwinch`](Self::with_sigwinch) for automatic resize detection.
    #[must_use]
    pub fn new() -> Self {
        let initial_width = TerminalWidth::get_or_default();
        let (sender, receiver) = watch::channel(initial_width);
        Self { sender, receiver }
    }

    /// Create a new terminal width watcher with SIGWINCH handler (Unix only).
    ///
    /// This spawns a background task that listens for terminal resize signals
    /// and updates the width automatically. The task will exit when the `done`
    /// flag is set to `true`.
    ///
    /// # Arguments
    ///
    /// * `done` - A shared flag that signals when to stop watching for resize events.
    ///
    /// # Returns
    ///
    /// A tuple containing the watcher and a handle to the background task.
    /// The task should be awaited during cleanup.
    #[must_use]
    pub fn with_sigwinch(done: Arc<AtomicBool>) -> (Self, JoinHandle<()>) {
        let watcher = Self::new();
        let task = watcher.spawn_sigwinch_handler(done);
        (watcher, task)
    }

    /// Spawn a SIGWINCH signal handler task.
    ///
    /// On Unix systems, this listens for SIGWINCH signals (terminal resize)
    /// and updates the terminal width accordingly.
    ///
    /// On non-Unix systems, this returns a no-op task.
    ///
    /// # Arguments
    ///
    /// * `done` - A shared flag that signals when to stop watching for resize events.
    #[must_use]
    pub fn spawn_sigwinch_handler(&self, done: Arc<AtomicBool>) -> JoinHandle<()> {
        #[cfg(unix)]
        {
            let sender = self.sender.clone();
            tokio::task::spawn(async move {
                use tokio::signal::unix::{signal, SignalKind};

                let mut sigwinch = match signal(SignalKind::window_change()) {
                    Ok(s) => s,
                    Err(_) => {
                        // Non-critical: progress bar resize won't work,
                        // but crossterm Event::Resize may still work.
                        return;
                    }
                };

                loop {
                    tokio::select! {
                        _ = sigwinch.recv() => {
                            let new_width = TerminalWidth::get_or_default();
                            let _ = sender.send(new_width);
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                            if done.load(Ordering::SeqCst) {
                                break;
                            }
                        }
                    }
                }
            })
        }

        #[cfg(not(unix))]
        {
            let _ = done;
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
}
