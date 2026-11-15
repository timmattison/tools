//! The lines of text that show a [`Summary`](crate::verdict::Summary).

use crate::verdict::Summary;

/// The widest line that [`render`] writes, in display columns.
pub const WIDTH: usize = 80;

/// Gives the lines that show `summary`.
#[must_use]
pub fn render(_summary: &Summary) -> Vec<String> {
    Vec::new()
}

/// Cuts `label` to `width` display columns, or pads it with spaces to that
/// width.
fn fit(label: &str, _width: usize) -> String {
    label.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use termbar::str_display_width_as_u16;

    /// Gives the display width of `text` as a `usize`.
    fn columns(text: &str) -> usize {
        usize::from(str_display_width_as_u16(text))
    }

    /// A label that fits gets spaces up to the column, counted in display
    /// columns and not in characters.
    #[test]
    fn a_label_that_fits_is_padded_to_the_column() {
        assert_eq!(fit("abc", 6), "abc   ");
        assert_eq!(fit("abc", 3), "abc");
        let padded = fit("日本", 8);
        assert_eq!(padded, "日本    ");
        assert_eq!(columns(&padded), 8);
    }

    /// A label that is too wide ends with an ellipsis, and it fills the
    /// column exactly.
    #[test]
    fn a_label_that_is_too_wide_ends_with_an_ellipsis() {
        let cut = fit("abcdefghij", 6);
        assert_eq!(cut, "abcde…");
        assert_eq!(columns(&cut), 6);
    }

    /// A cut falls on a character boundary and never goes over the column.
    /// A character of two columns that does not fit leaves one space.
    #[test]
    fn a_cut_keeps_whole_characters_and_never_goes_over_the_column() {
        let japanese = fit("日本語のツール", 8);
        assert_eq!(japanese, "日本語 …");
        assert_eq!(columns(&japanese), 8);
        let party = fit("🎉🎉🎉🎉", 6);
        assert_eq!(party, "🎉🎉 …");
        assert_eq!(columns(&party), 6);
        let cafe = fit("café au lait", 6);
        assert_eq!(cafe, "café …");
        assert_eq!(columns(&cafe), 6);
        for width in 1..12 {
            for label in ["日本語のツール", "🎉 party", "café au lait"] {
                assert!(columns(&fit(label, width)) <= width, "{label} in {width}");
            }
        }
    }

    /// An empty label gives only spaces.
    #[test]
    fn an_empty_label_gives_only_spaces() {
        assert_eq!(fit("", 4), "    ");
    }
}
