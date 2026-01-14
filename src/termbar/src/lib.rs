//! Terminal progress bar library with resize support.
//!
//! This crate provides utilities for creating progress bars that automatically
//! adapt to terminal width changes. It includes:
//!
//! - **Terminal width detection**: Synchronous and async utilities for getting
//!   the current terminal width and watching for resize events.
//! - **Progress style builders**: Pre-configured styles for common operations
//!   like file copying, verification, hashing, and batch operations.
//! - **Dynamic resizing**: Support for updating progress bar styles when the
//!   terminal is resized.
//!
//! # Example
//!
//! ```rust,ignore
//! use termbar::{ProgressStyleBuilder, TerminalWidth};
//!
//! // Get terminal width
//! let width = TerminalWidth::get_or_default();
//!
//! // Create a progress style
//! let style = ProgressStyleBuilder::copy("myfile.txt").build(width)?;
//! ```
//!
//! # Async resize watching
//!
//! ```rust,ignore
//! use std::sync::atomic::{AtomicBool, Ordering};
//! use std::sync::Arc;
//! use termbar::TerminalWidthWatcher;
//!
//! let done = Arc::new(AtomicBool::new(false));
//! let (watcher, resize_task) = TerminalWidthWatcher::with_sigwinch(done.clone());
//!
//! // Get current width
//! let width = watcher.current_width();
//!
//! // Get receiver for watching changes
//! let receiver = watcher.receiver();
//!
//! // When done, signal the task to stop
//! done.store(true, Ordering::SeqCst);
//! resize_task.await;
//! ```

mod error;
mod style;
mod width;

pub use error::{Result, TermbarError};
pub use style::ProgressStyleBuilder;
pub use width::{TerminalWidth, TerminalWidthWatcher};

/// Default terminal width when detection fails (80 columns).
pub const DEFAULT_TERMINAL_WIDTH: u16 = 80;

/// Minimum progress bar width in characters.
pub const MIN_BAR_WIDTH: u16 = 10;

/// Maximum progress bar width in characters.
pub const MAX_BAR_WIDTH: u16 = 100;

/// Default progress characters for smooth sub-character progress visualization.
///
/// These characters provide 8 levels of progress within each character cell:
/// `█▉▊▋▌▍▎▏  `
pub const PROGRESS_CHARS: &str = "█▉▊▋▌▍▎▏  ";

/// Escape braces in a string for use in indicatif templates.
///
/// Indicatif uses `{placeholder}` syntax, so literal braces must be doubled
/// to be displayed as actual brace characters.
///
/// # Arguments
///
/// * `s` - The string to escape.
///
/// # Returns
///
/// The string with `{` replaced by `{{` and `}` replaced by `}}`.
///
/// # Example
///
/// ```
/// use termbar::escape_template_braces;
///
/// let escaped = escape_template_braces("file{1}.txt");
/// assert_eq!(escaped, "file{{1}}.txt");
/// ```
#[must_use]
pub fn escape_template_braces(s: &str) -> String {
    s.replace('{', "{{").replace('}', "}}")
}

/// Calculate the progress bar width based on terminal width and fixed element overhead.
///
/// The bar width is calculated by subtracting the overhead from the terminal width,
/// then clamping to a reasonable range ([`MIN_BAR_WIDTH`] to [`MAX_BAR_WIDTH`]).
///
/// # Arguments
///
/// * `terminal_width` - The current terminal width in columns.
/// * `fixed_overhead` - The number of columns used by fixed elements (spinner, text, etc.).
///
/// # Returns
///
/// The calculated bar width, clamped to the valid range.
///
/// # Example
///
/// ```
/// use termbar::calculate_bar_width;
///
/// // Terminal width 80, overhead 60 -> bar width 20
/// assert_eq!(calculate_bar_width(80, 60), 20);
///
/// // Terminal width 40, overhead 60 -> bar width clamped to minimum (10)
/// assert_eq!(calculate_bar_width(40, 60), 10);
///
/// // Terminal width 200, overhead 60 -> bar width clamped to maximum (100)
/// assert_eq!(calculate_bar_width(200, 60), 100);
/// ```
#[must_use]
pub fn calculate_bar_width(terminal_width: u16, fixed_overhead: u16) -> u16 {
    let available = terminal_width.saturating_sub(fixed_overhead);
    available.clamp(MIN_BAR_WIDTH, MAX_BAR_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_template_braces() {
        assert_eq!(escape_template_braces("hello"), "hello");
        assert_eq!(escape_template_braces("{test}"), "{{test}}");
        assert_eq!(escape_template_braces("file{1}.txt"), "file{{1}}.txt");
        assert_eq!(escape_template_braces("{}"), "{{}}");
        // Empty string should return empty string
        assert_eq!(escape_template_braces(""), "");
    }

    #[test]
    fn test_calculate_bar_width_normal() {
        // Normal case: terminal 80, overhead 60 -> 20
        assert_eq!(calculate_bar_width(80, 60), 20);
    }

    #[test]
    fn test_calculate_bar_width_min_clamp() {
        // Overhead > terminal -> clamp to MIN_BAR_WIDTH
        assert_eq!(calculate_bar_width(40, 60), MIN_BAR_WIDTH);
    }

    #[test]
    fn test_calculate_bar_width_max_clamp() {
        // Wide terminal -> clamp to MAX_BAR_WIDTH
        assert_eq!(calculate_bar_width(200, 60), MAX_BAR_WIDTH);
    }

    #[test]
    fn test_calculate_bar_width_exact_min() {
        // Exactly at minimum
        assert_eq!(calculate_bar_width(70, 60), MIN_BAR_WIDTH);
    }

    #[test]
    fn test_calculate_bar_width_exact_max() {
        // Exactly at maximum
        assert_eq!(calculate_bar_width(160, 60), MAX_BAR_WIDTH);
    }

    #[test]
    fn test_constants() {
        assert!(MIN_BAR_WIDTH < MAX_BAR_WIDTH);
        assert!(DEFAULT_TERMINAL_WIDTH > MIN_BAR_WIDTH);
        assert!(!PROGRESS_CHARS.is_empty());
    }
}
