//! Progress bar style builders.
//!
//! This module provides builders for creating progress bar styles that
//! automatically adjust to the terminal width.

use indicatif::ProgressStyle;

use crate::error::{Result, TermbarError};
use crate::{calculate_bar_width, escape_template_braces, str_display_width_as_u16, PROGRESS_CHARS};

/// Common format string for progress stats (bytes, percentage, speed, ETA).
const PROGRESS_STATS_FORMAT: &str = "{bytes}/{total_bytes} ({percent}%) ({bytes_per_sec}, {eta})";

/// Format string for batch progress stats (bytes, speed, ETA).
/// `{msg}` is used for file count and status (e.g., "(0/3)" or "(2/3)").
const BATCH_PROGRESS_STATS_FORMAT: &str =
    "{msg} {bytes}/{total_bytes} @ {bytes_per_sec} (~{eta} remaining)";

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
                let filename_display_width = str_display_width_as_u16(original);
                let filename = escape_template_braces(original);
                // spinner(2) + filename + brackets(4) + bytes(25) + speed/eta(25) + spaces(3) = ~60 + filename display width
                let overhead = 60 + filename_display_width;
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
                let filename_display_width = str_display_width_as_u16(original);
                let filename = escape_template_braces(original);
                // spinner(2) + filename + brackets(4) + bytes(25) + speed/eta(25) + " verifying"(10) + spaces(3) = ~70 + filename display width
                let overhead = 70 + filename_display_width;
                let bar_width = calculate_bar_width(terminal_width, overhead);
                format!(
                    "{{spinner:.yellow}} {} [{{bar:{}.yellow/dim}}] {} verifying",
                    filename, bar_width, PROGRESS_STATS_FORMAT
                )
            }
            StyleType::Batch => {
                // "Batch [bar] (99/99) 999.99 GiB/999.99 GiB @ 999.99 MiB/s (~99:99:99 remaining)" = ~85 chars overhead
                let bar_width = calculate_bar_width(terminal_width, 85);
                format!(
                    "{{prefix:.bold}} [{{bar:{}.blue/dim}}] {}",
                    bar_width, BATCH_PROGRESS_STATS_FORMAT
                )
            }
            StyleType::Hash => {
                // spinner(2) + brackets(4) + bytes/total(25) + speed/eta(35) + msg(variable) + spaces(4) = ~70 overhead
                // We use a larger overhead to leave room for the message
                let bar_width = calculate_bar_width(terminal_width, 70);
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
    fn extract_bar_width(template: &str) -> Option<u16> {
        let bar_start = template.find("{bar:")?;
        let after_bar = &template[bar_start + 5..];
        let dot_pos = after_bar.find('.')?;
        after_bar[..dot_pos].parse().ok()
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
}
