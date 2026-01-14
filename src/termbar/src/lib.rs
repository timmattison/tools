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

/// Calculate a string's terminal display width as u16, capping at [`u16::MAX`].
///
/// This function uses the `unicode-width` crate to calculate the actual number
/// of terminal columns a string will occupy when displayed. This correctly handles:
/// - Multi-byte UTF-8 characters (e.g., emoji 🎉, CJK characters 中文)
/// - Zero-width characters (e.g., combining marks)
/// - Wide characters that occupy 2 terminal columns (e.g., CJK ideographs)
///
/// # Arguments
///
/// * `s` - The string to measure.
///
/// # Returns
///
/// The string's terminal display width as u16, or [`u16::MAX`] if the width exceeds u16 range.
///
/// # Example
///
/// ```
/// use termbar::str_display_width_as_u16;
///
/// // ASCII: 1 byte = 1 column
/// assert_eq!(str_display_width_as_u16("hello"), 5);
/// assert_eq!(str_display_width_as_u16(""), 0);
///
/// // Emoji: typically 2 columns wide
/// assert_eq!(str_display_width_as_u16("🎉"), 2);
///
/// // CJK: 2 columns per character
/// assert_eq!(str_display_width_as_u16("中"), 2);
/// ```
#[must_use]
pub fn str_display_width_as_u16(s: &str) -> u16 {
    use unicode_width::UnicodeWidthStr;
    u16::try_from(s.width()).unwrap_or(u16::MAX)
}

/// Convert a string's byte length to u16, capping at [`u16::MAX`].
///
/// **Deprecated**: Use [`str_display_width_as_u16`] instead for accurate terminal
/// display width calculations. This function is kept for cases where byte length
/// is specifically needed.
///
/// This function returns the **byte length** of the string, not the display width.
/// For ASCII strings, these are equivalent. However, for strings containing
/// multi-byte UTF-8 characters, the byte length may differ from the display width.
///
/// # Arguments
///
/// * `s` - The string to measure.
///
/// # Returns
///
/// The string byte length as u16, or [`u16::MAX`] if the length exceeds u16 range.
#[must_use]
#[deprecated(since = "0.1.1", note = "Use str_display_width_as_u16 for accurate terminal display width")]
pub fn str_len_as_u16(s: &str) -> u16 {
    u16::try_from(s.len()).unwrap_or(u16::MAX)
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

    #[test]
    fn test_str_display_width_as_u16() {
        // ASCII: 1 byte = 1 column
        assert_eq!(str_display_width_as_u16(""), 0);
        assert_eq!(str_display_width_as_u16("hello"), 5);
        assert_eq!(str_display_width_as_u16("file{1}.txt"), 11);
    }

    #[test]
    fn test_str_display_width_as_u16_unicode() {
        // Emoji: 4 bytes but displays as 2 columns
        assert_eq!(str_display_width_as_u16("🎉"), 2);
        // CJK: 3 bytes per character but each displays as 2 columns
        assert_eq!(str_display_width_as_u16("中"), 2);
        // Mixed: "file" (4 cols) + 🎉 (2 cols) + ".txt" (4 cols) = 10 columns
        assert_eq!(str_display_width_as_u16("file🎉.txt"), 10);
    }

    #[test]
    #[allow(deprecated)]
    fn test_str_len_as_u16_deprecated() {
        // Test byte length (deprecated function)
        assert_eq!(str_len_as_u16("hello"), 5);
        // Emoji: 4 bytes in UTF-8
        assert_eq!(str_len_as_u16("🎉"), 4);
        // "file" (4 bytes) + 🎉 (4 bytes) + ".txt" (4 bytes) = 12 bytes
        assert_eq!(str_len_as_u16("file🎉.txt"), 12);
    }
}
