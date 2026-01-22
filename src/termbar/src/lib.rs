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

// Compile-time assertions for constant relationships.
// These ensure the public constants maintain valid relationships regardless of
// whether tests are run. Moving these out of tests ensures they're always checked.
const _: () = assert!(
    MIN_BAR_WIDTH < MAX_BAR_WIDTH,
    "MIN_BAR_WIDTH must be less than MAX_BAR_WIDTH"
);
const _: () = assert!(
    DEFAULT_TERMINAL_WIDTH > MIN_BAR_WIDTH,
    "DEFAULT_TERMINAL_WIDTH must be greater than MIN_BAR_WIDTH"
);

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
///
/// # Invariant
///
/// This must equal `ELLIPSIS_WITH_EXT.len()`. Since our ellipsis is ASCII,
/// byte length equals character count equals display width.
/// The compile-time assertion below enforces this invariant.
const ELLIPSIS_WITH_EXT_WIDTH: usize = ELLIPSIS_WITH_EXT.len();

/// Ellipsis used when truncating filenames without an extension.
///
/// We use `...` (3 dots) for standard truncation appearance.
const ELLIPSIS_NO_EXT: &str = "...";

/// Width in terminal columns of [`ELLIPSIS_NO_EXT`].
///
/// # Invariant
///
/// This must equal `ELLIPSIS_NO_EXT.len()`. Since our ellipsis is ASCII,
/// byte length equals character count equals display width.
/// The compile-time assertion below enforces this invariant.
const ELLIPSIS_NO_EXT_WIDTH: usize = ELLIPSIS_NO_EXT.len();

// Compile-time assertions to ensure ellipsis strings remain ASCII and
// that the width constants stay in sync with the string definitions.
// If someone changes these to Unicode characters (e.g., '…'), these assertions
// will fail, prompting them to update the width calculation logic.
//
// We verify two things for each ellipsis:
// 1. The string has the expected length (catches changes to the string)
// 2. The _WIDTH constant equals the string length (catches constant drift)
const _: () = assert!(
    ELLIPSIS_WITH_EXT.len() == 2,
    "ELLIPSIS_WITH_EXT must be exactly 2 ASCII characters"
);
const _: () = assert!(
    ELLIPSIS_WITH_EXT_WIDTH == ELLIPSIS_WITH_EXT.len(),
    "ELLIPSIS_WITH_EXT_WIDTH must equal ELLIPSIS_WITH_EXT.len()"
);
const _: () = assert!(
    ELLIPSIS_NO_EXT.len() == 3,
    "ELLIPSIS_NO_EXT must be exactly 3 ASCII characters"
);
const _: () = assert!(
    ELLIPSIS_NO_EXT_WIDTH == ELLIPSIS_NO_EXT.len(),
    "ELLIPSIS_NO_EXT_WIDTH must equal ELLIPSIS_NO_EXT.len()"
);

/// Minimum basename characters to show when preserving an extension.
///
/// When truncating a filename with an extension, we ensure at least this many
/// characters of the basename are visible. This prevents awkward results like
/// "...txt" with no visible filename portion. If there isn't enough space for
/// this minimum plus the ellipsis and extension, we fall back to simple
/// truncation without extension preservation.
///
/// # Design Rationale
///
/// The value 1 was chosen deliberately to maximize truncation flexibility:
///
/// - **Narrow terminal support**: A value of 1 allows truncation to work in
///   terminals as narrow as 70 columns (with typical progress bar overhead).
///   Higher values would require wider terminals.
///
/// - **Graceful degradation**: With value 1, the minimum truncated result is
///   `"X...ext"` (e.g., `"l...txt"`). While this shows minimal basename content,
///   the preserved extension still conveys file type information, which is often
///   the most important detail for progress bars.
///
/// - **Tradeoff accepted**: A value of 2-3 would produce more readable results
///   like `"lo...txt"` or `"lon...txt"`, but would fail in narrower terminals.
///   Since progress bars are primarily about showing *that* a file is being
///   processed (not identifying *which* file in detail), the extension alone
///   provides sufficient context in constrained situations.
///
/// - **Fallback behavior**: When even 1 character + ellipsis + extension doesn't
///   fit, the code falls back to simple truncation without extension preservation,
///   ensuring something always displays.
///
/// # Invariant
///
/// Must be at least 1 to ensure visible content in truncated output.
/// The compile-time assertion below enforces this invariant.
const MIN_BASENAME_CHARS: usize = 1;

// Compile-time assertion to ensure MIN_BASENAME_CHARS is sensible.
// A value of 0 would produce truncated filenames with no visible content.
const _: () = assert!(
    MIN_BASENAME_CHARS >= 1,
    "MIN_BASENAME_CHARS must be at least 1 to ensure visible content"
);

/// Minimum width required for meaningful truncation with an indicator.
///
/// Below this width, truncation returns raw characters without any ellipsis
/// indicator because there's not enough space for both content and indicator.
///
/// # Invariant
///
/// This equals `MIN_BASENAME_CHARS + ELLIPSIS_NO_EXT_WIDTH` (1 + 3 = 4), allowing
/// at least one content character plus the ellipsis indicator: `X...`
/// The compile-time assertion below enforces this invariant.
const MIN_TRUNCATION_WIDTH: usize = MIN_BASENAME_CHARS + ELLIPSIS_NO_EXT_WIDTH;

// Compile-time assertion: This is a CHANGE DETECTOR, not a value to blindly update.
// If this fails, it means MIN_BASENAME_CHARS or ELLIPSIS_NO_EXT_WIDTH changed.
// Review those constants and verify the new derived value is intentional before
// updating the expected value here.
//
// The assertion message includes the derivation so future maintainers can verify
// the expected value without needing to read surrounding comments.
const _: () = assert!(
    MIN_TRUNCATION_WIDTH == 4,
    "MIN_TRUNCATION_WIDTH changed! Expected MIN_BASENAME_CHARS(1) + ELLIPSIS_NO_EXT_WIDTH(3) = 4. Review those constants - was this intentional?"
);

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
        // We use chars().count() (character count) rather than display width because:
        // 1. Extension length limits are about recognizing file types, not display fitting
        // 2. Display width varies by character (CJK = 2 columns, ASCII = 1), making
        //    limits inconsistent (e.g., ".日本語" would be "3 chars but 6 columns")
        // 3. Character count provides predictable behavior across all scripts
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
/// 2. If `max_width < 4`, return raw prefix (no room for content + ellipsis)
/// 3. Extract the extension (last `.xxx` portion, if present and ≤10 chars)
/// 4. If extension exists and fits with minimum basename (1 char + `..` + `.ext`):
///    - Take as much basename as will fit, append `..` + `.extension`
/// 5. If extension exists but full extension doesn't fit:
///    - Split remaining space 1/3 to basename, 2/3 to extension (prioritizing
///      extension visibility since it indicates file type)
///    - This ratio ensures the extension remains recognizable even when truncated
/// 6. If no extension or not enough space: return `beginning...`
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
/// // The ellipsis ".." plus extension ".mkv" creates "...mkv" (3 visible dots)
/// assert!(truncated.contains("..."));
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

        // Check if we have room for: MIN_BASENAME_CHARS + .. + .ext
        // Using ELLIPSIS_WITH_EXT ("..") so result is "name...ext" (3 dots total)
        let min_with_ext = MIN_BASENAME_CHARS + ELLIPSIS_WITH_EXT_WIDTH + dot_ext_width;
        if max_width_usize >= min_with_ext {
            let basename_budget = max_width_usize - ELLIPSIS_WITH_EXT_WIDTH - dot_ext_width;
            // INVARIANT: This assertion is guaranteed by the `if` condition above.
            // Given: max_width_usize >= MIN_BASENAME_CHARS + ELLIPSIS_WITH_EXT_WIDTH + dot_ext_width
            // Then:  max_width_usize - ELLIPSIS_WITH_EXT_WIDTH - dot_ext_width >= MIN_BASENAME_CHARS
            // Therefore: basename_budget >= MIN_BASENAME_CHARS
            debug_assert!(basename_budget >= MIN_BASENAME_CHARS);
            let truncated_basename = take_chars_by_width(basename, basename_budget);
            return format!("{}{}{}", truncated_basename, ELLIPSIS_WITH_EXT, dot_ext);
        }

        // Not enough room to preserve the full extension with minimum basename visibility.
        // This branch is only reached when the extension is long (e.g., ".extension",
        // ".javascript"). Short extensions like ".c", ".rs", ".txt" always take the
        // branch above because they fit within the minimum requirements.
        //
        // Since we're here, the extension must be truncated. We allocate 1/3 of the
        // remaining space to basename and 2/3 to extension because:
        // 1. The extension indicates file type, which is often more useful than
        //    seeing more of the basename
        // 2. For long extensions, we want to preserve enough to be recognizable
        //    (e.g., ".javas" from ".javascript" is more useful than ".jav")
        //
        // NOTE: The resulting format like "ba...exten" has a visual ambiguity where
        // the ellipsis could be mistaken as part of the extension. This is an
        // acceptable tradeoff for this rare edge case (only reached with extensions
        // longer than ~6 characters that still need truncation). Short extensions
        // like .mkv, .txt, .rs are always preserved fully via the branch above.
        let remaining = max_width_usize.saturating_sub(ELLIPSIS_WITH_EXT_WIDTH);
        let basename_budget = remaining / 3;

        // If we can't show MIN_BASENAME_CHARS, fall through to no-extension truncation
        if basename_budget >= MIN_BASENAME_CHARS {
            let ext_budget = remaining - basename_budget;
            let truncated_basename = take_chars_by_width(basename, basename_budget);
            let truncated_ext = take_chars_by_width(&dot_ext, ext_budget);
            return format!("{}{}{}", truncated_basename, ELLIPSIS_WITH_EXT, truncated_ext);
        }

        // Fall through to no-extension truncation below
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
    fn test_constants_runtime_properties() {
        // Compile-time constant relationships are verified at module level.
        // This test verifies runtime properties that can't be checked at compile time.
        assert!(!PROGRESS_CHARS.is_empty(), "PROGRESS_CHARS must not be empty");

        // Verify PROGRESS_CHARS has the expected structure for indicatif:
        // 8 partial block characters + 2 spaces = 10 characters
        assert_eq!(
            PROGRESS_CHARS.chars().count(),
            10,
            "PROGRESS_CHARS should have 10 characters (8 blocks + 2 spaces)"
        );
    }

    #[test]
    fn test_ellipsis_constants_are_ascii() {
        // The ellipsis width constants assume ASCII (byte length = display width).
        // If these are changed to Unicode (e.g., '…'), the width calculation
        // would be incorrect. These runtime checks complement the compile-time
        // assertions in the constant definitions.
        assert!(
            ELLIPSIS_WITH_EXT.is_ascii(),
            "ELLIPSIS_WITH_EXT must be ASCII for width calculation to be correct"
        );
        assert!(
            ELLIPSIS_NO_EXT.is_ascii(),
            "ELLIPSIS_NO_EXT must be ASCII for width calculation to be correct"
        );

        // Verify width constants match actual display width
        assert_eq!(
            ELLIPSIS_WITH_EXT_WIDTH,
            ELLIPSIS_WITH_EXT.len(),
            "ELLIPSIS_WITH_EXT_WIDTH must equal string length"
        );
        assert_eq!(
            ELLIPSIS_NO_EXT_WIDTH,
            ELLIPSIS_NO_EXT.len(),
            "ELLIPSIS_NO_EXT_WIDTH must equal string length"
        );
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
    fn test_split_filename_extension_directory_like_names() {
        // Current directory "." and parent directory ".." should be returned unchanged
        // These are edge cases that might appear in path handling
        assert_eq!(split_filename_extension("."), (".", None));
        assert_eq!(split_filename_extension(".."), ("..", None));
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
    fn test_truncate_filename_directory_like_names() {
        // Current directory "." and parent directory ".." should be returned unchanged
        // since they're shorter than any reasonable max_width
        assert_eq!(truncate_filename(".", 10), ".");
        assert_eq!(truncate_filename("..", 10), "..");
        // Even at width 1, "." should be unchanged
        assert_eq!(truncate_filename(".", 1), ".");
        // At width 1, ".." must be truncated to "."
        assert_eq!(truncate_filename("..", 1), ".");
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
        // 15 chars - 3 (ellipsis) = 12 chars of basename
        assert_eq!(
            result, "Makefile_wit...",
            "Should truncate to 12 chars + ellipsis"
        );
        assert_eq!(str_display_width_as_u16(&result), 15);
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
        // At width 4, can't preserve extension, falls back to "l..."
        let result = truncate_filename("longfilename.txt", 4);
        assert_eq!(result, "l...", "Should be 1 char + 3 dot ellipsis");
        assert_eq!(str_display_width_as_u16(&result), 4);
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
    fn test_min_truncation_width_algebraic_relationship() {
        // Verify MIN_TRUNCATION_WIDTH equals MIN_BASENAME_CHARS + ELLIPSIS_NO_EXT_WIDTH.
        // This tests the relationship, not a magic number, so it remains valid if
        // the underlying constants change intentionally.
        assert_eq!(
            MIN_TRUNCATION_WIDTH,
            MIN_BASENAME_CHARS + ELLIPSIS_NO_EXT_WIDTH,
            "MIN_TRUNCATION_WIDTH must equal MIN_BASENAME_CHARS + ELLIPSIS_NO_EXT_WIDTH"
        );

        // Also verify the expected value as a change detector (mirrors compile-time assertion)
        assert_eq!(
            MIN_TRUNCATION_WIDTH, 4,
            "MIN_TRUNCATION_WIDTH changed from expected value 4. \
             If this is intentional, update this test and the compile-time assertion."
        );
    }

    #[test]
    fn test_min_basename_chars_minimum_bound() {
        // Verify MIN_BASENAME_CHARS is at least 1 (mirrors compile-time assertion).
        // A value of 0 would produce truncated filenames with no visible content.
        assert!(
            MIN_BASENAME_CHARS >= 1,
            "MIN_BASENAME_CHARS must be at least 1 to ensure visible content"
        );
    }

    #[test]
    fn test_falls_back_to_no_extension_when_basename_too_small() {
        // When there's not enough room for MIN_BASENAME_CHARS + ellipsis + extension,
        // we should fall back to simple truncation without extension preservation.
        // "file.txt" at width 4: can't fit "f...txt" (7 chars min for extension preservation)
        // So we should get "f..." (simple truncation)
        let result = truncate_filename("file.txt", 4);
        assert_eq!(
            result, "f...",
            "Should fall back to no-extension truncation when extension can't fit"
        );
    }

    #[test]
    fn test_preserves_extension_with_minimum_basename() {
        // At the boundary where we CAN fit MIN_BASENAME_CHARS + ellipsis + extension
        // "file.txt" at width 7: "f...txt" should work
        // min_with_ext = 1 (MIN_BASENAME_CHARS) + 2 (ellipsis) + 4 (.txt) = 7
        let result = truncate_filename("file.txt", 7);
        assert_eq!(
            result, "f...txt",
            "Should preserve extension with exactly MIN_BASENAME_CHARS"
        );
    }

    #[test]
    fn test_truncation_at_boundary() {
        // At exactly MIN_TRUNCATION_WIDTH (4), we should get 1 char + 3 dots
        // The extension can't be preserved because there's not enough room
        let result = truncate_filename("longfilename.txt", 4);
        assert_eq!(
            result, "l...",
            "At MIN_TRUNCATION_WIDTH, should get 1 char + ellipsis"
        );
        assert_eq!(str_display_width_as_u16(&result), 4);
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
    fn test_short_extensions_always_preserved_fully() {
        // Short extensions like .c, .rs, .txt should ALWAYS be preserved in full.
        // They never reach the 1/3-2/3 split code path because they always fit
        // within the minimum requirements (MIN_BASENAME_CHARS + ellipsis + ext).
        //
        // This test documents this important design invariant.

        // .c extension: min_with_ext = 1 + 2 + 2 = 5
        // At width 5, we should see "x...c" (full extension preserved)
        let result = truncate_filename("verylongfilename.c", 5);
        assert!(
            result.ends_with(".c"),
            "Short extension .c should always be fully preserved: {}",
            result
        );
        assert_eq!(result, "v...c");

        // .rs extension: min_with_ext = 1 + 2 + 3 = 6
        let result = truncate_filename("verylongfilename.rs", 6);
        assert!(
            result.ends_with(".rs"),
            "Short extension .rs should always be fully preserved: {}",
            result
        );
        assert_eq!(result, "v...rs");

        // .txt extension: min_with_ext = 1 + 2 + 4 = 7
        let result = truncate_filename("verylongfilename.txt", 7);
        assert!(
            result.ends_with(".txt"),
            "Short extension .txt should always be fully preserved: {}",
            result
        );
        assert_eq!(result, "v...txt");
    }

    #[test]
    fn test_long_extension_truncation_favors_extension() {
        // When both basename and extension must be truncated,
        // extension should get 2/3 of the budget.
        //
        // NOTE: This code path is only reached for LONG extensions that
        // can't fit fully. Short extensions like .c, .rs, .txt always
        // take the earlier branch where they're preserved in full.
        let result = truncate_filename("ab.longerext", 10);
        // max_width=10, ellipsis=2, remaining=8
        // basename_budget=8/3=2, ext_budget=8-2=6
        // truncated_basename from "ab" = "ab" (fits in 2)
        // truncated_ext from ".longerext" with budget 6 = ".longe"
        // Result: "ab" + ".." + ".longe" = "ab...longe" which is 10 chars

        assert_eq!(
            result, "ab...longe",
            "Expected exact truncation result for 'ab.longerext' at width 10"
        );
        assert_eq!(
            str_display_width_as_u16(&result),
            10,
            "Result should be exactly 10 columns"
        );
    }

    #[test]
    fn test_extension_truncation_preserves_dot() {
        // When truncating extension, the leading dot should be preserved

        // Test with an extension that exceeds MAX_EXTENSION_LEN (10).
        // The extension is treated as part of the basename, so we get simple truncation.
        let test_filename = "a.verylongextension";
        let ext_part = "verylongextension";
        // Self-documenting assertion: verify our understanding of the test data
        assert_eq!(
            ext_part.chars().count(),
            17,
            "Test invariant: extension must be 17 chars (> MAX_EXTENSION_LEN)"
        );
        assert!(
            ext_part.chars().count() > MAX_EXTENSION_LEN,
            "Test invariant: extension must exceed MAX_EXTENSION_LEN to test this code path"
        );

        let result = truncate_filename(test_filename, 8);
        assert_eq!(
            result, "a.ver...",
            "Very long extension treated as no extension"
        );

        // Test with a valid long extension that requires the 1/3-2/3 split code path.
        let ext_part = "extension";
        let test_filename2 = "basename.extension";
        let max_width: u16 = 10;

        // Self-documenting assertions: verify our understanding of the test data
        assert_eq!(ext_part.chars().count(), 9, "Test invariant: extension is 9 chars");
        assert!(
            ext_part.chars().count() <= MAX_EXTENSION_LEN,
            "Test invariant: extension must be <= MAX_EXTENSION_LEN to be recognized"
        );

        // Verify the truncation algorithm manually:
        // - dot_ext = ".extension" (10 chars)
        // - min_with_ext = MIN_BASENAME_CHARS(1) + ELLIPSIS_WITH_EXT_WIDTH(2) + 10 = 13
        // - max_width(10) < min_with_ext(13), so we enter the 1/3-2/3 split branch
        // - remaining = max_width - ELLIPSIS_WITH_EXT_WIDTH = 10 - 2 = 8
        // - basename_budget = remaining / 3 = 8 / 3 = 2
        // - ext_budget = remaining - basename_budget = 8 - 2 = 6
        // - truncated_basename = take_chars_by_width("basename", 2) = "ba"
        // - truncated_ext = take_chars_by_width(".extension", 6) = ".exten"
        // - Result: "ba" + ".." + ".exten" = "ba...exten" (10 chars)
        let dot_ext_width = 1 + ext_part.chars().count(); // +1 for the dot
        let min_with_ext = MIN_BASENAME_CHARS + ELLIPSIS_WITH_EXT_WIDTH + dot_ext_width;
        assert!(
            usize::from(max_width) < min_with_ext,
            "Test invariant: max_width must be < min_with_ext to trigger 1/3-2/3 split"
        );

        let result2 = truncate_filename(test_filename2, max_width);
        assert_eq!(
            result2, "ba...exten",
            "Should truncate both basename and extension using 1/3-2/3 split"
        );
        assert_eq!(str_display_width_as_u16(&result2), max_width);

        // Verify the expected parts are preserved
        assert!(result2.starts_with("ba"), "Should preserve start of basename");
        assert!(result2.contains("..."), "Should have ellipsis");
        // Note: The result "ba...exten" is constructed as:
        //   "ba" (truncated basename) + ".." (ellipsis) + ".exten" (truncated extension with dot)
        // The three visible dots come from the ellipsis ".." plus the dot in ".exten".
    }

    // ========================================================================
    // Tests for unicode extension handling (chars().count() vs len())
    // ========================================================================

    #[test]
    fn test_unicode_extension_char_count() {
        // Test that extension length uses char count, not byte count.
        // CJK characters are multi-byte but count as single characters.
        let ext_part = "日本語";
        // Self-documenting assertions: verify our understanding of the test data
        assert_eq!(ext_part.chars().count(), 3, "Test invariant: 3 CJK characters");
        assert_eq!(ext_part.len(), 9, "Test invariant: 9 bytes (3 chars × 3 bytes each)");
        assert!(
            ext_part.chars().count() <= MAX_EXTENSION_LEN,
            "Test invariant: extension must be <= MAX_EXTENSION_LEN to be recognized"
        );

        let (basename, ext) = split_filename_extension("file.日本語");
        assert_eq!(basename, "file");
        assert_eq!(ext, Some("日本語"));
    }

    #[test]
    fn test_unicode_extension_too_long_by_chars() {
        // Extension exceeding MAX_EXTENSION_LEN by character count (not bytes).
        // Hiragana characters are multi-byte but count as single characters.
        let long_ext = "あいうえおかきくけこさ";
        // Self-documenting assertion: verify our understanding of the test data
        assert_eq!(
            long_ext.chars().count(),
            11,
            "Test invariant: extension must be 11 chars (> MAX_EXTENSION_LEN)"
        );
        assert!(
            long_ext.chars().count() > MAX_EXTENSION_LEN,
            "Test invariant: extension must exceed MAX_EXTENSION_LEN to test this code path"
        );

        let filename = format!("file.{}", long_ext);
        let (basename, ext) = split_filename_extension(&filename);
        // Exceeds MAX_EXTENSION_LEN, so treated as no extension
        assert_eq!(basename, filename.as_str());
        assert_eq!(ext, None);
    }

    #[test]
    fn test_unicode_extension_at_max_length() {
        // Extension with exactly MAX_EXTENSION_LEN unicode characters (boundary test).
        let exactly_max = "あいうえおかきくけこ";
        // Self-documenting assertion: verify our understanding of the test data
        assert_eq!(
            exactly_max.chars().count(),
            MAX_EXTENSION_LEN,
            "Test invariant: extension must be exactly MAX_EXTENSION_LEN chars"
        );

        let filename = format!("file.{}", exactly_max);
        let (basename, ext) = split_filename_extension(&filename);

        assert_eq!(basename, "file");
        assert_eq!(ext, Some(exactly_max));
    }

    // ========================================================================
    // Algebraic relationship tests
    // ========================================================================
    //
    // These tests verify the algebraic relationships that the code relies on.
    // They use the actual constants to ensure the tests remain valid if
    // constants are changed, and provide clear failure messages.
    // ========================================================================

    #[test]
    fn test_basename_budget_algebraic_relationship() {
        // This test verifies the algebraic relationship used by the debug_assert
        // in truncate_filename: when max_width >= min_with_ext, then
        // basename_budget >= MIN_BASENAME_CHARS.
        //
        // Given: max_width >= MIN_BASENAME_CHARS + ELLIPSIS_WITH_EXT_WIDTH + dot_ext_width
        // Then:  basename_budget = max_width - ELLIPSIS_WITH_EXT_WIDTH - dot_ext_width
        //        >= MIN_BASENAME_CHARS

        // Use a concrete example with .txt extension (4 chars including dot)
        let ext_width = 3; // "txt"
        let dot_ext_width = ext_width + 1; // ".txt" = 4

        // Calculate the minimum width where extension preservation is possible
        let min_with_ext = MIN_BASENAME_CHARS + ELLIPSIS_WITH_EXT_WIDTH + dot_ext_width;

        // At exactly this width, basename_budget should equal MIN_BASENAME_CHARS
        let basename_budget_at_min = min_with_ext - ELLIPSIS_WITH_EXT_WIDTH - dot_ext_width;
        assert_eq!(
            basename_budget_at_min, MIN_BASENAME_CHARS,
            "At min_with_ext, basename_budget should equal MIN_BASENAME_CHARS. \
             This verifies the algebraic relationship used in truncate_filename."
        );

        // Verify the actual truncation produces the expected result
        // min_with_ext = 1 + 2 + 4 = 7 for .txt
        let result = truncate_filename("longfilename.txt", min_with_ext as u16);

        // At min_with_ext, the result should be "l...txt" (1 basename char + ".." + ".txt")
        // Verify the structure directly rather than using a filter that could be misleading
        assert_eq!(
            result, "l...txt",
            "At min_with_ext width, should get exactly MIN_BASENAME_CHARS basename + ellipsis + extension"
        );

        // Also verify the basename portion has exactly MIN_BASENAME_CHARS characters
        // by checking that the result starts with exactly that many chars before the ellipsis
        let basename_portion: String = result.chars().take_while(|c| *c != '.').collect();
        assert_eq!(
            basename_portion.chars().count(),
            MIN_BASENAME_CHARS,
            "Basename portion before ellipsis should have exactly MIN_BASENAME_CHARS characters"
        );
    }
}
