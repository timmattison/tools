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
//! use termbar::TerminalWidthWatcher;
//!
//! // Create watcher with automatic SIGWINCH handling
//! let (watcher, resize_task, shutdown_tx) = TerminalWidthWatcher::with_sigwinch_channel();
//!
//! // Get current width
//! let width = watcher.current_width();
//!
//! // Get receiver for watching changes
//! let receiver = watcher.receiver();
//!
//! // When done, signal the task to stop by dropping the shutdown sender
//! drop(shutdown_tx);
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
/// The string contains 10 characters used by indicatif for progress rendering:
/// - 8 partial block characters (`█▉▊▋▌▍▎▏`) for sub-character progress levels
/// - 2 space characters for the empty/background portion of the bar
pub const PROGRESS_CHARS: &str = "█▉▊▋▌▍▎▏  ";

/// Maximum length for a file extension to be recognized as such.
///
/// Extensions longer than this are treated as part of the basename.
/// This prevents unusual filenames like `file.verylongextensionname` from
/// being split incorrectly during truncation.
const MAX_EXTENSION_LEN: usize = 10;

/// Ellipsis used when truncating filenames with an extension.
///
/// We use `..` (2 dots) so that when combined with the extension's dot,
/// the result is 3 visible dots: `filename...ext` (cleaner than 4 dots).
const ELLIPSIS_WITH_EXT: &str = "..";

/// Width in terminal columns of [`ELLIPSIS_WITH_EXT`].
const ELLIPSIS_WITH_EXT_WIDTH: usize = 2;

/// Ellipsis used when truncating filenames without an extension.
///
/// We use `...` (3 dots) for standard truncation appearance.
const ELLIPSIS_NO_EXT: &str = "...";

/// Width in terminal columns of [`ELLIPSIS_NO_EXT`].
const ELLIPSIS_NO_EXT_WIDTH: usize = 3;

/// Minimum width required for meaningful truncation with an indicator.
///
/// Below this width, truncation returns raw characters without any ellipsis
/// indicator because there's not enough space for both content and indicator.
/// This is 4 characters to allow at least: `X...` (1 char + 3 dot ellipsis).
const MIN_TRUNCATION_WIDTH: usize = 4;

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

/// Calculate the maximum filename display width that allows the progress bar to fit.
///
/// This function determines how wide a filename can be while still leaving room for:
/// - The minimum progress bar width ([`MIN_BAR_WIDTH`] = 10 columns)
/// - The fixed overhead (spinner, stats, brackets, etc.)
///
/// # Arguments
///
/// * `terminal_width` - The current terminal width in columns.
/// * `base_overhead` - Fixed overhead excluding filename (e.g., 60 for copy style).
///
/// # Returns
///
/// Maximum filename display width in columns.
///
/// # Example
///
/// ```
/// use termbar::calculate_max_filename_width;
///
/// // Terminal 100, overhead 60, min bar 10 -> max filename = 30
/// assert_eq!(calculate_max_filename_width(100, 60), 30);
///
/// // Narrow terminal: 80 - 60 - 10 = 10 max filename
/// assert_eq!(calculate_max_filename_width(80, 60), 10);
/// ```
#[must_use]
pub fn calculate_max_filename_width(terminal_width: u16, base_overhead: u16) -> u16 {
    // terminal_width = base_overhead + filename_width + bar_width
    // We want: bar_width >= MIN_BAR_WIDTH
    // So: filename_width <= terminal_width - base_overhead - MIN_BAR_WIDTH
    terminal_width
        .saturating_sub(base_overhead)
        .saturating_sub(MIN_BAR_WIDTH)
}

/// Take characters from the start of a string until reaching max display width.
///
/// Uses unicode display width for accurate terminal column counting.
///
/// # Arguments
///
/// * `s` - The string to take characters from.
/// * `max_width` - Maximum display width in terminal columns.
///
/// # Returns
///
/// A string containing characters from the start of `s` that fit within `max_width`.
fn take_chars_by_width(s: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthChar;

    let mut result = String::new();
    let mut width = 0;

    for ch in s.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width > max_width {
            break;
        }
        result.push(ch);
        width += ch_width;
    }

    result
}

/// Split a filename into basename and extension.
///
/// Returns `(basename, Some(extension))` or `(basename, None)`.
/// Handles hidden files (starting with '.') correctly.
///
/// # Arguments
///
/// * `filename` - The filename to split.
///
/// # Returns
///
/// A tuple of (basename, optional extension without the dot).
///
/// # Examples
///
/// - `"file.txt"` -> `("file", Some("txt"))`
/// - `"archive.tar.gz"` -> `("archive.tar", Some("gz"))`
/// - `".bashrc"` -> `(".bashrc", None)`
/// - `".hidden.txt"` -> `(".hidden", Some("txt"))`
fn split_filename_extension(filename: &str) -> (&str, Option<&str>) {
    // Handle hidden files: ".bashrc" -> basename=".bashrc", ext=None
    // Handle ".hidden.txt" -> basename=".hidden", ext=Some("txt")

    let search_start = if filename.starts_with('.') { 1 } else { 0 };

    if let Some(dot_pos) = filename[search_start..].rfind('.') {
        let actual_pos = search_start + dot_pos;
        let ext = &filename[actual_pos + 1..];
        // Only treat as extension if it's non-empty and reasonable length.
        // Use chars().count() for proper unicode handling (not byte length).
        if !ext.is_empty() && ext.chars().count() <= MAX_EXTENSION_LEN {
            return (&filename[..actual_pos], Some(ext));
        }
    }

    (filename, None)
}

/// Truncate a filename to fit within a maximum display width while preserving the extension.
///
/// When truncation is needed, the function produces output in one of two formats:
/// - With extension: `beginning...ext` (e.g., `"American.Psycho.2000.UNCUT...mkv"`)
/// - Without extension: `beginning...` (e.g., `"Makefile_with_very_long..."`)
///
/// The `..` ellipsis is used when an extension is present so that combined with
/// the extension's leading dot, the result shows 3 dots total for a clean appearance.
///
/// # Arguments
///
/// * `filename` - The filename to truncate (just the filename, not the full path).
/// * `max_width` - Maximum display width in terminal columns.
///
/// # Returns
///
/// The truncated filename, or the original if it fits within `max_width`.
///
/// # Algorithm
///
/// 1. If the filename fits within `max_width`, return it unchanged
/// 2. Extract the extension (last `.xxx` portion, if present)
/// 3. Calculate space needed for ellipsis and extension
/// 4. Take as much of the beginning as will fit
/// 5. Return `beginning...extension` (with extension) or `beginning...` (without)
///
/// # Edge Cases
///
/// - Files with no extension: truncate to `beginning...`
/// - Files with very long extensions: truncate the extension if needed
/// - Files starting with `.` (hidden files): treat the part after first `.` as basename
/// - Unicode filenames: uses display width, not byte length
///
/// # Example
///
/// ```
/// use termbar::truncate_filename;
///
/// // Long filename gets truncated (3 dots total: ".." + "." from extension)
/// let truncated = truncate_filename(
///     "American.Psycho.2000.UNCUT.2160p.BluRay.REMUX.HEVC.DTS-HD.MA.TrueHD.7.1.Atmos-FGT.mkv",
///     30
/// );
/// assert!(truncated.ends_with(".mkv"));
/// assert!(truncated.contains("..."));  // ".." + ".mkv" appears as "...mkv"
///
/// // Short filename unchanged
/// assert_eq!(truncate_filename("file.txt", 30), "file.txt");
/// ```
#[must_use]
pub fn truncate_filename(filename: &str, max_width: u16) -> String {
    use unicode_width::UnicodeWidthStr;

    let max_width_usize = usize::from(max_width);
    let current_width = filename.width();

    // If it already fits, return unchanged
    if current_width <= max_width_usize {
        return filename.to_string();
    }

    // Below MIN_TRUNCATION_WIDTH, we can't fit meaningful content + ellipsis,
    // so we just return the raw prefix without any truncation indicator.
    // This is an intentional design choice for extreme edge cases.
    if max_width_usize < MIN_TRUNCATION_WIDTH {
        return take_chars_by_width(filename, max_width_usize);
    }

    // Find extension - look for last '.' that isn't at position 0
    // Handle hidden files like ".bashrc" correctly
    let (basename, extension) = split_filename_extension(filename);

    if let Some(ext) = extension {
        let ext_width = ext.width();
        let dot_ext = format!(".{}", ext);
        let dot_ext_width = ext_width + 1; // +1 for the dot

        // Check if we have room for: something + .. + .ext
        // Minimum: 1 char + .. + .ext
        // Using ELLIPSIS_WITH_EXT ("..") so result is "name...ext" (3 dots total)
        if max_width_usize >= 1 + ELLIPSIS_WITH_EXT_WIDTH + dot_ext_width {
            let basename_budget = max_width_usize - ELLIPSIS_WITH_EXT_WIDTH - dot_ext_width;
            let truncated_basename = take_chars_by_width(basename, basename_budget);
            return format!("{}{}{}", truncated_basename, ELLIPSIS_WITH_EXT, dot_ext);
        }

        // Extension too long - truncate extension too
        // Format: basename_part..ext_part (still using 2-dot ellipsis)
        // Give 1/3 to basename, 2/3 to extension since we prioritize extension visibility
        let remaining = max_width_usize.saturating_sub(ELLIPSIS_WITH_EXT_WIDTH);
        let basename_budget = remaining / 3;
        let ext_budget = remaining - basename_budget;

        let truncated_basename = take_chars_by_width(basename, basename_budget);
        let truncated_ext = take_chars_by_width(&dot_ext, ext_budget);
        return format!("{}{}{}", truncated_basename, ELLIPSIS_WITH_EXT, truncated_ext);
    }

    // No extension - just truncate with ellipsis at end
    let basename_budget = max_width_usize.saturating_sub(ELLIPSIS_NO_EXT_WIDTH);
    let truncated = take_chars_by_width(filename, basename_budget);
    format!("{}{}", truncated, ELLIPSIS_NO_EXT)
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
        // Use const blocks for compile-time constant validation
        const _: () = assert!(MIN_BAR_WIDTH < MAX_BAR_WIDTH);
        const _: () = assert!(DEFAULT_TERMINAL_WIDTH > MIN_BAR_WIDTH);
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

    // Tests for calculate_max_filename_width
    #[test]
    fn test_calculate_max_filename_width_normal() {
        // Terminal 100, overhead 60, min bar 10 -> max filename = 30
        assert_eq!(calculate_max_filename_width(100, 60), 30);
    }

    #[test]
    fn test_calculate_max_filename_width_narrow() {
        // Narrow terminal: 80 - 60 - 10 = 10 max filename
        assert_eq!(calculate_max_filename_width(80, 60), 10);
    }

    #[test]
    fn test_calculate_max_filename_width_very_narrow() {
        // Very narrow terminal: 70 - 60 - 10 = 0
        assert_eq!(calculate_max_filename_width(70, 60), 0);
    }

    #[test]
    fn test_calculate_max_filename_width_wide() {
        // Wide terminal: 200 - 60 - 10 = 130
        assert_eq!(calculate_max_filename_width(200, 60), 130);
    }

    // Tests for split_filename_extension
    #[test]
    fn test_split_filename_extension_normal() {
        assert_eq!(split_filename_extension("file.txt"), ("file", Some("txt")));
        assert_eq!(split_filename_extension("document.pdf"), ("document", Some("pdf")));
    }

    #[test]
    fn test_split_filename_extension_multiple_dots() {
        assert_eq!(
            split_filename_extension("archive.tar.gz"),
            ("archive.tar", Some("gz"))
        );
    }

    #[test]
    fn test_split_filename_extension_hidden_file() {
        // Hidden file with no real extension
        assert_eq!(split_filename_extension(".bashrc"), (".bashrc", None));
    }

    #[test]
    fn test_split_filename_extension_hidden_with_ext() {
        // Hidden file with extension
        assert_eq!(
            split_filename_extension(".hidden.txt"),
            (".hidden", Some("txt"))
        );
    }

    #[test]
    fn test_split_filename_extension_no_extension() {
        assert_eq!(split_filename_extension("Makefile"), ("Makefile", None));
        assert_eq!(split_filename_extension("README"), ("README", None));
    }

    #[test]
    fn test_split_filename_extension_very_long_extension() {
        // Extensions > 10 chars are not treated as extensions
        assert_eq!(
            split_filename_extension("file.verylongextensionname"),
            ("file.verylongextensionname", None)
        );
    }

    // Tests for truncate_filename
    #[test]
    fn test_truncate_filename_short_unchanged() {
        assert_eq!(truncate_filename("file.txt", 30), "file.txt");
        assert_eq!(truncate_filename("a.b", 10), "a.b");
    }

    #[test]
    fn test_truncate_filename_empty() {
        assert_eq!(truncate_filename("", 10), "");
    }

    #[test]
    fn test_truncate_filename_long_with_extension() {
        let long = "American.Psycho.2000.UNCUT.2160p.BluRay.REMUX.HEVC.DTS-HD.MA.TrueHD.7.1.Atmos-FGT.mkv";
        let result = truncate_filename(long, 30);

        // Should end with .mkv
        assert!(
            result.ends_with(".mkv"),
            "Should preserve extension: {}",
            result
        );
        // Should contain ellipsis
        assert!(result.contains("..."), "Should contain ellipsis: {}", result);
        // Should fit within max width
        assert!(
            str_display_width_as_u16(&result) <= 30,
            "Should fit: {}",
            result
        );
        // Should start with beginning of original
        assert!(
            result.starts_with("American"),
            "Should start with beginning: {}",
            result
        );
    }

    #[test]
    fn test_truncate_filename_no_extension() {
        let result = truncate_filename("Makefile_with_very_long_name_here", 15);
        assert!(
            result.ends_with("..."),
            "Should end with ellipsis: {}",
            result
        );
        assert!(
            str_display_width_as_u16(&result) <= 15,
            "Should fit: {}",
            result
        );
    }

    #[test]
    fn test_truncate_filename_hidden_file_fits() {
        // Hidden file that fits
        assert_eq!(truncate_filename(".bashrc", 20), ".bashrc");
    }

    #[test]
    fn test_truncate_filename_hidden_file_with_extension() {
        // Hidden file with extension
        let result = truncate_filename(".very_long_hidden_config.json", 20);
        assert!(
            result.ends_with(".json"),
            "Should preserve extension: {}",
            result
        );
        assert!(
            str_display_width_as_u16(&result) <= 20,
            "Should fit: {}",
            result
        );
    }

    #[test]
    fn test_truncate_filename_minimum_width() {
        let result = truncate_filename("longfilename.txt", 4);
        assert!(
            str_display_width_as_u16(&result) <= 4,
            "Should fit in 4: {}",
            result
        );
    }

    #[test]
    fn test_truncate_filename_unicode_cjk() {
        // CJK characters (2 display columns each)
        let result = truncate_filename("文件名称很长的文档.txt", 15);
        assert!(
            str_display_width_as_u16(&result) <= 15,
            "Should fit: {}",
            result
        );
        assert!(
            result.ends_with(".txt"),
            "Should preserve extension: {}",
            result
        );
    }

    #[test]
    fn test_truncate_filename_unicode_emoji() {
        // Emoji filename
        let result = truncate_filename("my_cool_emoji_file_🎉🎊🎁.png", 20);
        assert!(
            str_display_width_as_u16(&result) <= 20,
            "Should fit: {}",
            result
        );
    }

    #[test]
    fn test_truncate_filename_exact_fit() {
        let filename = "exactly18chars.txt"; // 18 chars
        assert_eq!(truncate_filename(filename, 18), filename);
    }

    #[test]
    fn test_truncate_filename_very_small_max() {
        // max_width < 4 should just take first chars
        let result = truncate_filename("longfilename.txt", 3);
        assert_eq!(str_display_width_as_u16(&result), 3);
        assert_eq!(result, "lon");
    }

    #[test]
    fn test_truncate_filename_zero_width() {
        // Edge case: max_width = 0 should return empty string
        assert_eq!(truncate_filename("file.txt", 0), "");
        assert_eq!(truncate_filename("", 0), "");
        assert_eq!(truncate_filename("longfilename.txt", 0), "");
    }

    #[test]
    fn test_truncate_filename_one_width() {
        // Edge case: max_width = 1 should return first char
        let result = truncate_filename("longfilename.txt", 1);
        assert_eq!(result, "l");
        assert_eq!(str_display_width_as_u16(&result), 1);
    }

    #[test]
    fn test_progress_bar_fits_with_truncation() {
        // Simulate the actual use case: very long filename at terminal width 80
        let long_filename = "American.Psycho.2000.UNCUT.2160p.BluRay.REMUX.HEVC.DTS-HD.MA.TrueHD.7.1.Atmos-FGT.mkv";
        let terminal_width: u16 = 80;
        let base_overhead: u16 = 60;

        let max_filename_width = calculate_max_filename_width(terminal_width, base_overhead);
        let truncated = truncate_filename(long_filename, max_filename_width);
        let filename_width = str_display_width_as_u16(&truncated);

        // Total should fit: base_overhead + filename + MIN_BAR_WIDTH <= terminal_width
        let total = base_overhead + filename_width + MIN_BAR_WIDTH;
        assert!(
            total <= terminal_width,
            "Total {} should fit in terminal {}: overhead={}, filename={}, bar={}",
            total,
            terminal_width,
            base_overhead,
            filename_width,
            MIN_BAR_WIDTH
        );
    }

    // ========================================================================
    // Tests for MIN_TRUNCATION_WIDTH behavior
    // ========================================================================

    #[test]
    fn test_min_truncation_width_constant() {
        // Verify the constant is used correctly
        assert_eq!(MIN_TRUNCATION_WIDTH, 4);
    }

    #[test]
    fn test_truncation_at_boundary() {
        // At exactly MIN_TRUNCATION_WIDTH, we should get ellipsis
        let result = truncate_filename("longfilename.txt", 4);
        // Should be able to fit at least one char + ellipsis (3 dots) = 4 chars
        // But with extension, we need more room, so it falls back to no-extension style
        assert!(
            str_display_width_as_u16(&result) <= 4,
            "Result '{}' should fit in 4 columns",
            result
        );
    }

    #[test]
    fn test_below_min_truncation_width_no_ellipsis() {
        // Below MIN_TRUNCATION_WIDTH, we just get raw chars with no indicator
        let result = truncate_filename("longfilename.txt", 3);
        assert_eq!(result, "lon");
        assert!(!result.contains('.'), "No ellipsis below MIN_TRUNCATION_WIDTH");
    }

    // ========================================================================
    // Tests for extension truncation split behavior (1/3 basename, 2/3 ext)
    // ========================================================================

    #[test]
    fn test_long_extension_truncation_favors_extension() {
        // When both basename and extension must be truncated,
        // extension should get 2/3 of the budget
        let result = truncate_filename("ab.longerext", 10);
        // max_width=10, ellipsis=2, remaining=8
        // basename_budget=8/3=2, ext_budget=8-2=6
        // truncated_basename from "ab" = "ab" (fits in 2)
        // truncated_ext from ".longerext" with budget 6 = ".longe"
        // Result: "ab" + ".." + ".longe" = "ab...longe" but wait, that's 10 chars
        // Actually let me trace through again:
        // remaining = 10 - 2 = 8
        // basename_budget = 8 / 3 = 2
        // ext_budget = 8 - 2 = 6
        // truncated_basename = take_chars_by_width("ab", 2) = "ab"
        // truncated_ext = take_chars_by_width(".longerext", 6) = ".longe"
        // result = "ab" + ".." + ".longe" = "ab...longe" which is 10 chars

        assert!(
            str_display_width_as_u16(&result) <= 10,
            "Result '{}' should fit in 10 columns",
            result
        );
        // Extension portion should be longer than basename portion
        // (extension gets 2/3 of budget)
    }

    #[test]
    fn test_extension_truncation_preserves_dot() {
        // When truncating extension, the leading dot should be preserved

        // This has a 19-char extension which exceeds MAX_EXTENSION_LEN (10),
        // so it's treated as no extension - should truncate with "..." at end
        let result = truncate_filename("a.verylongextension", 8);
        assert!(
            result.ends_with("..."),
            "Very long extension treated as no extension: '{}'",
            result
        );

        // Try with a valid long extension
        let result2 = truncate_filename("basename.extension", 10);
        // extension = "extension" (9 chars, <= 10, valid)
        // dot_ext = ".extension" (10 chars)
        // Need 1 + 2 + 10 = 13 for full, but only have 10
        // So we truncate: remaining = 10 - 2 = 8
        // basename_budget = 2, ext_budget = 6
        // truncated_ext from ".extension" = ".exten"
        assert!(
            str_display_width_as_u16(&result2) <= 10,
            "Result '{}' should fit",
            result2
        );
    }

    // ========================================================================
    // Tests for unicode extension handling (chars().count() vs len())
    // ========================================================================

    #[test]
    fn test_unicode_extension_char_count() {
        // Test that extension length uses char count, not byte count
        // "日本語" is 3 characters but 9 bytes
        // As an extension, it should be accepted (3 <= MAX_EXTENSION_LEN)

        let (basename, ext) = split_filename_extension("file.日本語");
        assert_eq!(basename, "file");
        assert_eq!(ext, Some("日本語"));
    }

    #[test]
    fn test_unicode_extension_too_long_by_chars() {
        // Extension with 11 unicode characters (> MAX_EXTENSION_LEN)
        // Even though it might have fewer bytes than a long ASCII extension,
        // we count characters
        let long_ext = "あいうえおかきくけこさ"; // 11 hiragana chars
        let filename = format!("file.{}", long_ext);

        let (basename, ext) = split_filename_extension(&filename);
        // 11 chars > 10, so treated as no extension
        assert_eq!(basename, filename.as_str());
        assert_eq!(ext, None);
    }

    #[test]
    fn test_unicode_extension_at_max_length() {
        // Extension with exactly MAX_EXTENSION_LEN (10) unicode characters
        let exactly_ten = "あいうえおかきくけこ"; // 10 hiragana chars
        assert_eq!(exactly_ten.chars().count(), 10);

        let filename = format!("file.{}", exactly_ten);
        let (basename, ext) = split_filename_extension(&filename);

        assert_eq!(basename, "file");
        assert_eq!(ext, Some(exactly_ten));
    }
}
