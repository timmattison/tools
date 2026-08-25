//! The size of the things a terminal draws an image into.
//!
//! A terminal draws text in a grid of character cells, and it draws an image
//! in pixels. A tool that puts an image on that grid therefore has to convert
//! between the two, and the conversion needs the size of one cell. This module
//! holds that measure and every size that comes off it.

use std::borrow::Cow;
use std::io;
use std::os::unix::io::AsRawFd;

use image::DynamicImage;

/// Estimated pixel width per terminal character cell.
/// Used as fallback when actual pixel dimensions cannot be queried via ioctl.
/// Value of 10px is typical for modern terminals with default fonts.
const ESTIMATED_CELL_WIDTH_PX: u32 = 10;

/// Estimated pixel height per terminal character cell.
/// Used as fallback when actual pixel dimensions cannot be queried via ioctl.
/// Value of 20px is typical for modern terminals with default fonts (roughly 2:1 aspect).
const ESTIMATED_CELL_HEIGHT_PX: u32 = 20;

/// Default Sixel output width in pixels when no size information is available.
/// Used as final fallback when both ioctl and character cell estimates fail.
const DEFAULT_SIXEL_WIDTH_PX: u32 = 800;

/// Default Sixel output height in pixels when no size information is available.
/// Used as final fallback when both ioctl and character cell estimates fail.
const DEFAULT_SIXEL_HEIGHT_PX: u32 = 600;

/// Horizontal margin factor for Sixel output (95% of terminal width).
/// Leaves some margin to avoid horizontal overflow.
///
/// The margin is one of the two bounds on the size of a Sixel image, and it is
/// not the bound that keeps the image inside the rows that a caller gives it.
/// See [`sixel_pixel_budget`], which takes the smaller of the margin and the
/// budget of the caller in character cells.
const SIXEL_HORIZONTAL_MARGIN: f64 = 0.95;

/// Vertical margin factor for Sixel output (90% of terminal height).
/// Leaves more vertical margin as some terminals have status bars or prompts.
///
/// The margin knows nothing about the header rows that a caller prints above
/// the image, and it is not the bound that keeps the prompt on the screen. See
/// [`sixel_pixel_budget`], which takes the smaller of the margin and the budget
/// of the caller in character cells.
const SIXEL_VERTICAL_MARGIN: f64 = 0.90;

/// The width of the terminal that [`terminal_cells`] assumes when it cannot
/// read the real width.
const FALLBACK_TERMINAL_COLS: u32 = 80;

/// The height of the terminal that [`terminal_cells`] assumes when it cannot
/// read the real height.
const FALLBACK_TERMINAL_ROWS: u32 = 24;

/// The size of the terminal that standard output writes to, in columns and then
/// rows.
///
/// The answer is 80 columns by 24 rows when no probe measured a terminal, and
/// that pair is not a guess about this machine. It is the size of the VT100,
/// every terminal emulator since opens a window of that size unless the user
/// says otherwise, and every tool that ever needed a fallback width took the
/// same pair. A window of that size is therefore the one a picture drawn
/// blind has the best chance of fitting.
///
/// The answer is never `None`, because a caller of this function draws into the
/// terminal whatever the terminal reports. It needs a number to lay a picture
/// out with, and a `None` only moves the same guess up into the caller, where
/// each caller would spell it differently. A caller that has a second way to
/// draw must not call this function: it must ask `termsize` itself, where a
/// terminal that reported nothing answers `None`, and then pick the way that
/// needs no size. [`cell_pixels`] is the same rule for the size of a cell.
///
/// The probe reads standard output, because a caller of this function writes
/// its picture there. `src/termsize/src/lib.rs` says why a size of zero columns
/// or zero rows is no answer, and the fallback stands for such a terminal too.
#[must_use]
pub fn terminal_cells() -> (u32, u32) {
    termsize::stdout_size().map_or(
        (FALLBACK_TERMINAL_COLS, FALLBACK_TERMINAL_ROWS),
        |(cols, rows)| (u32::from(cols), u32::from(rows)),
    )
}

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
pub(crate) fn terminal_pixels() -> Option<(u32, u32)> {
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
///
/// This is not [`cell_pixels_or_estimate`]. That function answers the same
/// question and never says `None`: it falls back to an estimate of the cell of
/// a typical terminal. The fallback is right for a tool with no second way to
/// draw, because an image at an estimated size beats no image. It is wrong for
/// a tool that has one, because an image drawn at a guessed size is worse than
/// the block characters that need no size at all.
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

/// The size of one character cell in pixels, with an estimate for a terminal
/// that reports none.
///
/// This is the measure that a tool takes when it must draw an image whatever
/// the terminal says, which is what `ic` does: a run of `ic` has no second way
/// to show the picture, so an image at an estimated size beats no image at all.
/// [`cell_pixels`] is the same question for a caller that does have a second
/// way. That one answers `None` for a terminal that reports no pixel size, and
/// the caller then draws the thing that needs no measure.
///
/// The estimate is 10 pixels by 20, which is about the cell of a modern
/// terminal at its default font, and the ratio of the two carries the shape of
/// a cell better than either number carries its size.
///
/// A window of very few pixels over very many columns divides down to a cell of
/// zero pixels, and this function hands that zero back rather than call it an
/// estimate. Every consumer of the answer therefore holds a floor of one, and
/// [`image_rows`] states that floor.
#[must_use]
pub(crate) fn cell_pixels_or_estimate() -> (u32, u32) {
    if let Some((total_px_w, total_px_h)) = terminal_pixels() {
        let (term_cols, term_rows) = terminal_cells();
        if term_cols > 0 && term_rows > 0 {
            return (total_px_w / term_cols, total_px_h / term_rows);
        }
    }
    (ESTIMATED_CELL_WIDTH_PX, ESTIMATED_CELL_HEIGHT_PX)
}

/// Returns the cell aspect ratio (height / width in pixels).
/// Uses actual terminal cell dimensions when available, falling back to estimates.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "a character cell is a few tens of pixels, far inside the exact range of f64"
)]
pub(crate) fn cell_aspect_ratio() -> f64 {
    let (cell_w, cell_h) = cell_pixels_or_estimate();
    cell_h as f64 / cell_w as f64
}

/// Convert character cell display dimensions to target pixel dimensions.
///
/// The caller reads the size of one cell from [`cell_pixels_or_estimate`], so
/// that one read covers every conversion of one image.
fn cells_to_pixels(cols: u32, rows: u32, cell_w: u32, cell_h: u32) -> (u32, u32) {
    (cols * cell_w, rows * cell_h)
}

/// Calculate display dimensions that preserve aspect ratio within the given bounds.
///
/// Terminal cells are typically ~2:1 (height:width in pixels), so we account for that
/// via the `cell_aspect` parameter (cell height / cell width in pixels). Callers
/// obtain this from [`cell_aspect_ratio`], which uses actual terminal
/// dimensions when available and falls back to an estimate of the cell. This
/// function works in terminal character cells, not pixels.
///
/// The casts from f64 to u32 are intentional - display dimensions are always positive
/// and will never exceed u32::MAX for any reasonable terminal size.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "display dimensions are always positive and fit in u32"
)]
pub(crate) fn calculate_aspect_preserving_size(
    img_width: u32,
    img_height: u32,
    max_width: Option<u32>,
    max_height: Option<u32>,
    preserve_aspect: bool,
    cell_aspect: f64,
) -> (Option<u32>, Option<u32>) {
    if !preserve_aspect {
        return (max_width, max_height);
    }

    // Guard against division by zero. Valid images always have height > 0,
    // but we handle this defensively to avoid panics on malformed input.
    if img_height == 0 {
        return (max_width, max_height);
    }

    match (max_width, max_height) {
        (Some(max_w), Some(max_h)) => {
            let effective_max_h_pixels = max_h as f64 * cell_aspect;
            let effective_max_w_pixels = max_w as f64;

            let img_aspect = img_width as f64 / img_height as f64;
            let box_aspect = effective_max_w_pixels / effective_max_h_pixels;

            if img_aspect > box_aspect {
                // Image is wider than box - constrain by width
                let display_width = max_w;
                let display_height = ((max_w as f64 / img_aspect) / cell_aspect).round() as u32;
                (Some(display_width), Some(display_height.max(1)))
            } else {
                // Image is taller than box - constrain by height
                let display_height = max_h;
                let display_width = (max_h as f64 * cell_aspect * img_aspect).round() as u32;
                (Some(display_width.max(1)), Some(display_height))
            }
        }
        (Some(w), None) => (Some(w), None),
        (None, Some(h)) => (None, Some(h)),
        (None, None) => (None, None),
    }
}

/// Downscale an image to fit the display pixel dimensions if it exceeds them.
///
/// Converts character cell display dimensions to pixel dimensions using either the
/// actual terminal pixel size (via ioctl) or an estimate of the cell, then resizes
/// the image if it's larger than the target. This prevents sending hundreds of megabytes
/// of pixel data to the terminal for very large images (e.g., panoramas).
///
/// Returns a borrowed reference to the original image when no downscaling is needed
/// (dimensions unspecified or image already fits), avoiding an unnecessary clone.
/// Returns an owned downscaled image when the original exceeds the target pixel dimensions.
#[must_use]
pub(crate) fn downscale_to_display_pixels<'a>(
    img: &'a DynamicImage,
    display_width: Option<u32>,
    display_height: Option<u32>,
) -> Cow<'a, DynamicImage> {
    let (target_pixel_w, target_pixel_h) = match (display_width, display_height) {
        (Some(cols), Some(rows)) => {
            let (cell_w, cell_h) = cell_pixels_or_estimate();
            cells_to_pixels(cols, rows, cell_w, cell_h)
        }
        _ => return Cow::Borrowed(img),
    };

    if img.width() <= target_pixel_w && img.height() <= target_pixel_h {
        return Cow::Borrowed(img);
    }

    Cow::Owned(img.resize(
        target_pixel_w,
        target_pixel_h,
        image::imageops::FilterType::Lanczos3,
    ))
}

/// Calculate pixel dimensions for Sixel output, preserving aspect ratio.
///
/// Unlike [`calculate_aspect_preserving_size`] which works in terminal character
/// cells, this function works directly in pixels for Sixel output. The aspect
/// ratio calculation is intentionally different because Sixel doesn't need to
/// account for cell aspect ratio.
///
/// The casts from f64 to u32 are intentional - pixel dimensions are always positive
/// and will never exceed u32::MAX for any reasonable display.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "pixel dimensions are always positive and fit in u32"
)]
pub(crate) fn calculate_sixel_dimensions(
    img_width: u32,
    img_height: u32,
    target_pixel_width: u32,
    target_pixel_height: u32,
    preserve_aspect: bool,
) -> (u32, u32) {
    if !preserve_aspect {
        return (target_pixel_width, target_pixel_height);
    }

    let img_aspect = img_width as f64 / img_height as f64;
    let target_aspect = target_pixel_width as f64 / target_pixel_height as f64;

    if img_aspect > target_aspect {
        // Image is wider - constrain by width
        let w = target_pixel_width;
        let h = (target_pixel_width as f64 / img_aspect) as u32;
        (w, h.max(1))
    } else {
        // Image is taller - constrain by height
        let h = target_pixel_height;
        let w = (target_pixel_height as f64 * img_aspect) as u32;
        (w.max(1), h)
    }
}

/// Give the size in pixels that a Sixel image can occupy.
///
/// Two bounds hold the image, and the image must obey both. The margin bounds
/// the image by the pixel size that the terminal reports, which keeps the image
/// off the edge of the screen. The budget of the caller bounds the image by a
/// count of character cells, which is the size of the terminal less the header
/// rows and the prompt row, or the `--width` and the `--height` that the user
/// asks for. The smaller of the two wins on each axis, so the header, the image
/// and the prompt all fit inside the terminal.
///
/// The two bounds come from different sources, and each one can be absent. A
/// terminal in Zellij or in ttyd reports no pixel size, and the budget of the
/// caller is then the only bound. An axis of the budget is absent when the user
/// gives only one of `--width` and `--height`, and the margin is then the only
/// bound on that axis. With no pixel size and only one axis of the budget there
/// is nothing to compute a size from, so a default size becomes the answer.
///
/// # Arguments
/// * `terminal_pixel_size` - The width and the height of the terminal in
///   pixels, when the terminal reports them.
/// * `cell_cols` - The width of the budget of the caller in character cells.
/// * `cell_rows` - The height of the budget of the caller in character cells.
/// * `cell_width_px` - The width of one character cell in pixels.
/// * `cell_height_px` - The height of one character cell in pixels.
///
/// # Returns
/// The width and the height of the budget in pixels. Each axis is 1 or more, so
/// a degenerate terminal cannot ask the encoder for an image of no size.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pixel dimensions are always positive and fit in u32"
)]
pub(crate) fn sixel_pixel_budget(
    terminal_pixel_size: Option<(u32, u32)>,
    cell_cols: Option<u32>,
    cell_rows: Option<u32>,
    cell_width_px: u32,
    cell_height_px: u32,
) -> (u32, u32) {
    // A count of cells that is absurdly large overflows the product. The
    // product saturates instead, and a saturated product bounds nothing.
    let cell_budget_px =
        |cells: Option<u32>, cell_px: u32| cells.map(|count| count.saturating_mul(cell_px));
    let cols_px = cell_budget_px(cell_cols, cell_width_px);
    let rows_px = cell_budget_px(cell_rows, cell_height_px);

    let (width_px, height_px) = if let Some((px_w, px_h)) = terminal_pixel_size {
        let margin_width_px = (f64::from(px_w) * SIXEL_HORIZONTAL_MARGIN) as u32;
        let margin_height_px = (f64::from(px_h) * SIXEL_VERTICAL_MARGIN) as u32;
        (
            cols_px.map_or(margin_width_px, |budget| margin_width_px.min(budget)),
            rows_px.map_or(margin_height_px, |budget| margin_height_px.min(budget)),
        )
    } else if let (Some(width_px), Some(height_px)) = (cols_px, rows_px) {
        (width_px, height_px)
    } else {
        (DEFAULT_SIXEL_WIDTH_PX, DEFAULT_SIXEL_HEIGHT_PX)
    };

    (width_px.max(1), height_px.max(1))
}

/// Calculate the number of terminal rows that an image fills.
///
/// The image height in pixels divides by the height of one character cell, and
/// the result rounds up, because a partial row still uses a full row.
///
/// # Arguments
/// * `height_px` - The height of the image in pixels.
/// * `cell_height_px` - The height of one character cell in pixels.
///
/// # Returns
/// The row count. The result is always 1 or more, so the caller never asks the
/// terminal for a movement of zero rows. A `cell_height_px` of 0 counts as 1,
/// because [`cell_pixels_or_estimate`] divides the terminal pixel height by the
/// row count and can give 0.
#[must_use]
pub(crate) fn image_rows(height_px: u32, cell_height_px: u32) -> u32 {
    height_px.div_ceil(cell_height_px.max(1)).max(1)
}

/// Calculate the number of terminal rows that a rendered image fills, for the
/// protocols that take the size of the image in character cells.
///
/// The Kitty graphics protocol and the iTerm2 inline image protocol both take a
/// width and a height in character cells, and both make their own decision when
/// one of the two is absent. There are four cases, and each one follows a rule
/// of the protocols:
///
/// * `display_height` is `Some(h)`, with or without a width. Both protocols
///   scale the image into the rows that the caller asks for, so the answer is
///   `h`.
/// * Only `display_width` is `Some(w)`. Both protocols then compute the other
///   dimension to keep the aspect ratio of the image. The rendered height in
///   pixels is `img_height_px * (w * cell_width_px) / img_width_px`.
/// * Neither is given. Both protocols render the image at its own pixel size,
///   so the answer comes from the pixel height of the image.
///
/// # Arguments
/// * `img_width_px` - The width of the image in pixels.
/// * `img_height_px` - The height of the image in pixels.
/// * `display_width` - The width that the caller asks for, in character cells.
/// * `display_height` - The height that the caller asks for, in character cells.
/// * `cell_width_px` - The width of one character cell in pixels.
/// * `cell_height_px` - The height of one character cell in pixels.
///
/// # Returns
/// The row count. The result is always 1 or more, so the caller never asks the
/// terminal for a movement of zero rows, which a terminal reads as one row. An
/// `img_width_px` of 0 gives no scale factor, so the width-only case falls back
/// to the pixel height of the image.
#[must_use]
pub(crate) fn image_rows_in_cells(
    img_width_px: u32,
    img_height_px: u32,
    display_width: Option<u32>,
    display_height: Option<u32>,
    cell_width_px: u32,
    cell_height_px: u32,
) -> u32 {
    if let Some(rows) = display_height {
        return rows.max(1);
    }

    match display_width {
        // The arithmetic runs in u64, because a wide image and a tall image
        // together overflow u32 long before they overflow u64.
        Some(cols) if img_width_px > 0 => {
            let rendered_height_px =
                u64::from(img_height_px) * u64::from(cols) * u64::from(cell_width_px)
                    / u64::from(img_width_px);
            image_rows(
                u32::try_from(rendered_height_px).unwrap_or(u32::MAX),
                cell_height_px,
            )
        }
        _ => image_rows(img_height_px, cell_height_px),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    // =========================================================================
    // Tests for calculate_aspect_preserving_size
    // =========================================================================

    /// Standard cell aspect ratio used in tests (typical terminal: cells ~2x tall as wide).
    const TEST_CELL_ASPECT: f64 = 2.0;

    #[test]
    fn aspect_preserving_returns_original_when_disabled() {
        let result =
            calculate_aspect_preserving_size(100, 100, Some(50), Some(50), false, TEST_CELL_ASPECT);
        assert_eq!(result, (Some(50), Some(50)));
    }

    #[test]
    fn aspect_preserving_handles_zero_height_defensively() {
        // Zero height should not panic (division by zero), returns original dimensions
        let result =
            calculate_aspect_preserving_size(100, 0, Some(50), Some(50), true, TEST_CELL_ASPECT);
        assert_eq!(result, (Some(50), Some(50)));
    }

    #[test]
    fn aspect_preserving_square_image_in_square_box() {
        // Square image (100x100) in square box (50x50)
        // With cell aspect ratio of 2:1, effective box is 50x100 pixels
        // Image aspect = 1.0, box aspect = 50/100 = 0.5
        // Image is wider than box, constrain by width
        let result =
            calculate_aspect_preserving_size(100, 100, Some(50), Some(50), true, TEST_CELL_ASPECT);
        // display_width = 50, display_height = (50/1.0)/2.0 = 25
        assert_eq!(result, (Some(50), Some(25)));
    }

    #[test]
    fn aspect_preserving_wide_image() {
        // Wide image (200x100) in square box (50x50)
        // Image aspect = 2.0
        // Constrain by width
        let result =
            calculate_aspect_preserving_size(200, 100, Some(50), Some(50), true, TEST_CELL_ASPECT);
        // display_width = 50, display_height = (50/2.0)/2.0 = 12.5 -> 13 (rounded)
        assert_eq!(result, (Some(50), Some(13)));
    }

    #[test]
    fn aspect_preserving_tall_image() {
        // Tall image (100x400) in square box (50x50)
        // Image aspect = 0.25
        // Constrain by height
        let result =
            calculate_aspect_preserving_size(100, 400, Some(50), Some(50), true, TEST_CELL_ASPECT);
        // display_height = 50, display_width = 50 * 2.0 * 0.25 = 25
        assert_eq!(result, (Some(25), Some(50)));
    }

    #[test]
    fn aspect_preserving_only_width_specified() {
        let result =
            calculate_aspect_preserving_size(100, 100, Some(50), None, true, TEST_CELL_ASPECT);
        assert_eq!(result, (Some(50), None));
    }

    #[test]
    fn aspect_preserving_only_height_specified() {
        let result =
            calculate_aspect_preserving_size(100, 100, None, Some(50), true, TEST_CELL_ASPECT);
        assert_eq!(result, (None, Some(50)));
    }

    #[test]
    fn aspect_preserving_no_dimensions_specified() {
        let result = calculate_aspect_preserving_size(100, 100, None, None, true, TEST_CELL_ASPECT);
        assert_eq!(result, (None, None));
    }

    #[test]
    fn aspect_preserving_minimum_dimension_is_one() {
        // Very wide image that would result in height < 1
        let result =
            calculate_aspect_preserving_size(10000, 1, Some(10), Some(10), true, TEST_CELL_ASPECT);
        // Should clamp height to at least 1
        assert!(result.1.unwrap() >= 1);

        // Very tall image that would result in width < 1
        let result =
            calculate_aspect_preserving_size(1, 10000, Some(10), Some(10), true, TEST_CELL_ASPECT);
        // Should clamp width to at least 1
        assert!(result.0.unwrap() >= 1);
    }

    // =========================================================================
    // Tests for calculate_sixel_dimensions
    // =========================================================================

    #[test]
    fn sixel_returns_target_when_aspect_disabled() {
        let result = calculate_sixel_dimensions(100, 100, 800, 600, false);
        assert_eq!(result, (800, 600));
    }

    #[test]
    fn sixel_square_image_in_landscape_target() {
        // Square image (100x100) in landscape target (800x600)
        // Image aspect = 1.0, target aspect = 800/600 = 1.33
        // Image is taller than target, constrain by height
        let result = calculate_sixel_dimensions(100, 100, 800, 600, true);
        // h = 600, w = 600 * 1.0 = 600
        assert_eq!(result, (600, 600));
    }

    #[test]
    fn sixel_wide_image_in_landscape_target() {
        // Wide image (1600x900, 16:9) in landscape target (800x600)
        // Image aspect = 1.78, target aspect = 1.33
        // Image is wider, constrain by width
        let result = calculate_sixel_dimensions(1600, 900, 800, 600, true);
        // w = 800, h = 800 / 1.78 = 450
        assert_eq!(result, (800, 450));
    }

    #[test]
    fn sixel_tall_image_in_landscape_target() {
        // Tall image (100x400) in landscape target (800x600)
        // Image aspect = 0.25, target aspect = 1.33
        // Image is taller, constrain by height
        let result = calculate_sixel_dimensions(100, 400, 800, 600, true);
        // h = 600, w = 600 * 0.25 = 150
        assert_eq!(result, (150, 600));
    }

    #[test]
    fn sixel_minimum_dimension_is_one() {
        // Very wide image
        let result = calculate_sixel_dimensions(10000, 1, 100, 100, true);
        assert!(result.1 >= 1);

        // Very tall image
        let result = calculate_sixel_dimensions(1, 10000, 100, 100, true);
        assert!(result.0 >= 1);
    }

    #[test]
    fn sixel_handles_zero_height_defensively() {
        // Zero height should produce infinity aspect, which comparing > target_aspect
        // will be true, so we'll constrain by width
        // w = 100, h = 100 / infinity = 0, clamped to 1
        let result = calculate_sixel_dimensions(100, 0, 100, 100, true);
        assert_eq!(result.1, 1); // Height should be clamped to 1
    }

    // =========================================================================
    // Tests for sixel_pixel_budget
    // =========================================================================

    /// The width of the terminal in pixels that the budget tests use. Over 80
    /// columns it gives cells of 12 pixels, because 1000 / 80 is 12.5.
    const TEST_TERM_WIDTH_PX: u32 = 1000;

    /// The height of the terminal in pixels that the budget tests use. Over 24
    /// rows it gives cells of 47 pixels, because 1150 / 24 is 47.9.
    const TEST_TERM_HEIGHT_PX: u32 = 1150;

    /// The width of one character cell of the terminal in the budget tests.
    const TEST_TERM_CELL_WIDTH_PX: u32 = 12;

    /// The height of one character cell of the terminal in the budget tests.
    const TEST_TERM_CELL_HEIGHT_PX: u32 = 47;

    /// The width that the margin gives, which is 1000 * 0.95.
    const TEST_MARGIN_WIDTH_PX: u32 = 950;

    /// The height that the margin gives, which is 1150 * 0.90.
    const TEST_MARGIN_HEIGHT_PX: u32 = 1035;

    /// Call `sixel_pixel_budget` with the terminal of the budget tests.
    fn budget_with_cells(cols: Option<u32>, rows: Option<u32>) -> (u32, u32) {
        sixel_pixel_budget(
            Some((TEST_TERM_WIDTH_PX, TEST_TERM_HEIGHT_PX)),
            cols,
            rows,
            TEST_TERM_CELL_WIDTH_PX,
            TEST_TERM_CELL_HEIGHT_PX,
        )
    }

    #[test]
    fn sixel_pixel_budget_holds_the_image_inside_the_row_budget() {
        // The terminal has 24 rows of 47 pixels, so the margin gives 1035
        // pixels, which is 23 rows. One header row, 23 image rows and one
        // prompt row are 25 rows in a terminal of 24. The caller therefore
        // gives the image 22 rows, which is 1034 pixels, and that budget must
        // win over the margin.
        let (_, height) = budget_with_cells(None, Some(22));
        assert_eq!(height, 1034);
    }

    #[test]
    fn sixel_pixel_budget_holds_the_image_inside_the_column_budget() {
        // The terminal has 80 columns of 12 pixels, so the margin gives 950
        // pixels. The caller keeps 2 columns for the chrome of the terminal and
        // gives the image 78 columns, which is 936 pixels.
        let (width, _) = budget_with_cells(Some(78), None);
        assert_eq!(width, 936);
    }

    #[test]
    fn sixel_pixel_budget_keeps_the_margin_when_the_cell_budget_is_larger() {
        // The margin keeps the image off the edge of the screen. A cell budget
        // larger than the screen must not push the image past that edge.
        assert_eq!(
            budget_with_cells(Some(200), Some(100)),
            (TEST_MARGIN_WIDTH_PX, TEST_MARGIN_HEIGHT_PX)
        );
    }

    #[test]
    fn sixel_pixel_budget_keeps_the_margin_on_an_axis_with_no_cell_budget() {
        // The user gives only --width, so there is no budget for the rows.
        assert_eq!(
            budget_with_cells(Some(20), None),
            (240, TEST_MARGIN_HEIGHT_PX)
        );
        // The user gives only --height, so there is no budget for the columns.
        assert_eq!(
            budget_with_cells(None, Some(10)),
            (TEST_MARGIN_WIDTH_PX, 470)
        );
        // Neither axis has a budget, so the margin stands on both.
        assert_eq!(
            budget_with_cells(None, None),
            (TEST_MARGIN_WIDTH_PX, TEST_MARGIN_HEIGHT_PX)
        );
    }

    #[test]
    fn sixel_pixel_budget_uses_the_cell_budget_without_a_pixel_size() {
        // Zellij and ttyd report no pixel size, so the cells give the budget.
        assert_eq!(
            sixel_pixel_budget(
                None,
                Some(80),
                Some(24),
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            (800, 480)
        );
    }

    #[test]
    fn sixel_pixel_budget_falls_back_to_the_default_size() {
        // Without a pixel size and without both axes of the cell budget there
        // is nothing to compute a size from.
        let default_size = (DEFAULT_SIXEL_WIDTH_PX, DEFAULT_SIXEL_HEIGHT_PX);
        assert_eq!(
            sixel_pixel_budget(
                None,
                Some(80),
                None,
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            default_size
        );
        assert_eq!(
            sixel_pixel_budget(
                None,
                None,
                Some(24),
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            default_size
        );
        assert_eq!(
            sixel_pixel_budget(None, None, None, TEST_CELL_WIDTH_PX, TEST_CELL_HEIGHT_PX),
            default_size
        );
    }

    #[test]
    fn sixel_pixel_budget_never_gives_an_axis_of_zero() {
        // A budget of zero pixels asks the encoder for an image of no size, so
        // each axis keeps a floor of one pixel.
        assert_eq!(budget_with_cells(Some(0), Some(0)), (1, 1));
        assert_eq!(
            sixel_pixel_budget(
                Some((TEST_TERM_WIDTH_PX, TEST_TERM_HEIGHT_PX)),
                Some(80),
                Some(24),
                0,
                0
            ),
            (1, 1)
        );
        assert_eq!(
            sixel_pixel_budget(
                None,
                Some(0),
                Some(0),
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            (1, 1)
        );
        // A terminal of one pixel gives a margin of less than one pixel.
        assert_eq!(sixel_pixel_budget(Some((1, 1)), None, None, 1, 1), (1, 1));
    }

    #[test]
    fn sixel_pixel_budget_survives_a_cell_budget_that_overflows() {
        // A terminal that reports an absurd size must not panic the tool. The
        // product saturates, so the margin wins.
        assert_eq!(
            budget_with_cells(Some(u32::MAX), Some(u32::MAX)),
            (TEST_MARGIN_WIDTH_PX, TEST_MARGIN_HEIGHT_PX)
        );
        assert_eq!(
            sixel_pixel_budget(None, Some(u32::MAX), Some(u32::MAX), u32::MAX, u32::MAX),
            (u32::MAX, u32::MAX)
        );
    }

    // =========================================================================
    // Tests verifying constants are reasonable
    // =========================================================================

    #[test]
    #[allow(
        clippy::cast_precision_loss,
        reason = "a character cell is a few tens of pixels, far inside the exact range of f64"
    )]
    fn constants_have_reasonable_values() {
        // Derived cell aspect ratio (from fallback constants) should be reasonable (1.5 to 3.0)
        let fallback_aspect = ESTIMATED_CELL_HEIGHT_PX as f64 / ESTIMATED_CELL_WIDTH_PX as f64;
        assert!(fallback_aspect > 1.0);
        assert!(fallback_aspect < 4.0);

        // Cell pixel estimates should be positive and reasonable
        const { assert!(ESTIMATED_CELL_WIDTH_PX >= 6) };
        const { assert!(ESTIMATED_CELL_WIDTH_PX <= 20) };
        const { assert!(ESTIMATED_CELL_HEIGHT_PX >= 12) };
        const { assert!(ESTIMATED_CELL_HEIGHT_PX <= 40) };

        // Default sixel dimensions should be reasonable screen sizes
        const { assert!(DEFAULT_SIXEL_WIDTH_PX >= 640) };
        const { assert!(DEFAULT_SIXEL_HEIGHT_PX >= 480) };

        // Margins should be between 0.5 and 1.0
        const { assert!(SIXEL_HORIZONTAL_MARGIN > 0.5) };
        const { assert!(SIXEL_HORIZONTAL_MARGIN <= 1.0) };
        const { assert!(SIXEL_VERTICAL_MARGIN > 0.5) };
        const { assert!(SIXEL_VERTICAL_MARGIN <= 1.0) };
    }

    // =========================================================================
    // Tests for cells_to_pixels
    // =========================================================================

    #[test]
    fn cells_to_pixels_basic() {
        assert_eq!(cells_to_pixels(80, 24, 10, 20), (800, 480));
    }

    #[test]
    fn cells_to_pixels_single_cell() {
        assert_eq!(cells_to_pixels(1, 1, 10, 20), (10, 20));
    }

    #[test]
    fn cells_to_pixels_custom_cell_dimensions() {
        assert_eq!(cells_to_pixels(40, 12, 16, 32), (640, 384));
    }

    // =========================================================================
    // Tests for downscale_to_display_pixels
    // =========================================================================

    /// Helper to create a test image of the given dimensions.
    fn make_test_image(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
    }

    #[test]
    fn downscale_returns_borrowed_when_no_dimensions() {
        let img = make_test_image(100, 100);
        let result = downscale_to_display_pixels(&img, None, None);
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn downscale_returns_borrowed_when_only_width() {
        let img = make_test_image(100, 100);
        let result = downscale_to_display_pixels(&img, Some(50), None);
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn downscale_returns_borrowed_when_only_height() {
        let img = make_test_image(100, 100);
        let result = downscale_to_display_pixels(&img, None, Some(50));
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn downscale_returns_borrowed_when_image_fits() {
        // With estimated cell dimensions (10x20), 80 cols x 24 rows = 800x480 pixels.
        // A 100x100 image fits within that, so no downscale needed.
        // Note: in CI/test environments, terminal_pixels() returns None,
        // so the estimate of the cell is used.
        let img = make_test_image(100, 100);
        let result = downscale_to_display_pixels(&img, Some(80), Some(24));
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result.width(), 100);
        assert_eq!(result.height(), 100);
    }

    #[test]
    fn downscale_shrinks_oversized_image() {
        // 10 cols x 5 rows, pixel target depends on actual cell dimensions
        let (cell_w, cell_h) = cell_pixels_or_estimate();
        let (max_w, max_h) = cells_to_pixels(10, 5, cell_w, cell_h);
        // Image must be larger than the target to trigger downscaling
        let img = make_test_image(max_w * 5, max_h * 5);
        let result = downscale_to_display_pixels(&img, Some(10), Some(5));
        assert!(matches!(result, Cow::Owned(_)));
        assert!(result.width() <= max_w);
        assert!(result.height() <= max_h);
    }

    #[test]
    #[allow(
        clippy::cast_precision_loss,
        reason = "the dimensions of a test image are a few hundred pixels, far inside the exact range of f64"
    )]
    fn downscale_preserves_aspect_ratio() {
        // Wide image with 5:1 aspect ratio, larger than any reasonable target
        let (cell_w, cell_h) = cell_pixels_or_estimate();
        let (max_w, max_h) = cells_to_pixels(10, 5, cell_w, cell_h);
        let img = make_test_image(max_w * 10, max_h * 2);
        let result = downscale_to_display_pixels(&img, Some(10), Some(5));
        assert!(matches!(result, Cow::Owned(_)));
        assert!(result.width() <= max_w);
        assert!(result.height() <= max_h);
        // Aspect ratio should be roughly maintained (5:1)
        let aspect = result.width() as f64 / result.height() as f64;
        assert!(
            (aspect - 5.0).abs() < 0.5,
            "aspect ratio {aspect} should be close to 5.0"
        );
    }

    #[test]
    fn downscale_handles_panoramic_image() {
        // Very large panorama, target depends on actual cell dimensions
        let (cell_w, cell_h) = cell_pixels_or_estimate();
        let (max_w, max_h) = cells_to_pixels(80, 24, cell_w, cell_h);
        let img = make_test_image(16384, 4096);
        let result = downscale_to_display_pixels(&img, Some(80), Some(24));
        assert!(matches!(result, Cow::Owned(_)));
        assert!(result.width() <= max_w);
        assert!(result.height() <= max_h);
    }

    // =========================================================================
    // Tests for image_rows
    // =========================================================================

    #[test]
    fn image_rows_divides_an_exact_multiple() {
        assert_eq!(image_rows(100, 20), 5);
        assert_eq!(image_rows(20, 20), 1);
    }

    #[test]
    fn image_rows_rounds_up_a_partial_row() {
        // A partial row still uses a full row.
        assert_eq!(image_rows(101, 20), 6);
        assert_eq!(image_rows(21, 20), 2);
        assert_eq!(image_rows(1, 20), 1);
    }

    #[test]
    fn image_rows_never_gives_zero() {
        // A movement of zero rows means one row to a terminal, so the row count
        // must stay at 1 or more.
        assert_eq!(image_rows(0, 20), 1);
        assert_eq!(image_rows(0, 0), 1);
    }

    #[test]
    fn image_rows_survives_a_zero_cell_height() {
        // cell_pixels_or_estimate divides the terminal pixel height by the row
        // count, so it can give 0. A cell height of 0 counts as 1 pixel.
        assert_eq!(image_rows(100, 0), 100);
    }

    // =========================================================================
    // Tests for image_rows_in_cells
    // =========================================================================

    /// The width of one character cell in the tests below, in pixels.
    const TEST_CELL_WIDTH_PX: u32 = 10;

    /// The height of one character cell in the tests below, in pixels.
    const TEST_CELL_HEIGHT_PX: u32 = 20;

    #[test]
    fn image_rows_in_cells_takes_a_given_height() {
        // Both protocols scale the image into the rows that the caller asks
        // for, so a given height is the answer, with or without a width.
        assert_eq!(
            image_rows_in_cells(
                100,
                100,
                Some(10),
                Some(5),
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            5
        );
        assert_eq!(
            image_rows_in_cells(
                100,
                100,
                None,
                Some(7),
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            7
        );
    }

    #[test]
    fn image_rows_in_cells_keeps_the_aspect_ratio_of_a_width() {
        // A width of 10 cells is 100 pixels. The image is twice as wide as it
        // is tall, so it renders 50 pixels high, which is 3 rows of 20 pixels.
        assert_eq!(
            image_rows_in_cells(
                100,
                50,
                Some(10),
                None,
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            3
        );
        // A width of 20 cells is 200 pixels, which doubles the image to 200
        // pixels high, which is 10 rows.
        assert_eq!(
            image_rows_in_cells(
                100,
                100,
                Some(20),
                None,
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            10
        );
    }

    #[test]
    fn image_rows_in_cells_falls_back_to_the_pixel_height() {
        // With no width and no height both protocols render the image at its
        // own pixel size.
        assert_eq!(
            image_rows_in_cells(
                100,
                100,
                None,
                None,
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            5
        );
        // A partial row still uses a full row.
        assert_eq!(
            image_rows_in_cells(
                100,
                101,
                None,
                None,
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            6
        );
    }

    #[test]
    fn image_rows_in_cells_survives_a_zero_image_width() {
        // A width of zero pixels gives no scale factor, so the width-only case
        // falls back to the pixel height of the image.
        assert_eq!(
            image_rows_in_cells(
                0,
                100,
                Some(10),
                None,
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            5
        );
    }

    #[test]
    fn image_rows_in_cells_never_gives_zero() {
        // A movement of zero rows means one row to a terminal, so the row count
        // must stay at 1 or more.
        assert_eq!(
            image_rows_in_cells(
                100,
                100,
                None,
                Some(0),
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            1
        );
        assert_eq!(
            image_rows_in_cells(100, 0, None, None, TEST_CELL_WIDTH_PX, TEST_CELL_HEIGHT_PX),
            1
        );
        assert_eq!(
            image_rows_in_cells(
                100,
                1,
                Some(1),
                None,
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            1
        );
    }
}
