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
use image::RgbaImage;

/// The image of one history of round-trip times, as the Recent column of one
/// row draws it.
///
/// # Arguments
/// * `history` - Every sample that the fold of one hop holds, oldest first.
/// * `width` - The width of the image in pixels.
/// * `height` - The height of the image in pixels.
pub(crate) fn plot(history: &[Sample], width: u32, height: u32) -> RgbaImage {
    todo!("the picture of a history draws its samples")
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

    /// The pixel of an image at one place.
    ///
    /// A test that reached past the edge of the image must fail its assertion
    /// and must not stop with an index, so the reader of the failure sees the
    /// pixel the test wanted beside the pixel the image gave.
    fn pixel(image: &RgbaImage, x: u32, y: u32) -> Rgba<u8> {
        image.get_pixel_checked(x, y).copied().unwrap_or(CLEAR)
    }

    #[test]
    fn the_background_of_the_image_is_transparent() {
        // A transparent background lets the terminal show its own background
        // through the cell, and the column then reads as a part of the table
        // and not as a box laid over it.
        let image = plot(&[Sample::Time(ONE_TIME)], ONE_COLUMN, SHORT);

        for row in 0..SHORT - 1 {
            assert_eq!(
                pixel(&image, 0, row),
                CLEAR,
                "the pixels above the bar carry no color and no alpha at all"
            );
        }
    }
}
