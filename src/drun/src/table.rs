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
    let lines: Vec<Vec<&str>> = std::iter::once(header.to_vec())
        .chain(
            rows.iter()
                .map(|row| row.iter().map(String::as_str).collect()),
        )
        .collect();
    let columns = lines.iter().map(Vec::len).max().unwrap_or_default();
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            lines
                .iter()
                .filter_map(|line| line.get(column))
                .map(|cell| cell.width())
                .max()
                .unwrap_or_default()
        })
        .collect();
    let mut text = String::new();
    for line in &lines {
        let last = line.len().saturating_sub(1);
        for (column, (cell, width)) in line.iter().zip(&widths).enumerate() {
            text.push_str(cell);
            if column < last {
                text.push_str(&" ".repeat(width - cell.width()));
                text.push_str(GAP);
            }
        }
        text.push('\n');
    }
    text
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
        let text = render(
            &["A", "B"],
            &[row(&["1", "2", "chosen"]), row(&["10", "20"])],
        );
        assert_eq!(text, "A   B\n1   2   chosen\n10  20\n");
    }

    #[test]
    fn width_counts_terminal_columns_and_not_bytes() {
        let text = render(&["NAME", "X"], &[row(&["日本", "1"]), row(&["café", "2"])]);
        assert_eq!(text, "NAME  X\n日本  1\ncafé  2\n");
    }

    #[test]
    fn an_empty_last_cell_leaves_no_spaces_at_the_end() {
        let text = render(
            &["HOST", "PORTS"],
            &[row(&["box-a", ""]), row(&["b", "80"])],
        );
        assert_eq!(text, "HOST   PORTS\nbox-a\nb      80\n");
    }

    #[test]
    fn no_rows_gives_the_header_alone() {
        assert_eq!(render(&["HOST", "NAME"], &[]), "HOST  NAME\n");
    }
}
