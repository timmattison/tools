//! The image of the Recent column of one row of the live table.
//!
//! The Recent column of the table draws the round-trip times of one hop with
//! block elements, one glyph for each terminal column. The column is nine
//! columns wide, so nine samples reach a reader and the fold holds sixty. A
//! terminal that draws a real image draws pixels instead of glyphs, and one
//! character cell holds ten pixels or more across, so the same nine columns
//! then hold every sample the fold keeps. That is the whole reason this module
//! exists: the picture says six times as much in the columns it already had.
//!
//! The image stands apart from the terminal, and it stands apart from the
//! protocol that carries it. This module reads a history and a size in pixels,
//! and it answers pixels. `termgfx` picks the protocol and writes the escape
//! sequence, and `live.rs` says where the image stands. A test of the picture
//! therefore needs no terminal, no environment, and no clock: it reads the
//! pixels back and names what it wants of them.
//!
//! # Why the picture carries its own colors
//!
//! The block elements of the text table take the foreground of the terminal,
//! whatever the reader set it to, and `ui::Mark::style` says why: the height of
//! a bar already says what the bar says, so a table that painted every bar
//! would argue with the choice of the reader for no gain. An image carries its
//! own pixels and cannot ask what that foreground is, so this module states
//! three colors and gives the reason of each of them.

use crate::stats::Sample;
use image::{Rgba, RgbaImage};

/// The color of a bar.
///
/// One color must read on a light terminal and on a dark one. A gray at either
/// end of the range fails one of the two. A mid teal carries enough saturation
/// to stand against white and enough lightness to stand against black, and it
/// is not the red that the table already spends on a lost probe.
const BAR: Rgba<u8> = Rgba([64, 176, 192, 255]);

/// The color of the column of a lost probe.
///
/// The table already spends this red on the mark of a lost probe, and a reader
/// of the live table reads that red as a loss.
const LOST: Rgba<u8> = Rgba([224, 64, 64, 255]);

/// The number of rows between two painted pixels of the column of a lost probe.
///
/// Two, so the pixel at an even row is painted and the pixel at an odd row is
/// clear. The mark of a lost probe in the text table is `╳`, and it is no bar
/// of the set, because a run that prints no color must still tell a loss from a
/// slow answer. A solid red column and a tall teal bar differ by color alone,
/// so the dots carry that same argument into the image: a dotted column is no
/// bar, whatever a reader sees of the color.
const DOT_STEP: usize = 2;

/// The image of one history of round-trip times, as the Recent column of one
/// row draws it.
///
/// The scale is the scale of the block elements, read over the whole history:
/// the smallest sample of the history stands at the floor of the cell and the
/// largest one fills it. A lost probe names no limit of that scale, and neither
/// does a sample that is not a finite number. `ui::sparkline` states both
/// reasons, and this module does not restate them.
///
/// The height of a bar is one pixel and then the part of the cell that the
/// sample takes: `1 + round(part * (height - 1))`. The smallest sample of a
/// history therefore takes one row of pixels and the largest one fills the
/// cell. A bar of no pixel would draw nothing, and a sample that the run
/// measured is not a sample that the run lost.
///
/// A history whose samples are all equal, and a history of one sample, each
/// give a span of zero, and every bar of such a history stands at the floor.
/// `ui::sparkline` draws the lowest block element for such a window, for the
/// same reason: a flat line at the top of the cell would draw the quietest hop
/// of a path as the loudest one.
///
/// A sample that the run measured draws a bar of [`BAR`], which is a mid teal.
/// One color must read on a light terminal and on a dark one, and it must not
/// be the red that the table already spends on a lost probe.
///
/// A probe that no hop answered draws a column of [`LOST`], which is that red,
/// and it draws it dotted: the pixel at an even row is painted and the pixel at
/// an odd row is clear, down the whole height. The dots are what tell a loss
/// from a slow answer. A solid red column and a tall teal bar differ by color
/// alone, and the text table already refuses that: the mark of a lost probe
/// there is `╳` and it is no bar of the set.
///
/// The background is transparent. A transparent background lets the terminal
/// show its own background through the cell, and the column then reads as a
/// part of the table and not as a box laid over it. A terminal that ignores the
/// alpha paints the background of its own palette there, and the flag that
/// turns this picture on is off by default for exactly that reason.
///
/// # Arguments
/// * `history` - Every sample that the fold of one hop holds, oldest first.
/// * `width` - The width of the image in pixels.
/// * `height` - The height of the image in pixels.
pub(crate) fn plot(history: &[Sample], width: u32, height: u32) -> RgbaImage {
    // Every channel of a new image is zero, and an alpha of zero is a pixel
    // that paints nothing at all.
    let mut image = RgbaImage::new(width, height);
    let count = history.len();
    // The scale reads only the answers that compare. A lost probe measured no
    // time, and a time that is not a finite number does not compare, so neither
    // of them names a limit of the scale.
    let mut lowest = f64::INFINITY;
    let mut highest = f64::NEG_INFINITY;
    for time in history.iter().filter_map(finite_time) {
        lowest = lowest.min(time);
        highest = highest.max(time);
    }
    let span = highest - lowest;
    for column in 0..width {
        let index = usize::try_from(column).unwrap_or(usize::MAX).min(count - 1);
        match history[index] {
            Sample::Lost => {
                for row in (0..height).step_by(DOT_STEP) {
                    image.put_pixel(column, row, LOST);
                }
            }
            Sample::Time(time) => {
                // A history of one sample, and a history whose samples are all
                // equal, each give a span of zero. A history that holds no
                // sample which compares gives a span below zero, because the
                // two limits stayed at the infinities that the fold started
                // them at. Neither history divides, and both of them draw at
                // the floor. A time that is not a finite number draws there as
                // well, because such a time compares with nothing.
                let part = if span > 0.0 && time.is_finite() {
                    (time - lowest) / span
                } else {
                    0.0
                };
                let pixels = bar_pixels(part, height);
                for row in (height - pixels)..height {
                    image.put_pixel(column, row, BAR);
                }
            }
        }
    }
    image
}

/// The height of one bar in pixels, at its part of the span of the history.
///
/// The floor takes one pixel and every pixel above it belongs to the span, so
/// the smallest sample of a history draws one row of pixels and the largest one
/// fills the cell. A bar of no pixel would draw nothing at all, and a sample
/// that the run measured is not a sample that the run lost: the reader must be
/// able to tell the two apart at the floor of the cell as well as at the top of
/// it.
///
/// # Arguments
/// * `part` - The place of the sample in the span of the history, from 0 at the
///   smallest sample to 1 at the largest one.
/// * `height` - The height of the image in pixels.
///
/// # Returns
/// The number of pixels of the bar, which is 1 or more and never more than
/// `height`.
fn bar_pixels(part: f64, height: u32) -> u32 {
    let above_the_floor = f64::from(height.saturating_sub(1));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the part runs from 0 to 1, so the product stays inside the rows above the floor and never runs below zero. The cast of a float to an integer saturates in Rust anyway: a number below zero and a number that is not a number each give 0, and a number too large gives `u32::MAX`"
    )]
    let raised = (part * above_the_floor).round() as u32;
    raised.saturating_add(1)
}

/// The round-trip time of one sample, when that sample holds a time which
/// compares.
///
/// This is `ui::finite_time`, and the two stand apart on purpose: `ui` names
/// the marks of a text column and this module names the pixels of an image, so
/// neither one reaches into the other. Both of them state the same rule,
/// because both of them draw the same history at the same scale.
fn finite_time(sample: &Sample) -> Option<f64> {
    match *sample {
        Sample::Time(time) if time.is_finite() => Some(time),
        Sample::Time(_) | Sample::Lost => None,
    }
}

#[cfg(test)]
mod tests {
    use super::plot;
    use crate::stats::Sample;
    use image::{Rgba, RgbaImage};

    /// The pixel that stands where nothing is painted.
    ///
    /// The test spells the four numbers, and the module spells them again. That
    /// is on purpose, as it is for the bars of the sparkline: a test that read
    /// the constant of the module would agree with every background the module
    /// ever holds, and the background of this image is what lets the terminal
    /// show through the cell.
    const CLEAR: Rgba<u8> = Rgba([0, 0, 0, 0]);

    /// The width in pixels of every image of one sample below.
    ///
    /// One column of pixels for one sample, so the tests of a color read a
    /// column that no other sample shares.
    const ONE_COLUMN: u32 = 1;

    /// The height in pixels of the small images below.
    ///
    /// Four rows are enough to tell a bar of one pixel from a bar that fills
    /// the cell, and few enough that a test names every row of the column.
    const SHORT: u32 = 4;

    /// One answer of a hop, in milliseconds.
    const ONE_TIME: f64 = 1.0;

    /// An answer of that same hop that took longer, in milliseconds.
    ///
    /// The scale of the image runs from the smallest sample of the history to
    /// the largest one, so a test that reads the pixels above a bar must read
    /// the column of a sample which is not the largest. This second time makes
    /// [`ONE_TIME`] the smallest sample of the history it stands in.
    const A_SLOWER_TIME: f64 = 2.0;

    /// The width in pixels of an image of two samples.
    const TWO_COLUMNS: u32 = 2;

    /// The number of pixels of the bar that stands in the column at `x`.
    ///
    /// A bar grows from the floor of the cell, so the count reads up from the
    /// last row of the column and stops at the first row that holds no bar. A
    /// column whose bar floats above a clear pixel therefore counts short, and
    /// the test that reads it fails.
    fn bar(image: &RgbaImage, x: u32) -> u32 {
        let mut pixels = 0;
        while pixels < image.height() && pixel(image, x, image.height() - 1 - pixels) == TEAL {
            pixels += 1;
        }
        pixels
    }

    /// The pixel of an image at one place.
    ///
    /// A test that reached past the edge of the image must fail its assertion
    /// and must not stop with an index, so the reader of the failure sees the
    /// pixel the test wanted beside the pixel the image gave.
    fn pixel(image: &RgbaImage, x: u32, y: u32) -> Rgba<u8> {
        image.get_pixel_checked(x, y).copied().unwrap_or(CLEAR)
    }

    /// The color of a bar, as a reader of a graphics terminal sees it.
    ///
    /// The test spells the four numbers, and the module spells them again, for
    /// the reason that [`CLEAR`] states.
    const TEAL: Rgba<u8> = Rgba([64, 176, 192, 255]);

    #[test]
    fn a_bar_is_teal() {
        // One color must read on a light terminal and on a dark one, and it
        // must not be the red that the table already spends on a loss.
        let image = plot(&[Sample::Time(ONE_TIME)], ONE_COLUMN, SHORT);

        assert_eq!(
            pixel(&image, 0, SHORT - 1),
            TEAL,
            "the bar of a sample that the run measured stands in an opaque mid teal"
        );
    }

    /// The color of the column of a lost probe, as a reader of a graphics
    /// terminal sees it.
    ///
    /// The test spells the four numbers, and the module spells them again, for
    /// the reason that [`CLEAR`] states.
    const RED: Rgba<u8> = Rgba([224, 64, 64, 255]);

    #[test]
    fn a_lost_probe_draws_a_dotted_red_column() {
        // A solid red column and a tall teal bar differ by color alone, and the
        // mark of a lost probe must tell a loss from a slow answer whatever a
        // reader sees of the color. The dots carry that argument into the
        // image: a dotted column is no bar.
        let image = plot(&[Sample::Lost], ONE_COLUMN, SHORT);

        for row in 0..SHORT {
            let wanted = if row.is_multiple_of(2) { RED } else { CLEAR };
            assert_eq!(
                pixel(&image, 0, row),
                wanted,
                "the column of a lost probe paints its even rows and leaves its odd rows clear, down the whole height"
            );
        }
    }

    /// The shortest round-trip time of the history that the scale reads, in
    /// milliseconds.
    const QUICKEST: f64 = 10.0;

    /// The longest round-trip time of that same history, in milliseconds.
    const SLOWEST: f64 = 20.0;

    /// The number of columns of that history: the two answers above, one lost
    /// probe, and two answers that no scale can read.
    const FIVE_COLUMNS: u32 = 5;

    #[test]
    fn the_smallest_sample_of_the_history_stands_at_the_floor_and_the_largest_fills_the_cell() {
        // The scale is the scale of the block elements, read over the whole
        // history. A lost probe measured no time, so it names no limit of that
        // scale, and a time that is not a finite number does not compare, so it
        // names no limit either. `ui::sparkline` states both reasons.
        //
        // An infinity that reached the scale would take the top of it, and
        // every answer of the hop would then draw the lowest bar.
        let image = plot(
            &[
                Sample::Time(QUICKEST),
                Sample::Lost,
                Sample::Time(f64::INFINITY),
                Sample::Time(f64::NAN),
                Sample::Time(SLOWEST),
            ],
            FIVE_COLUMNS,
            SHORT,
        );

        assert_eq!(
            bar(&image, 0),
            1,
            "the smallest sample of the history stands at the floor of the cell"
        );
        assert_eq!(
            bar(&image, 2),
            1,
            "an infinity draws the lowest bar and names no limit of the scale"
        );
        assert_eq!(
            bar(&image, 3),
            1,
            "a sample that is not a number draws the lowest bar as well"
        );
        assert_eq!(
            bar(&image, FIVE_COLUMNS - 1),
            SHORT,
            "and the largest sample of the history fills the cell"
        );
    }

    /// One answer that a hop gave over and over, in milliseconds.
    const A_STEADY_TIME: f64 = 5.0;

    /// The number of columns of a history of three steady answers.
    const THREE_COLUMNS: u32 = 3;

    #[test]
    fn a_history_that_varies_by_nothing_draws_at_the_floor() {
        // A history whose samples are all equal, and a history of one sample,
        // each give a span of zero. `ui::sparkline` draws the lowest block
        // element for such a window, and the image draws the lowest bar, for
        // the same reason: a flat line at the top of the cell would draw the
        // quietest hop of a path as the loudest one.
        let steady = plot(
            &[
                Sample::Time(A_STEADY_TIME),
                Sample::Time(A_STEADY_TIME),
                Sample::Time(A_STEADY_TIME),
            ],
            THREE_COLUMNS,
            SHORT,
        );

        for column in 0..THREE_COLUMNS {
            assert_eq!(
                bar(&steady, column),
                1,
                "every bar of a history that varies by nothing stands at the floor"
            );
        }

        let one = plot(&[Sample::Time(A_STEADY_TIME)], ONE_COLUMN, SHORT);
        assert_eq!(
            bar(&one, 0),
            1,
            "and a history of one sample varies by nothing either"
        );
    }

    /// The height in pixels of the image that reads the height of a bar.
    ///
    /// Ten rows are enough that the bar of a sample at the middle of the span
    /// stands clear of the bar at the floor and clear of the bar that fills the
    /// cell.
    const TALL: u32 = 10;

    /// The answer at the middle of the span of that image, in milliseconds.
    ///
    /// The three answers are 10, 20, and 30, so the middle one takes half of
    /// the span. Half of the nine rows above the floor is 4.5, which rounds to
    /// 5, so the bar of it stands 6 pixels tall.
    const MIDDLE: f64 = 15.0;

    /// The height in pixels of the bar of that middle answer.
    const MIDDLE_PIXELS: u32 = 6;

    #[test]
    fn the_height_of_a_bar_is_its_part_of_the_cell_and_never_no_pixel_at_all() {
        // A bar of no pixel would draw nothing, and a sample that the run
        // measured is not a sample that the run lost. The smallest sample
        // therefore takes one row of pixels and the largest fills the cell, and
        // every sample between them takes its part of the rows that stand above
        // that floor.
        let image = plot(
            &[
                Sample::Time(QUICKEST),
                Sample::Time(MIDDLE),
                Sample::Time(SLOWEST),
            ],
            THREE_COLUMNS,
            TALL,
        );

        assert_eq!(
            bar(&image, 0),
            1,
            "the smallest sample takes one row of pixels and never none"
        );
        assert_eq!(
            bar(&image, 1),
            MIDDLE_PIXELS,
            "a sample at the middle of the span takes half of the rows above that floor"
        );
        assert_eq!(
            bar(&image, 2),
            TALL,
            "and the largest sample fills the cell"
        );
    }

    /// The number of samples that the fold of one hop holds.
    ///
    /// The test spells the count, and `stats.rs` spells it again. That is on
    /// purpose: the whole point of the image is that it draws every sample the
    /// fold keeps, where the nine columns of the block elements draw nine of
    /// them.
    const HISTORY: u32 = 60;

    /// The width in pixels of an image that draws that whole history.
    ///
    /// Nine character cells of ten pixels each. Ninety columns over sixty
    /// samples gives each sample one or two columns of pixels.
    const WIDE: u32 = 90;

    #[test]
    fn every_sample_of_the_history_draws() {
        // The block elements show nine of the sixty samples that the fold
        // holds, and this is the whole point of the image: the column of pixels
        // at `x` draws the sample at `x * count / width`, so no sample of the
        // history goes undrawn.
        //
        // The image is as tall as the history is long, so each of the sixty
        // samples takes a bar of its own height and a test reads which sample a
        // column drew.
        let history: Vec<Sample> = (0..HISTORY).map(|step| Sample::Time(f64::from(step))).collect();
        let image = plot(&history, WIDE, HISTORY);

        for column in 0..WIDE {
            let index = column * HISTORY / WIDE;
            assert_eq!(
                bar(&image, column),
                index + 1,
                "the column of pixels at {column} draws the sample at {index}"
            );
        }

        let mut heights: Vec<u32> = (0..WIDE).map(|column| bar(&image, column)).collect();
        heights.sort_unstable();
        heights.dedup();
        assert_eq!(
            heights,
            (1..=HISTORY).collect::<Vec<u32>>(),
            "and every sample of the history draws a bar of its own"
        );
    }

    #[test]
    fn the_background_of_the_image_is_transparent() {
        // A transparent background lets the terminal show its own background
        // through the cell, and the column then reads as a part of the table
        // and not as a box laid over it.
        //
        // The column that the test reads holds the smallest sample of the
        // history, because the largest sample fills the cell and leaves no row
        // of a background to read.
        let image = plot(
            &[Sample::Time(ONE_TIME), Sample::Time(A_SLOWER_TIME)],
            TWO_COLUMNS,
            SHORT,
        );

        for row in 0..SHORT - 1 {
            assert_eq!(
                pixel(&image, 0, row),
                CLEAR,
                "the pixels above the bar carry no color and no alpha at all"
            );
        }
    }
}
