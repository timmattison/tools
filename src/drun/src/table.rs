//! Lays rows of text out in columns.

use unicode_width::UnicodeWidthStr;

/// The space between two columns.
const GAP: &str = "  ";

/// Lays `header` and `rows` out in columns, one line for each, with two
/// spaces between the columns.
///
/// Each column is as wide as its widest cell, in terminal columns. A row can
/// have fewer or more cells than the header. The last cell of a line gets no
/// padding, so no line ends in spaces.
pub fn render(header: &[&str], rows: &[Vec<String>]) -> String {
    todo!("{header:?} {rows:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(|cell| (*cell).to_owned()).collect()
    }

    #[test]
    fn columns_are_as_wide_as_their_widest_cell() {
        let text = render(
            &["HOST", "STATE"],
            &[row(&["box-a", "running"]), row(&["b", "exited"])],
        );
        assert_eq!(text, "HOST   STATE\nbox-a  running\nb      exited\n");
    }

    #[test]
    fn a_row_with_more_cells_than_the_header_gets_more_columns() {
        let text = render(&["A", "B"], &[row(&["1", "2", "chosen"]), row(&["10", "20"])]);
        assert_eq!(text, "A   B\n1   2   chosen\n10  20\n");
    }

    #[test]
    fn width_counts_terminal_columns_and_not_bytes() {
        let text = render(&["NAME", "X"], &[row(&["日本", "1"]), row(&["café", "2"])]);
        assert_eq!(text, "NAME  X\n日本  1\ncafé  2\n");
    }

    #[test]
    fn no_rows_gives_the_header_alone() {
        assert_eq!(render(&["HOST", "NAME"], &[]), "HOST  NAME\n");
    }
}
