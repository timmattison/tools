//! The comparison of two pictures, pixel by pixel, for `--diff`.
//!
//! [`compare`] takes two decoded pictures and gives one picture of the
//! differences, the regions that differ, and a count of the pixels that
//! differ. The module reads no file and writes no terminal.
#![deny(clippy::indexing_slicing)]

use anyhow::Result;
use image::{DynamicImage, Rgba, RgbaImage};

/// Red, the color of a pixel that differs and of each outline.
pub const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);

/// A pixel that no picture covers. It has no color and no alpha.
pub const BLANK: Rgba<u8> = Rgba([0, 0, 0, 0]);

/// Opaque black, the ground of the `mask` mode.
const BLACK: Rgba<u8> = Rgba([0, 0, 0, 255]);

/// The largest canvas, in pixels: 2^27 pixels, which is 512 MiB of RGBA.
pub const MAX_CANVAS_PIXELS: u64 = 1 << 27;

/// What the picture of the differences shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Mode {
    /// The second image, with a red outline around each region that differs
    #[default]
    Boxes,
    /// The first image, with each pixel that differs in red
    Highlight,
    /// Opaque black, with each pixel that differs in red
    Mask,
}

/// A rectangle of the canvas, in pixels. All four edges are inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    /// The first column of the rectangle.
    pub left: u32,
    /// The first row of the rectangle.
    pub top: u32,
    /// The last column of the rectangle.
    pub right: u32,
    /// The last row of the rectangle.
    pub bottom: u32,
}

/// The result of one comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct Diff {
    /// The picture of the differences, the size of the canvas.
    pub picture: RgbaImage,
    /// The padded and merged boxes, sorted by (top, left).
    pub regions: Vec<Region>,
    /// The count of the pixels that differ.
    pub differing_pixels: u64,
}

impl Diff {
    /// The second line of the header that `ic` prints above the picture.
    ///
    /// # Returns
    ///
    /// `the images are the same` when no pixel differs. Otherwise a line such
    /// as `3 regions differ (1204 pixels)`.
    pub fn summary(&self) -> String {
        String::new()
    }
}

/// Compare two images and give the picture of the differences in `mode`.
///
/// # Arguments
///
/// * `first` - The first image.
/// * `second` - The second image.
/// * `mode` - What the picture shows.
///
/// # Errors
///
/// Returns an error when the canvas holds more than [`MAX_CANVAS_PIXELS`]
/// pixels.
pub fn compare(first: &DynamicImage, second: &DynamicImage, mode: Mode) -> Result<Diff> {
    let first = first.to_rgba8();
    let second = second.to_rgba8();
    let width = first.width().max(second.width());
    let height = first.height().max(second.height());
    let mut picture = RgbaImage::new(width, height);
    let mut differing_pixels = 0_u64;
    for (x, y, out) in picture.enumerate_pixels_mut() {
        let one = pixel_at(&first, x, y);
        let differs = one != pixel_at(&second, x, y);
        if differs {
            differing_pixels = differing_pixels.saturating_add(1);
        }
        let covered =
            first.get_pixel_checked(x, y).is_some() || second.get_pixel_checked(x, y).is_some();
        *out = match mode {
            Mode::Highlight if differs => RED,
            Mode::Mask if differs => RED,
            Mode::Mask if covered => BLACK,
            _ => one,
        };
    }
    Ok(Diff {
        picture,
        regions: Vec::new(),
        differing_pixels,
    })
}

/// The pixel of `image` at (`x`, `y`), or [`BLANK`] when the image does not
/// cover that position.
fn pixel_at(image: &RgbaImage, x: u32, y: u32) -> Rgba<u8> {
    image.get_pixel_checked(x, y).copied().unwrap_or(BLANK)
}

/// The thickness of an outline, in pixels: `max(2, ceil(L / 400))`, where `L`
/// is the longer side of the canvas.
///
/// # Arguments
///
/// * `canvas_width` - The width of the canvas, in pixels.
/// * `canvas_height` - The height of the canvas, in pixels.
pub fn line_thickness(canvas_width: u32, canvas_height: u32) -> u32 {
    let _ = (canvas_width, canvas_height);
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A distinct opaque color for each position.
    fn color(x: u32, y: u32) -> Rgba<u8> {
        Rgba([
            u8::try_from(x * 40 + 10).unwrap(),
            u8::try_from(y * 40 + 10).unwrap(),
            100,
            255,
        ])
    }

    /// A picture that holds `color` at each position.
    fn picture(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_fn(width, height, color))
    }

    #[test]
    fn pictures_of_two_sizes_share_the_upper_left_corner() {
        let diff = compare(&picture(3, 2), &picture(2, 3), Mode::Highlight).unwrap();
        assert_eq!(diff.picture.dimensions(), (3, 3));
        assert_eq!(*diff.picture.get_pixel(1, 1), color(1, 1));
        assert_eq!(diff.differing_pixels, 4);
    }

    #[test]
    fn a_fully_transparent_strip_is_not_a_difference() {
        let first = RgbaImage::from_fn(3, 2, |x, y| if x == 2 { BLANK } else { color(x, y) });
        let diff = compare(
            &DynamicImage::ImageRgba8(first),
            &picture(2, 2),
            Mode::Highlight,
        )
        .unwrap();
        assert_eq!(diff.differing_pixels, 0);
    }

    /// The color that `changed` writes.
    const OTHER: Rgba<u8> = Rgba([1, 2, 3, 255]);

    /// A picture like `picture`, with `OTHER` at each listed position.
    fn changed(width: u32, height: u32, spots: &[(u32, u32)]) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_fn(width, height, |x, y| {
            if spots.contains(&(x, y)) {
                OTHER
            } else {
                color(x, y)
            }
        }))
    }

    #[test]
    fn highlight_paints_a_changed_pixel_red() {
        let diff = compare(&picture(3, 2), &changed(2, 3, &[(0, 0)]), Mode::Highlight).unwrap();
        assert_eq!(*diff.picture.get_pixel(1, 1), color(1, 1));
        assert_eq!(*diff.picture.get_pixel(0, 0), RED);
        assert_eq!(*diff.picture.get_pixel(2, 2), BLANK);
    }

    #[test]
    fn mask_paints_the_same_pixels_black_and_the_changed_ones_red() {
        let diff = compare(&picture(3, 2), &changed(2, 3, &[(0, 0)]), Mode::Mask).unwrap();
        assert_eq!(*diff.picture.get_pixel(1, 1), BLACK);
        assert_eq!(*diff.picture.get_pixel(0, 0), RED);
        assert_eq!(*diff.picture.get_pixel(2, 2), BLANK);
    }
}
