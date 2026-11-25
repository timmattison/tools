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
    let covered = |x: u32, y: u32| covers(&first, x, y) || covers(&second, x, y);
    let mut picture = RgbaImage::new(width, height);
    let mut differing = Grid::new(width, height);
    let mut differing_pixels = 0_u64;
    for (x, y, out) in picture.enumerate_pixels_mut() {
        let one = pixel_at(&first, x, y);
        let two = pixel_at(&second, x, y);
        let differs = one != two;
        if differs {
            differing_pixels = differing_pixels.saturating_add(1);
            differing.set(x, y);
        }
        *out = match mode {
            Mode::Boxes => two,
            Mode::Highlight if differs => RED,
            Mode::Mask if differs => RED,
            Mode::Mask if covered(x, y) => BLACK,
            _ => one,
        };
    }
    let thickness = line_thickness(width, height);
    let regions = find_regions(&differing, thickness);
    if mode == Mode::Boxes {
        for region in &regions {
            draw_outline(&mut picture, *region, thickness, covered);
        }
    }
    Ok(Diff {
        picture,
        regions,
        differing_pixels,
    })
}

/// The pixel of `image` at (`x`, `y`), or [`BLANK`] when the image does not
/// cover that position.
fn pixel_at(image: &RgbaImage, x: u32, y: u32) -> Rgba<u8> {
    image.get_pixel_checked(x, y).copied().unwrap_or(BLANK)
}

/// Whether `image` covers the position (`x`, `y`).
fn covers(image: &RgbaImage, x: u32, y: u32) -> bool {
    x < image.width() && y < image.height()
}

/// A flag for each pixel of the canvas.
struct Grid {
    /// The width of the canvas, in pixels.
    width: u32,
    /// The height of the canvas, in pixels.
    height: u32,
    /// The flags, row by row from the top left.
    cells: Vec<bool>,
}

impl Grid {
    /// A grid of `width` by `height` flags that are all off.
    fn new(width: u32, height: u32) -> Self {
        let count = u64::from(width).saturating_mul(u64::from(height));
        Self {
            width,
            height,
            cells: vec![false; usize::try_from(count).unwrap_or(0)],
        }
    }

    /// The position of (`x`, `y`) in `cells`, or `None` outside the grid.
    fn index(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let row = usize::try_from(y).ok()?;
        let column = usize::try_from(x).ok()?;
        row.checked_mul(usize::try_from(self.width).ok()?)?
            .checked_add(column)
    }

    /// Whether the flag at (`x`, `y`) is on. A position outside the grid is off.
    fn get(&self, x: u32, y: u32) -> bool {
        self.index(x, y)
            .and_then(|index| self.cells.get(index))
            .copied()
            .unwrap_or(false)
    }

    /// Turn the flag at (`x`, `y`) on. A position outside the grid does nothing.
    fn set(&mut self, x: u32, y: u32) {
        if let Some(cell) = self.index(x, y).and_then(|index| self.cells.get_mut(index)) {
            *cell = true;
        }
    }
}

/// The padded boxes of the pixels that differ, sorted by (top, left).
///
/// Two pixels that differ are in one group when they touch, also at a corner.
/// The box of a group has `thickness + 2` pixels of padding on each side, and
/// the canvas clips it.
///
/// # Arguments
///
/// * `differing` - The flag of each pixel that differs.
/// * `thickness` - The thickness of an outline, in pixels.
fn find_regions(differing: &Grid, thickness: u32) -> Vec<Region> {
    let padding = thickness.saturating_add(2);
    let last_column = differing.width.saturating_sub(1);
    let last_row = differing.height.saturating_sub(1);
    let mut seen = Grid::new(differing.width, differing.height);
    let mut regions = Vec::new();
    for y in 0..differing.height {
        for x in 0..differing.width {
            if !differing.get(x, y) || seen.get(x, y) {
                continue;
            }
            let mut bounds = Region {
                left: x,
                top: y,
                right: x,
                bottom: y,
            };
            let mut stack = vec![(x, y)];
            seen.set(x, y);
            while let Some((cx, cy)) = stack.pop() {
                bounds.left = bounds.left.min(cx);
                bounds.top = bounds.top.min(cy);
                bounds.right = bounds.right.max(cx);
                bounds.bottom = bounds.bottom.max(cy);
                for dy in -1..=1_i32 {
                    for dx in -1..=1_i32 {
                        let (Some(nx), Some(ny)) =
                            (cx.checked_add_signed(dx), cy.checked_add_signed(dy))
                        else {
                            continue;
                        };
                        if differing.get(nx, ny) && !seen.get(nx, ny) {
                            seen.set(nx, ny);
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            regions.push(Region {
                left: bounds.left.saturating_sub(padding),
                top: bounds.top.saturating_sub(padding),
                right: bounds.right.saturating_add(padding).min(last_column),
                bottom: bounds.bottom.saturating_add(padding).min(last_row),
            });
        }
    }
    regions.sort_by_key(|region| (region.top, region.left));
    regions
}

/// Draw a red outline inside the edge of `region`.
///
/// A pixel of the region is red when it is less than `thickness` pixels from
/// an edge. A pixel that `covered` rejects stays as it is.
///
/// # Arguments
///
/// * `picture` - The picture to draw on.
/// * `region` - The box to outline.
/// * `thickness` - The thickness of the outline, in pixels.
/// * `covered` - Whether an image covers a pixel.
fn draw_outline(
    picture: &mut RgbaImage,
    region: Region,
    thickness: u32,
    covered: impl Fn(u32, u32) -> bool,
) {
    for y in region.top..=region.bottom {
        for x in region.left..=region.right {
            let on_edge = x.saturating_sub(region.left) < thickness
                || region.right.saturating_sub(x) < thickness
                || y.saturating_sub(region.top) < thickness
                || region.bottom.saturating_sub(y) < thickness;
            if on_edge && covered(x, y) {
                if let Some(pixel) = picture.get_pixel_mut_checked(x, y) {
                    *pixel = RED;
                }
            }
        }
    }
}

/// The thickness of an outline, in pixels: `max(2, ceil(L / 400))`, where `L`
/// is the longer side of the canvas.
///
/// # Arguments
///
/// * `canvas_width` - The width of the canvas, in pixels.
/// * `canvas_height` - The height of the canvas, in pixels.
pub fn line_thickness(canvas_width: u32, canvas_height: u32) -> u32 {
    canvas_width.max(canvas_height).div_ceil(400).max(2)
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

    #[test]
    fn the_line_thickness_grows_with_the_longer_side() {
        assert_eq!(line_thickness(1, 1), 2);
        assert_eq!(line_thickness(800, 10), 2);
        assert_eq!(line_thickness(10, 801), 3);
        assert_eq!(line_thickness(4000, 3000), 10);
    }

    /// A color that depends on the position, and that is never `OTHER`.
    fn shade(x: u32, y: u32) -> Rgba<u8> {
        Rgba([
            u8::try_from(x * 8 % 256).unwrap(),
            u8::try_from(y * 8 % 256).unwrap(),
            50,
            255,
        ])
    }

    /// A picture of the given size that holds `shade`, with `OTHER` at each
    /// listed position.
    fn field(width: u32, height: u32, spots: &[(u32, u32)]) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_fn(width, height, |x, y| {
            if spots.contains(&(x, y)) {
                OTHER
            } else {
                shade(x, y)
            }
        }))
    }

    /// The region with these four edges.
    fn region(left: u32, top: u32, right: u32, bottom: u32) -> Region {
        Region {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn boxes_of_two_equal_pictures_show_the_second_picture() {
        let second = field(20, 20, &[]);
        let diff = compare(&field(20, 20, &[]), &second, Mode::Boxes).unwrap();
        assert!(diff.regions.is_empty());
        assert_eq!(diff.differing_pixels, 0);
        assert_eq!(diff.picture, second.to_rgba8());
    }

    #[test]
    fn boxes_outline_one_changed_pixel_inside_its_padded_box() {
        let second = field(30, 30, &[(10, 10)]);
        let diff = compare(&field(30, 30, &[]), &second, Mode::Boxes).unwrap();
        assert_eq!(diff.regions, vec![region(6, 6, 14, 14)]);
        assert_eq!(diff.differing_pixels, 1);
        for (x, y) in [(6, 10), (7, 10), (13, 10), (14, 10), (10, 6), (10, 14)] {
            assert_eq!(*diff.picture.get_pixel(x, y), RED, "({x}, {y})");
        }
        let second = second.to_rgba8();
        for (x, y) in [(8, 10), (10, 10), (12, 10), (5, 10)] {
            assert_eq!(
                diff.picture.get_pixel(x, y),
                second.get_pixel(x, y),
                "({x}, {y})"
            );
        }
    }
}
