//! The size of the things a terminal draws an image into.
//!
//! A terminal draws text in a grid of character cells, and it draws an image
//! in pixels. A tool that puts an image on that grid therefore has to convert
//! between the two, and the conversion needs the size of one cell. This module
//! holds that measure and every size that comes off it.

use std::io;
use std::os::unix::io::AsRawFd;

/// The size of the terminal window in pixels, when the terminal reports one.
///
/// The probe is the `TIOCGWINSZ` ioctl, which carries a pixel width and a pixel
/// height beside the column count and the row count of the same window. Many
/// terminals leave the two pixel fields at zero, because the fields are older
/// than the terminals and nothing made them fill them in. A pane of Zellij and
/// a ttyd panel both answer with zeros, and so does a pseudo-terminal that
/// nobody ever sized.
///
/// The answer is `None` for such a terminal, and `None` for a failed ioctl. A
/// zero is no size, so both of the two fields must be above zero for the answer
/// to stand.
#[must_use]
pub fn terminal_pixels() -> Option<(u32, u32)> {
    #[repr(C)]
    struct Winsize {
        ws_row: libc::c_ushort,
        ws_col: libc::c_ushort,
        ws_xpixel: libc::c_ushort,
        ws_ypixel: libc::c_ushort,
    }

    let mut ws = Winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let fd = io::stdout().as_raw_fd();
    // SAFETY: TIOCGWINSZ is a standard ioctl that only reads the terminal window size
    // into the provided Winsize struct. It does not modify any other state and the
    // Winsize struct is properly initialized.
    let result = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };

    if result == 0 && ws.ws_xpixel > 0 && ws.ws_ypixel > 0 {
        Some((u32::from(ws.ws_xpixel), u32::from(ws.ws_ypixel)))
    } else {
        None
    }
}

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
    cell_pixels_of(terminal_pixels(), termsize::stdout_size())
}

/// The size of one character cell that a pair of probe answers measures.
///
/// The read of the terminal stands apart from this arithmetic, so a test names
/// the answer of a terminal without a terminal to name it with.
///
/// The rule about a zero lives here, and not in the probes, so that one test
/// covers it. `termsize` already refuses a size of zero columns or zero rows,
/// so no zero count reaches this function through [`cell_pixels`] today. The
/// guard stays because this function is where the crate states what a cell
/// measures, and the test of the guard is what keeps the rule true if a probe
/// ever changes.
///
/// # Arguments
/// * `reported_pixels` - The width and the height of the window in pixels, when
///   the terminal reports them.
/// * `reported_cells` - The columns and the rows of the same window, when a
///   probe measured them.
///
/// # Returns
/// The width and the height of one cell in pixels, or `None` when either probe
/// answered nothing, when either count is zero, or when either quotient is
/// zero.
fn cell_pixels_of(
    reported_pixels: Option<(u32, u32)>,
    reported_cells: Option<(u16, u16)>,
) -> Option<(u32, u32)> {
    let (pixels_wide, pixels_tall) = reported_pixels?;
    let (columns, rows) = reported_cells?;
    let columns = u32::from(columns);
    let rows = u32::from(rows);
    if columns == 0 || rows == 0 {
        return None;
    }

    let cell_width = pixels_wide / columns;
    let cell_height = pixels_tall / rows;
    if cell_width == 0 || cell_height == 0 {
        return None;
    }

    Some((cell_width, cell_height))
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

    #[test]
    fn a_window_that_no_probe_measured_measures_no_cell() {
        assert_eq!(
            cell_pixels_of(Some(REPORTED_PIXELS), None),
            None,
            "a pixel size with no column count and no row count has nothing to divide by"
        );
    }

    #[test]
    fn a_count_of_zero_measures_no_cell() {
        assert_eq!(
            cell_pixels_of(Some(REPORTED_PIXELS), Some((0, 24))),
            None,
            "no character of a line prints into zero columns, so zero columns divide nothing"
        );
        assert_eq!(
            cell_pixels_of(Some(REPORTED_PIXELS), Some((80, 0))),
            None,
            "a window of no rows shows no line, so zero rows divide nothing"
        );
    }

    #[test]
    fn a_cell_of_less_than_one_pixel_measures_no_cell() {
        assert_eq!(
            cell_pixels_of(Some((40, 480)), Some(REPORTED_CELLS)),
            None,
            "40 pixels over 80 columns is a cell of no width, and a cell of no width holds no pixel of an image"
        );
        assert_eq!(
            cell_pixels_of(Some((800, 12)), Some(REPORTED_CELLS)),
            None,
            "12 pixels over 24 rows is a cell of no height, and a cell of no height holds no pixel of an image"
        );
    }
}
