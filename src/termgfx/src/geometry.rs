//! The size of the things a terminal draws an image into.
//!
//! A terminal draws text in a grid of character cells, and it draws an image
//! in pixels. A tool that puts an image on that grid therefore has to convert
//! between the two, and the conversion needs the size of one cell. This module
//! holds that measure and every size that comes off it.

/// The measured size of one character cell, in pixels, when the terminal
/// reports one.
///
/// A tool that draws an image into a fixed number of character cells has to
/// know how many pixels those cells hold. A terminal answers that question
/// through the `TIOCGWINSZ` ioctl, which carries the pixel size of the window
/// beside the column count and the row count of the same window. One cell is
/// the pixel width over the column count, and the pixel height over the row
/// count.
///
/// The answer is `None` when the terminal reports no pixel size. A pane of
/// Zellij reports none, a ttyd panel reports none, and a terminal that carries
/// no window reports none. A caller that gets `None` holds no measure of a
/// cell, and it must then draw something that needs no measure. `krt` draws
/// block characters.
///
/// A zero is no size, for the reason that `termsize` gives for a width: a
/// window of no columns holds no character of a line, so a division by that
/// count measures nothing, and the number beside a zero comes from the same
/// ioctl that reported the zero. See `src/termsize/src/lib.rs`. A column count
/// of zero, a row count of zero, and a quotient of zero pixels therefore all
/// give `None` here.
#[must_use]
pub fn cell_pixels() -> Option<(u32, u32)> {
    todo!("read the pixel size and the cell counts of the terminal, then divide the one by the other")
}

/// The size of one character cell that a pair of probe answers measures.
///
/// The read of the terminal stands apart from this arithmetic, so a test names
/// the answer of a terminal without a terminal to name it with.
///
/// # Arguments
/// * `terminal_pixels` - The width and the height of the window in pixels, when
///   the terminal reports them.
/// * `terminal_cells` - The columns and the rows of the same window, when a
///   probe measured them.
///
/// # Returns
/// The width and the height of one cell in pixels, or `None` when either probe
/// answered nothing, when either count is zero, or when either quotient is
/// zero.
fn cell_pixels_of(
    terminal_pixels: Option<(u32, u32)>,
    terminal_cells: Option<(u16, u16)>,
) -> Option<(u32, u32)> {
    todo!("divide the pixel size by the cell counts, and take a zero as no size")
}

#[cfg(test)]
mod tests {
    use super::cell_pixels_of;

    /// The pixel size that a window of cells 10 pixels wide and 20 pixels tall
    /// reports over 80 columns and 24 rows.
    const REPORTED_PIXELS: (u32, u32) = (800, 480);

    /// The columns and the rows of that same window.
    const REPORTED_CELLS: (u16, u16) = (80, 24);

    #[test]
    fn a_reported_pixel_size_measures_one_cell_and_no_pixel_size_measures_nothing() {
        assert_eq!(
            cell_pixels_of(Some(REPORTED_PIXELS), Some(REPORTED_CELLS)),
            Some((10, 20)),
            "800 pixels over 80 columns is a cell 10 pixels wide, and 480 pixels over 24 rows is a cell 20 pixels tall"
        );
        assert_eq!(
            cell_pixels_of(None, Some(REPORTED_CELLS)),
            None,
            "a terminal that reports no pixel size gives nothing to divide, so it measures no cell"
        );
    }
}
