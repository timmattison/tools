//! Progress bar style builders.
//!
//! This module provides builders for creating progress bar styles that
//! automatically adjust to the terminal width.

use indicatif::ProgressStyle;

use crate::error::{Result, TermbarError};
use crate::{
    calculate_bar_width, calculate_max_filename_width, escape_template_braces,
    str_display_width_as_u16, truncate_filename, PROGRESS_CHARS,
};

/// Common format string for progress stats (bytes, percentage, speed, ETA).
const PROGRESS_STATS_FORMAT: &str = "{bytes}/{total_bytes} ({percent}%) ({bytes_per_sec}, {eta})";

/// Format string for batch progress stats (bytes, speed, ETA).
/// `{msg}` is used for file count and status (e.g., "(0/3)" or "(2/3)").
const BATCH_PROGRESS_STATS_FORMAT: &str =
    "{msg} {bytes}/{total_bytes} @ {bytes_per_sec} (~{eta} remaining)";

/// Base overhead for copy style progress bars.
///
/// Components: spinner(2) + brackets(4) + bytes(25) + speed/eta(25) + spaces(3) = ~60
/// The filename width is added to this to get total overhead.
const COPY_STYLE_BASE_OVERHEAD: u16 = 60;

/// Base overhead for verify style progress bars.
///
/// Components: spinner(2) + brackets(4) + bytes(25) + speed/eta(25) + " verifying"(10) + spaces(3) = ~70
/// The filename width is added to this to get total overhead.
const VERIFY_STYLE_BASE_OVERHEAD: u16 = 70;

/// Base overhead for batch style progress bars.
///
/// Components: "Batch" prefix + brackets + stats format = ~85
const BATCH_STYLE_OVERHEAD: u16 = 85;

/// Base overhead for hash style progress bars.
///
/// Components: spinner(2) + brackets(4) + bytes/total(25) + speed/eta(35) + msg(variable) + spaces(4) = ~70
const HASH_STYLE_OVERHEAD: u16 = 70;

/// Builder for progress bar styles with automatic width calculation.
///
/// This builder creates progress styles that adapt to the terminal width,
/// ensuring the progress bar fits properly at any terminal size.
///
/// # Example
///
/// ```rust,ignore
/// use termbar::{ProgressStyleBuilder, TerminalWidth};
///
/// let width = TerminalWidth::get_or_default();
/// let style = ProgressStyleBuilder::copy("myfile.txt").build(width)?;
/// ```
#[derive(Debug, Clone)]
pub struct ProgressStyleBuilder {
    style_type: StyleType,
    progress_chars: String,
    custom_filename: Option<String>,
}

#[derive(Debug, Clone)]
enum StyleType {
    /// Copy progress style (cyan bar with spinner).
    Copy,
    /// Verification progress style (yellow bar with spinner).
    Verify,
    /// Batch progress style (blue bar with prefix).
    Batch,
    /// Hash progress style (cyan bar with spinner and hash prefix).
    Hash,
}

impl ProgressStyleBuilder {
    /// Create a progress style builder for file copy operations.
    ///
    /// Shows: `⠁ filename [████████░░░░] bytes/total (pct%) (speed, eta)`
    ///
    /// # Arguments
    ///
    /// * `filename` - The filename to display in the progress bar.
    #[must_use]
    pub fn copy(filename: &str) -> Self {
        Self {
            style_type: StyleType::Copy,
            progress_chars: PROGRESS_CHARS.to_string(),
            custom_filename: Some(filename.to_string()),
        }
    }

    /// Create a progress style builder for file verification operations.
    ///
    /// Shows: `⠁ filename [████████░░░░] bytes/total (pct%) (speed, eta) verifying`
    ///
    /// # Arguments
    ///
    /// * `filename` - The filename to display in the progress bar.
    #[must_use]
    pub fn verify(filename: &str) -> Self {
        Self {
            style_type: StyleType::Verify,
            progress_chars: PROGRESS_CHARS.to_string(),
            custom_filename: Some(filename.to_string()),
        }
    }

    /// Create a progress style builder for batch operations.
    ///
    /// Shows: `Batch [████████░░░░] (n/total) bytes/total @ speed (~eta remaining)`
    #[must_use]
    pub fn batch() -> Self {
        Self {
            style_type: StyleType::Batch,
            progress_chars: PROGRESS_CHARS.to_string(),
            custom_filename: None,
        }
    }

    /// Create a progress style builder for file hashing operations.
    ///
    /// Shows: `⠁ [████████░░░░] bytes/total (pct%) (speed, eta) msg`
    ///
    /// This style is designed for hash operations where the filename
    /// is shown via the progress bar message rather than the template.
    #[must_use]
    pub fn hash() -> Self {
        Self {
            style_type: StyleType::Hash,
            progress_chars: PROGRESS_CHARS.to_string(),
            custom_filename: None,
        }
    }

    /// Set custom progress characters.
    ///
    /// The default is `█▉▊▋▌▍▎▏  ` which provides smooth sub-character progress.
    ///
    /// # Arguments
    ///
    /// * `chars` - Progress characters string for indicatif.
    #[must_use]
    pub fn with_progress_chars(mut self, chars: &str) -> Self {
        self.progress_chars = chars.to_string();
        self
    }

    /// Build the progress style for the given terminal width.
    ///
    /// # Arguments
    ///
    /// * `terminal_width` - The current terminal width in columns.
    ///
    /// # Errors
    ///
    /// Returns an error if the progress style template is invalid.
    pub fn build(&self, terminal_width: u16) -> Result<ProgressStyle> {
        let template = self.create_template(terminal_width);

        Ok(ProgressStyle::default_bar()
            .template(&template)
            .map_err(|e| TermbarError::StyleCreation(e.to_string()))?
            .progress_chars(&self.progress_chars))
    }

    /// Create the template string for this style type.
    ///
    /// This is exposed as `pub(crate)` for testing purposes.
    pub(crate) fn create_template(&self, terminal_width: u16) -> String {
        match &self.style_type {
            StyleType::Copy => {
                // Calculate display width on the ORIGINAL filename, not the escaped version.
                // Escaped braces ({{ and }}) are template syntax that render as single characters.
                let original = self.custom_filename.as_deref().unwrap_or_default();

                // Calculate maximum filename width that fits with minimum bar
                let max_filename_width =
                    calculate_max_filename_width(terminal_width, COPY_STYLE_BASE_OVERHEAD);

                // Truncate filename if needed to ensure the line fits
                let truncated = truncate_filename(original, max_filename_width);
                let filename_display_width = str_display_width_as_u16(&truncated);
                let filename = escape_template_braces(&truncated);

                let overhead = COPY_STYLE_BASE_OVERHEAD + filename_display_width;
                let bar_width = calculate_bar_width(terminal_width, overhead);
                format!(
                    "{{spinner:.green}} {} [{{bar:{}.cyan/blue}}] {}",
                    filename, bar_width, PROGRESS_STATS_FORMAT
                )
            }
            StyleType::Verify => {
                // Calculate display width on the ORIGINAL filename, not the escaped version.
                // Escaped braces ({{ and }}) are template syntax that render as single characters.
                let original = self.custom_filename.as_deref().unwrap_or_default();

                // Calculate maximum filename width that fits with minimum bar
                let max_filename_width =
                    calculate_max_filename_width(terminal_width, VERIFY_STYLE_BASE_OVERHEAD);

                // Truncate filename if needed to ensure the line fits
                let truncated = truncate_filename(original, max_filename_width);
                let filename_display_width = str_display_width_as_u16(&truncated);
                let filename = escape_template_braces(&truncated);

                let overhead = VERIFY_STYLE_BASE_OVERHEAD + filename_display_width;
                let bar_width = calculate_bar_width(terminal_width, overhead);
                format!(
                    "{{spinner:.yellow}} {} [{{bar:{}.yellow/dim}}] {} verifying",
                    filename, bar_width, PROGRESS_STATS_FORMAT
                )
            }
            StyleType::Batch => {
                let bar_width = calculate_bar_width(terminal_width, BATCH_STYLE_OVERHEAD);
                format!(
                    "{{prefix:.bold}} [{{bar:{}.blue/dim}}] {}",
                    bar_width, BATCH_PROGRESS_STATS_FORMAT
                )
            }
            StyleType::Hash => {
                let bar_width = calculate_bar_width(terminal_width, HASH_STYLE_OVERHEAD);
                format!(
                    "{{spinner:.green}} [{{bar:{}.cyan/blue}}] {} {{msg}}",
                    bar_width, PROGRESS_STATS_FORMAT
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_copy_style_builds() {
        let style = ProgressStyleBuilder::copy("test.txt").build(80);
        assert!(style.is_ok());
    }

    #[test]
    fn test_verify_style_builds() {
        let style = ProgressStyleBuilder::verify("test.txt").build(80);
        assert!(style.is_ok());
    }

    #[test]
    fn test_batch_style_builds() {
        let style = ProgressStyleBuilder::batch().build(80);
        assert!(style.is_ok());
    }

    #[test]
    fn test_hash_style_builds() {
        let style = ProgressStyleBuilder::hash().build(80);
        assert!(style.is_ok());
    }

    #[test]
    fn test_custom_progress_chars() {
        let style = ProgressStyleBuilder::copy("test.txt")
            .with_progress_chars("#>-")
            .build(80);
        assert!(style.is_ok());
    }

    #[test]
    fn test_narrow_terminal() {
        // Should still build for very narrow terminals
        let style = ProgressStyleBuilder::copy("test.txt").build(20);
        assert!(style.is_ok());
    }

    #[test]
    fn test_wide_terminal() {
        // Should still build for very wide terminals
        let style = ProgressStyleBuilder::copy("test.txt").build(200);
        assert!(style.is_ok());
    }

    #[test]
    fn test_filename_with_braces() {
        // Filenames with braces should be escaped
        let style = ProgressStyleBuilder::copy("file{1}.txt").build(80);
        assert!(style.is_ok());
    }

    /// Helper to extract bar width from a template string.
    /// Looks for pattern like `{bar:XX.` where XX is the width.
    ///
    /// Uses character iteration instead of byte slicing to be robust against
    /// templates containing unicode characters (e.g., emoji filenames).
    fn extract_bar_width(template: &str) -> Option<u16> {
        // Find "{bar:" pattern using character iteration, not byte offsets.
        // This handles templates with unicode filenames correctly.
        let bar_marker: Vec<char> = "{bar:".chars().collect();
        let chars: Vec<char> = template.chars().collect();

        // Find the start position of "{bar:" in character indices
        let bar_start = chars
            .windows(bar_marker.len())
            .position(|window| window == bar_marker)?;

        // Extract digits after "{bar:" until we hit a non-digit
        let width_str: String = chars
            .iter()
            .skip(bar_start + bar_marker.len())
            .take_while(|c| c.is_ascii_digit())
            .collect();

        width_str.parse().ok()
    }

    #[test]
    fn test_bar_width_same_for_braces_vs_no_braces_copy() {
        // Regression test: bar width should be based on DISPLAY width, not escaped length.
        // "abcde" and "a{b}c" both have display width 5, so bar width should be identical.
        let template_no_braces = ProgressStyleBuilder::copy("abcde").create_template(120);
        let template_with_braces = ProgressStyleBuilder::copy("a{b}c").create_template(120);

        let width_no_braces = extract_bar_width(&template_no_braces)
            .expect("Failed to extract bar width from template without braces");
        let width_with_braces = extract_bar_width(&template_with_braces)
            .expect("Failed to extract bar width from template with braces");

        assert_eq!(
            width_no_braces, width_with_braces,
            "Bar width should be identical for filenames with same display width.\n\
             Without braces: {} (width {})\n\
             With braces: {} (width {})",
            template_no_braces, width_no_braces, template_with_braces, width_with_braces
        );
    }

    #[test]
    fn test_bar_width_same_for_braces_vs_no_braces_verify() {
        // Regression test for verify style
        let template_no_braces = ProgressStyleBuilder::verify("abcde").create_template(120);
        let template_with_braces = ProgressStyleBuilder::verify("a{b}c").create_template(120);

        let width_no_braces = extract_bar_width(&template_no_braces)
            .expect("Failed to extract bar width from template without braces");
        let width_with_braces = extract_bar_width(&template_with_braces)
            .expect("Failed to extract bar width from template with braces");

        assert_eq!(
            width_no_braces, width_with_braces,
            "Bar width should be identical for filenames with same display width"
        );
    }

    #[test]
    fn test_template_escapes_braces_correctly() {
        // Verify that braces are properly escaped in the template
        let template = ProgressStyleBuilder::copy("file{1}.txt").create_template(120);

        // The filename should appear with doubled braces
        assert!(
            template.contains("file{{1}}.txt"),
            "Template should contain escaped braces: {}",
            template
        );
    }

    #[test]
    fn test_unicode_filename_display_width() {
        // Test that Unicode filenames use display width, not byte length.
        // "file🎉.txt" is 12 bytes but only 10 display columns (emoji is 2 wide).
        // "file1234.txt" is 12 bytes and 12 display columns.
        let template_emoji = ProgressStyleBuilder::copy("file🎉.txt").create_template(120);
        let template_ascii = ProgressStyleBuilder::copy("file1234.txt").create_template(120);

        let width_emoji = extract_bar_width(&template_emoji)
            .expect("Failed to extract bar width from emoji template");
        let width_ascii = extract_bar_width(&template_ascii)
            .expect("Failed to extract bar width from ASCII template");

        // Emoji filename has smaller display width (10 vs 12), so bar should be wider
        assert!(
            width_emoji > width_ascii,
            "Emoji filename (10 cols) should have wider bar than ASCII (12 cols).\n\
             Emoji bar: {}, ASCII bar: {}",
            width_emoji, width_ascii
        );
    }

    // Tests for filename truncation integration

    #[test]
    fn test_copy_style_long_filename_builds() {
        let long_filename = "American.Psycho.2000.UNCUT.2160p.BluRay.REMUX.HEVC.DTS-HD.MA.TrueHD.7.1.Atmos-FGT.mkv";

        // Test at various terminal widths
        for width in [80, 100, 120, 60] {
            let style = ProgressStyleBuilder::copy(long_filename).build(width);
            assert!(style.is_ok(), "Should build at width {}", width);
        }
    }

    #[test]
    fn test_verify_style_long_filename_builds() {
        let long_filename = "American.Psycho.2000.UNCUT.2160p.BluRay.REMUX.HEVC.DTS-HD.MA.TrueHD.7.1.Atmos-FGT.mkv";

        for width in [80, 100, 120, 60] {
            let style = ProgressStyleBuilder::verify(long_filename).build(width);
            assert!(style.is_ok(), "Should build at width {}", width);
        }
    }

    #[test]
    fn test_template_truncates_long_filename() {
        // This test verifies that very long filenames get truncated
        let long_filename = "American.Psycho.2000.UNCUT.2160p.BluRay.REMUX.HEVC.DTS-HD.MA.TrueHD.7.1.Atmos-FGT.mkv";
        let terminal_width: u16 = 80;

        let template = ProgressStyleBuilder::copy(long_filename).create_template(terminal_width);

        // The template should NOT contain the full "Atmos-FGT" part since truncation should occur
        assert!(
            !template.contains("Atmos-FGT"),
            "Filename should be truncated, but found full name in template: {}",
            template
        );

        // The template should contain ellipsis from truncation
        assert!(
            template.contains("..."),
            "Truncated filename should contain ellipsis: {}",
            template
        );

        // The template should preserve .mkv extension
        assert!(
            template.contains(".mkv"),
            "Should preserve .mkv extension: {}",
            template
        );
    }

    #[test]
    fn test_template_preserves_short_filename() {
        // Short filenames should not be truncated
        let short_filename = "movie.mkv";
        let terminal_width: u16 = 120;

        let template = ProgressStyleBuilder::copy(short_filename).create_template(terminal_width);

        // The template should contain the full filename (escaped if needed)
        assert!(
            template.contains("movie.mkv"),
            "Short filename should not be truncated: {}",
            template
        );

        // Should NOT contain ellipsis
        assert!(
            !template.contains("..."),
            "Short filename should not have ellipsis: {}",
            template
        );
    }

    #[test]
    fn test_verify_template_truncates_long_filename() {
        let long_filename = "American.Psycho.2000.UNCUT.2160p.BluRay.REMUX.HEVC.DTS-HD.MA.TrueHD.7.1.Atmos-FGT.mkv";
        // Use 120 width - verify style has 70 base overhead, so max_filename = 120 - 70 - 10 = 40
        let terminal_width: u16 = 120;

        let template = ProgressStyleBuilder::verify(long_filename).create_template(terminal_width);

        // Verify style has more overhead (70 vs 60), so truncation should be more aggressive
        assert!(
            !template.contains("Atmos-FGT"),
            "Filename should be truncated in verify style: {}",
            template
        );

        // Should contain ellipsis and extension
        assert!(
            template.contains("...") && template.contains(".mkv"),
            "Should have ellipsis and .mkv extension: {}",
            template
        );

        // Should contain " verifying" suffix
        assert!(
            template.contains(" verifying"),
            "Verify style should have verifying suffix: {}",
            template
        );
    }
}
