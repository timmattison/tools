//! Terminal width detection and monitoring.
//!
//! This module provides utilities for getting the current terminal width
//! and watching for terminal resize events.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
///
/// # Shutdown Mechanism
///
/// The watcher provides two shutdown mechanisms:
/// - **Shutdown channel**: Use [`spawn_sigwinch_handler_with_shutdown`](Self::spawn_sigwinch_handler_with_shutdown)
///   for clean shutdown via a oneshot channel (recommended).
/// - **AtomicBool polling**: Use [`spawn_sigwinch_handler`](Self::spawn_sigwinch_handler)
///   for backward compatibility with existing code using `Arc<AtomicBool>`.
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

    /// Spawn a SIGWINCH signal handler task (legacy API).
    ///
    /// On Unix systems, this listens for SIGWINCH signals (terminal resize)
    /// and updates the terminal width accordingly.
    ///
    /// On non-Unix systems, this returns a no-op task.
    ///
    /// # Arguments
    ///
    /// * `done` - A shared flag that signals when to stop watching for resize events.
    ///
    /// # Note
    ///
    /// Consider using [`spawn_sigwinch_handler_with_shutdown`](Self::spawn_sigwinch_handler_with_shutdown)
    /// for cleaner shutdown semantics. This method polls the `done` flag every 100ms,
    /// while the shutdown channel version exits immediately when signaled.
    #[must_use]
    pub fn spawn_sigwinch_handler(&self, done: Arc<AtomicBool>) -> JoinHandle<()> {
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
