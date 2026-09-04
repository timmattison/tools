//! The size of the things a terminal draws an image into.
//!
//! A terminal draws text in a grid of character cells, and it draws an image
//! in pixels. A tool that puts an image on that grid therefore has to convert
//! between the two, and the conversion needs the size of one cell. This module
//! holds that measure and every size that comes off it.

use std::borrow::Cow;

use image::DynamicImage;
use termsize::Window;

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

/// The width of the terminal that [`cells_of`] assumes for a run that measured
/// no window.
const FALLBACK_TERMINAL_COLS: u32 = 80;

/// The height of the terminal that [`cells_of`] assumes for a run that measured
/// no window.
const FALLBACK_TERMINAL_ROWS: u32 = 24;

/// The size of the window that a tool draws its picture into, in columns and
/// then rows.
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
/// draw must not call this function: it must call [`termsize::drawing_window`],
/// where a run that measured no terminal answers `None`, and then pick the way
/// that needs no size. [`cell_pixels`] is the same rule for the size of a cell.
///
/// The probe reads standard output, then standard error, then standard input,
/// and then the controlling terminal. The picture goes to standard output, so
/// that descriptor comes first. `/dev/tty` comes last, because a standard output
/// that somebody captured is no proof that there is no terminal: a caller that
/// keeps the bytes of a run in a file still sits at the terminal that the
/// picture appears on. GitHub issue #350 reports what the read of standard
/// output alone did to such a run. `ic` measured nothing, drew the image at the
/// guessed size of a cell, and then reserved rows that the image did not cover.
///
/// `src/termsize/src/lib.rs` says why a size of zero columns or zero rows is no
/// answer, and the fallback stands for such a terminal too.
#[must_use]
pub fn terminal_cells() -> (u32, u32) {
    cells_of(termsize::drawing_window())
}

/// The size of the window in character cells, with the fallback for a run that
/// measured no terminal.
///
/// The read of the terminal stands apart from this arithmetic, so a test states
/// the answer of a probe without a terminal to state it with. Under `cargo test`
/// the standard output of a test binary is a pipe, so a probe reaches the
/// controlling terminal and measures the window of the person who typed the
/// command. A test that called a probe would therefore assert on the size of
/// that window.
///
/// [`terminal_cells`] probes for the window. A writer of `draw` measured one
/// window already, and it hands that window over instead, so the picture and
/// the rows that the cursor contract reserves below it come off one read.
///
/// # Arguments
/// * `window` - The window that the probe measured, or `None` when the probe
///   measured none.
///
/// # Returns
/// The number of columns and then the number of rows of that window, or
/// [`FALLBACK_TERMINAL_COLS`] by [`FALLBACK_TERMINAL_ROWS`] when the probe
/// measured no window.
pub(crate) fn cells_of(window: Option<Window>) -> (u32, u32) {
    window.map_or((FALLBACK_TERMINAL_COLS, FALLBACK_TERMINAL_ROWS), |window| {
        let (columns, rows) = window.cells();
        (u32::from(columns), u32::from(rows))
    })
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
/// The probe reads the descriptors that the picture goes to, and then the
/// controlling terminal. [`terminal_cells`] says why that is the order.
///
/// The answer is `None` when the terminal reports no pixel size. A pane of
/// Zellij reports none, a ttyd panel reports none, and a terminal that carries
/// no window reports none. A caller that gets `None` holds no measure of a
/// cell, and it must then draw something that needs no measure. `krt` draws
/// block characters.
///
/// A quotient of zero pixels is no size either, so it gives `None` as well. A
/// count of zero cannot reach the division, because `termsize::Window` makes no
/// window of zero columns and no window of zero rows. See
/// `src/termsize/src/lib.rs`.
///
/// This is not [`cell_pixels_or_estimate_of`]. That function answers the same
/// question and never says `None`: it falls back to an estimate of the cell of
/// a typical terminal. The fallback is right for a tool with no second way to
/// draw, because an image at an estimated size beats no image. It is wrong for
/// a tool that has one, because an image drawn at a guessed size is worse than
/// the block characters that need no size at all.
#[must_use]
pub fn cell_pixels() -> Option<(u32, u32)> {
    cell_pixels_of(termsize::drawing_window())
}

/// The size of one character cell that one window measures.
///
/// The read of the terminal stands apart from this arithmetic, so a test states
/// the answer of a probe without a terminal to state it with.
///
/// The cells and the pixels of one window come off one file descriptor in one
/// ioctl, so this division always divides the pixels of a window by the cells
/// of that same window. The two halves came from two probes before, and the
/// pixel probe read standard output alone. A run whose standard output was a
/// pipe therefore measured no pixel size, and `ic` drew at a guess.
///
/// # Arguments
/// * `window` - The window that the probe measured, or `None` when the probe
///   measured none.
///
/// # Returns
/// The width and the height of one cell in pixels, or `None` when the probe
/// measured no window, when the terminal reports no pixel size, or when either
/// quotient is zero.
fn cell_pixels_of(window: Option<Window>) -> Option<(u32, u32)> {
    let (pixels_wide, pixels_tall) = window_pixels(window)?;
    let (columns, rows) = window?.cells();

    // A count of zero used to fail a guard here. `termsize::Window` holds that
    // rule now, and it makes no window of zero columns and no window of zero
    // rows, so no count of zero reaches this division. The tests of that rule
    // live in `src/termsize/src/lib.rs`.
    let cell_width = pixels_wide / u32::from(columns);
    let cell_height = pixels_tall / u32::from(rows);
    if cell_width == 0 || cell_height == 0 {
        return None;
    }

    Some((cell_width, cell_height))
}

/// The size of the window in pixels, when the terminal reports one.
///
/// The ioctl answers in `u16` and every size of this module is a `u32`, so this
/// function widens the pair. It is the one place that does, so no caller of the
/// module holds two shapes of one measure.
///
/// # Arguments
/// * `window` - The window that the probe measured, or `None` when the probe
///   measured none.
///
/// # Returns
/// The width and then the height of the window in pixels, or `None` when the
/// probe measured no window and when the terminal reports no pixel size. A pane
/// of Zellij reports none, and a ttyd panel reports none.
pub(crate) fn window_pixels(window: Option<Window>) -> Option<(u32, u32)> {
    let (pixels_wide, pixels_tall) = window?.pixels()?;
    Some((u32::from(pixels_wide), u32::from(pixels_tall)))
}

/// The size of one character cell in pixels, with the estimate for a window
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
/// # Arguments
/// * `window` - The window that the probe measured, or `None` when the probe
///   measured none.
///
/// # Returns
/// The width and the height of one character cell in pixels. Both numbers are
/// above zero, because [`cell_pixels_of`] refuses a quotient of zero and the
/// estimate then stands.
pub(crate) fn cell_pixels_or_estimate_of(window: Option<Window>) -> (u32, u32) {
    cell_pixels_of(window).unwrap_or((ESTIMATED_CELL_WIDTH_PX, ESTIMATED_CELL_HEIGHT_PX))
}

/// The shape of one character cell, as its height over its width.
///
/// A character cell is taller than it is wide, so an image of a given number of
/// columns and rows is not the shape that those two counts suggest. This ratio
/// carries the difference, and [`calculate_aspect_preserving_size`] takes it.
///
/// The size of the cell arrives from the caller, and this function reads no
/// terminal. One writer of `draw` reads the window one time and then hands the
/// same measure to every step of one image, so no two steps of one image can
/// name two terminals.
///
/// # Arguments
/// * `cell_width_px` - The width of one character cell in pixels.
/// * `cell_height_px` - The height of one character cell in pixels.
///
/// # Returns
/// The height of the cell over its width. A typical terminal gives about 2.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "a character cell is a few tens of pixels, far inside the exact range of f64"
)]
pub(crate) fn cell_aspect_of(cell_width_px: u32, cell_height_px: u32) -> f64 {
    cell_height_px as f64 / cell_width_px as f64
}

/// Convert character cell display dimensions to target pixel dimensions.
///
/// The caller reads the size of one cell from [`cell_pixels_or_estimate_of`], so
/// that one read covers every conversion of one image.
fn cells_to_pixels(cols: u32, rows: u32, cell_w: u32, cell_h: u32) -> (u32, u32) {
    (cols * cell_w, rows * cell_h)
}

/// Calculate display dimensions that preserve aspect ratio within the given bounds.
///
/// Terminal cells are typically ~2:1 (height:width in pixels), so we account for that
/// via the `cell_aspect` parameter (cell height / cell width in pixels). Callers
/// obtain this from [`cell_aspect_of`], which takes the cell that
/// [`cell_pixels_or_estimate_of`] measured. This function works in terminal
/// character cells, not pixels.
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
/// The function converts the display size in character cells to a size in
/// pixels, and it then resizes the image when the image is larger than that
/// target. This keeps hundreds of megabytes of pixel data off the terminal for a
/// very large image, such as a panorama.
///
/// The size of the cell arrives from the caller, and this function reads no
/// terminal. One writer of `draw` reads the window one time and hands the same
/// measure to every step of one image, and a test states a cell without a
/// terminal to state it with.
///
/// # Arguments
/// * `img` - The image to draw.
/// * `display_width` - The width that the caller asks for, in character cells.
/// * `display_height` - The height that the caller asks for, in character cells.
/// * `cell_width_px` - The width of one character cell in pixels.
/// * `cell_height_px` - The height of one character cell in pixels.
///
/// # Returns
/// A borrowed reference to the image when no downscale is necessary, which is
/// the case when the caller states no size and when the image already fits. That
/// borrow keeps a copy of the whole image off the heap. An owned and smaller
/// image comes back when the image is larger than the target.
#[must_use]
pub(crate) fn downscale_to_display_pixels<'a>(
    img: &'a DynamicImage,
    display_width: Option<u32>,
    display_height: Option<u32>,
    cell_width_px: u32,
    cell_height_px: u32,
) -> Cow<'a, DynamicImage> {
    let (target_pixel_w, target_pixel_h) = match (display_width, display_height) {
        (Some(cols), Some(rows)) => cells_to_pixels(cols, rows, cell_width_px, cell_height_px),
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
/// terminal for a movement of zero rows. A `cell_height_px` of 0 counts as 1.
/// No caller hands one over today, because [`cell_pixels_or_estimate_of`] takes
/// the estimate for a window that divides down to a cell of no pixels. The floor
/// stays because a division by zero panics, and the arithmetic of this module
/// must not.
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

    /// The columns and the rows of the window that these tests measure.
    const REPORTED_CELLS: (u16, u16) = (80, 24);

    /// The pixel size of that same window. 800 pixels over 80 columns is a cell
    /// 10 pixels wide, and 480 pixels over 24 rows is a cell 20 pixels tall.
    const REPORTED_PIXELS: (u16, u16) = (800, 480);

    /// The measured cell of a window of [`REPORTED_CELLS`] and
    /// [`REPORTED_PIXELS`].
    const REPORTED_CELL: (u32, u32) = (10, 20);

    /// The columns and the rows of a second window. Neither number is the one
    /// of the fallback, so a test that finds this size knows that the answer
    /// came off the window and not off the fallback.
    const OTHER_CELLS: (u16, u16) = (132, 43);

    /// The pixel size of a window of a high pixel density. 1600 pixels over 80
    /// columns is a cell 20 pixels wide, and 960 pixels over 24 rows is a cell
    /// 40 pixels tall. Neither number is the one of the estimate, so a test that
    /// finds this cell knows that the answer came off the window.
    const DENSE_PIXELS: (u16, u16) = (1600, 960);

    /// The measured cell of a window of [`REPORTED_CELLS`] and [`DENSE_PIXELS`].
    const DENSE_CELL: (u32, u32) = (20, 40);

    /// The window that a terminal of a stated size reports.
    ///
    /// # Arguments
    /// * `cells` - The columns and the rows of the window.
    /// * `pixels` - The width and the height of the same window in pixels, when
    ///   the terminal reports them.
    ///
    /// # Returns
    /// The window, inside the `Option` that every function under test takes.
    ///
    /// # Panics
    /// Panics when the columns or the rows are zero. A test that measures a
    /// window must state one, and a test that received `None` here would assert
    /// on the answer for a run that measured no terminal at all.
    fn window(cells: (u16, u16), pixels: Option<(u16, u16)>) -> Option<Window> {
        let (columns, rows) = cells;
        Some(
            Window::measured(columns, rows, pixels)
                .expect("a test that measures a window must state a window of a real size"),
        )
    }

    #[test]
    fn a_window_gives_its_own_cells_and_a_run_that_measured_none_gives_the_fallback() {
        assert_eq!(
            cells_of(window(OTHER_CELLS, None)),
            (u32::from(OTHER_CELLS.0), u32::from(OTHER_CELLS.1)),
            "the answer is the size of the window that the probe measured"
        );
        assert_eq!(
            cells_of(None),
            (FALLBACK_TERMINAL_COLS, FALLBACK_TERMINAL_ROWS),
            "a run that measured no terminal takes the size of a VT100, which is the window a picture drawn blind fits best"
        );
    }

    #[test]
    fn a_reported_pixel_size_measures_one_cell_and_no_pixel_size_measures_nothing() {
        assert_eq!(
            cell_pixels_of(window(REPORTED_CELLS, Some(REPORTED_PIXELS))),
            Some(REPORTED_CELL),
            "800 pixels over 80 columns is a cell 10 pixels wide, and 480 pixels over 24 rows is a cell 20 pixels tall"
        );
        assert_eq!(
            cell_pixels_of(window(REPORTED_CELLS, None)),
            None,
            "a pane of Zellij and a ttyd panel report their cells and no pixel size, so there is nothing to divide and no cell to measure"
        );
    }

    #[test]
    fn a_window_that_no_probe_measured_measures_no_cell() {
        assert_eq!(
            cell_pixels_of(None),
            None,
            "a run that measured no terminal holds no pixel size and no cell count, so it measures no cell"
        );
    }

    #[test]
    fn a_count_of_zero_is_no_window_at_all() {
        // This arithmetic carried a guard against a count of zero before.
        // `termsize::Window` carries that rule now, so neither case below can
        // reach the division: a zero makes no window. The rest of the tests of
        // the rule live in `src/termsize/src/lib.rs`.
        assert_eq!(
            Window::measured(0, REPORTED_CELLS.1, Some(REPORTED_PIXELS)),
            None,
            "no character of a line prints into zero columns, so zero columns divide nothing"
        );
        assert_eq!(
            Window::measured(REPORTED_CELLS.0, 0, Some(REPORTED_PIXELS)),
            None,
            "a window of no rows shows no line, so zero rows divide nothing"
        );
    }

    #[test]
    fn a_cell_of_less_than_one_pixel_measures_no_cell() {
        assert_eq!(
            cell_pixels_of(window(REPORTED_CELLS, Some((40, REPORTED_PIXELS.1)))),
            None,
            "40 pixels over 80 columns is a cell of no width, and a cell of no width holds no pixel of an image"
        );
        assert_eq!(
            cell_pixels_of(window(REPORTED_CELLS, Some((REPORTED_PIXELS.0, 12)))),
            None,
            "12 pixels over 24 rows is a cell of no height, and a cell of no height holds no pixel of an image"
        );
    }

    #[test]
    fn a_window_carries_its_pixel_size_and_a_window_of_no_pixels_carries_none() {
        assert_eq!(
            window_pixels(window(REPORTED_CELLS, Some(REPORTED_PIXELS))),
            Some((
                u32::from(REPORTED_PIXELS.0),
                u32::from(REPORTED_PIXELS.1)
            )),
            "the answer is the pixel size that the terminal reported, in the width that the arithmetic of this module works in"
        );
        assert_eq!(
            window_pixels(window(REPORTED_CELLS, None)),
            None,
            "a pane of Zellij and a ttyd panel report no pixel size, so a Sixel image takes its bound from the cells alone"
        );
        assert_eq!(
            window_pixels(None),
            None,
            "a run that measured no terminal measured no pixel size either"
        );
    }

    #[test]
    fn the_estimate_stands_for_every_window_that_measures_no_cell() {
        assert_eq!(
            cell_pixels_or_estimate_of(window(REPORTED_CELLS, Some(DENSE_PIXELS))),
            DENSE_CELL,
            "a terminal that reports a pixel size measures the cell, and the estimate stands aside"
        );
        assert_eq!(
            cell_pixels_or_estimate_of(window(REPORTED_CELLS, None)),
            (ESTIMATED_CELL_WIDTH_PX, ESTIMATED_CELL_HEIGHT_PX),
            "a pane of Zellij reports no pixel size, and ic must draw an image there all the same"
        );
        assert_eq!(
            cell_pixels_or_estimate_of(None),
            (ESTIMATED_CELL_WIDTH_PX, ESTIMATED_CELL_HEIGHT_PX),
            "a run that measured no terminal holds no measure to draw with, and an image at an estimated size beats no image"
        );
    }
    // =========================================================================
    // Tests for calculate_aspect_preserving_size
    // =========================================================================

    /// Standard cell aspect ratio used in tests (typical terminal: cells ~2x tall as wide).
    const TEST_CELL_ASPECT: f64 = 2.0;

    #[test]
    fn a_cell_aspect_is_the_height_of_the_cell_over_its_width() {
        assert_eq!(
            cell_aspect_of(REPORTED_CELL.0, REPORTED_CELL.1),
            TEST_CELL_ASPECT,
            "a cell of 10 pixels by 20 is twice as tall as it is wide"
        );
        assert_eq!(
            cell_aspect_of(DENSE_CELL.0, DENSE_CELL.1),
            TEST_CELL_ASPECT,
            "a cell of 20 pixels by 40 holds the same shape, and the shape is what this ratio carries"
        );
        assert_eq!(
            cell_aspect_of(16, 16),
            1.0,
            "a square cell gives an image the shape that the column count and the row count state"
        );
    }

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
        let result =
            downscale_to_display_pixels(&img, None, None, TEST_CELL_WIDTH_PX, TEST_CELL_HEIGHT_PX);
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn downscale_returns_borrowed_when_only_width() {
        let img = make_test_image(100, 100);
        let result = downscale_to_display_pixels(
            &img,
            Some(50),
            None,
            TEST_CELL_WIDTH_PX,
            TEST_CELL_HEIGHT_PX,
        );
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn downscale_returns_borrowed_when_only_height() {
        let img = make_test_image(100, 100);
        let result = downscale_to_display_pixels(
            &img,
            None,
            Some(50),
            TEST_CELL_WIDTH_PX,
            TEST_CELL_HEIGHT_PX,
        );
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn downscale_returns_borrowed_when_image_fits() {
        // A cell of 10 pixels by 20 gives 80 columns by 24 rows a box of 800
        // pixels by 480. A 100x100 image fits inside that box, so no downscale
        // is necessary.
        let img = make_test_image(100, 100);
        let result = downscale_to_display_pixels(
            &img,
            Some(80),
            Some(24),
            TEST_CELL_WIDTH_PX,
            TEST_CELL_HEIGHT_PX,
        );
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result.width(), 100);
        assert_eq!(result.height(), 100);
    }

    #[test]
    fn downscale_shrinks_oversized_image() {
        // A cell of 10 pixels by 20 gives 10 columns by 5 rows a box of 100
        // pixels by 100. The image is five times that box on each axis, so the
        // downscale must run.
        let (max_w, max_h) = cells_to_pixels(10, 5, TEST_CELL_WIDTH_PX, TEST_CELL_HEIGHT_PX);
        let img = make_test_image(max_w * 5, max_h * 5);
        let result = downscale_to_display_pixels(
            &img,
            Some(10),
            Some(5),
            TEST_CELL_WIDTH_PX,
            TEST_CELL_HEIGHT_PX,
        );
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
        // A cell of 10 pixels by 20 gives 10 columns by 5 rows a box of 100
        // pixels by 100. The image is 5 times as wide as it is tall, and it is
        // larger than the box on both axes.
        let (max_w, max_h) = cells_to_pixels(10, 5, TEST_CELL_WIDTH_PX, TEST_CELL_HEIGHT_PX);
        let img = make_test_image(max_w * 10, max_h * 2);
        let result = downscale_to_display_pixels(
            &img,
            Some(10),
            Some(5),
            TEST_CELL_WIDTH_PX,
            TEST_CELL_HEIGHT_PX,
        );
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
        // A cell of 10 pixels by 20 gives 80 columns by 24 rows a box of 800
        // pixels by 480. A panorama of 16384 pixels by 4096 is far larger than
        // that box, and the raw pixels of it are about 200 megabytes.
        let (max_w, max_h) = cells_to_pixels(80, 24, TEST_CELL_WIDTH_PX, TEST_CELL_HEIGHT_PX);
        let img = make_test_image(16384, 4096);
        let result = downscale_to_display_pixels(
            &img,
            Some(80),
            Some(24),
            TEST_CELL_WIDTH_PX,
            TEST_CELL_HEIGHT_PX,
        );
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
        // A division by zero panics. No caller hands a cell of no height over
        // today, because cell_pixels_or_estimate_of takes the estimate for a
        // window that divides down to a cell of no pixels, and the floor holds
        // the arithmetic if one ever does.
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
