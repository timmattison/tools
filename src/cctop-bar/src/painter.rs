//! The status item as pixels. The painter turns a mini-map into a straight
//! RGBA image, so the `AppKit` shell only has to hand the bytes to an image.
//!
//! Each size is a named constant in points. A pixel size is the size in points
//! times the backing scale.

use cctop::minimap::{Minimap, Tile, TileStyle};
use cctop::state::Rgb;

/// The side padding, left and right, in points.
pub const PADDING: u32 = 3;
/// The height of the tile band at most, in points.
pub const BAND_HEIGHT: u32 = 12;
/// The space that the menu bar keeps above and below the band, in points.
pub const BAND_MARGIN: u32 = 4;
/// The height of the tile band at least, in points.
pub const BAND_MIN: u32 = 2;
/// The width of one tile, in points.
pub const TILE_WIDTH: u32 = 8;
/// The gap between two tiles, in points.
pub const TILE_GAP: u32 = 2;
/// The thickness of the border of an outline tile, in points.
pub const BORDER: u32 = 1;
/// The space between the text of the attention field and its edge, in points.
pub const FIELD_PADDING: u32 = 3;
/// The gap between the attention field and the first tile, in points.
pub const FIELD_GAP: u32 = 4;
/// The gap between the last tile and the overflow mark, in points.
pub const OVERFLOW_GAP: u32 = 3;
/// The color of the attention field: amber `#fab005`.
pub const AMBER: Rgb = Rgb {
    red: 0xfa,
    green: 0xb0,
    blue: 0x05,
};
/// The width of a glyph of the dot font, in dots.
pub const GLYPH_WIDTH: u32 = 5;
/// The height of a glyph of the dot font, in dots.
pub const GLYPH_HEIGHT: u32 = 7;
/// The distance between two glyphs, in dots: the glyph and one empty column.
pub const GLYPH_ADVANCE: u32 = GLYPH_WIDTH + 1;
/// The band height in pixels divided by this number gives the dot size.
pub const DOT_DIVISOR: u32 = 9;

/// The color of the digits on the attention field and of the ink on a light
/// menu bar.
const BLACK: Rgb = Rgb {
    red: 0,
    green: 0,
    blue: 0,
};
/// The ink on a dark menu bar.
const WHITE: Rgb = Rgb {
    red: 0xff,
    green: 0xff,
    blue: 0xff,
};
/// A pixel that no shape covers.
const TRANSPARENT: [u8; 4] = [0, 0, 0, 0];
/// The number of bytes of one pixel.
const BYTES_PER_PIXEL: usize = 4;

/// The appearance of the menu bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    /// A light menu bar: the ink is black.
    Light,
    /// A dark menu bar: the ink is white.
    Dark,
}

/// A straight (not premultiplied) RGBA image, row-major, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixmap {
    width: u32,
    height: u32,
    scale: u32,
    bytes: Vec<u8>,
}

impl Pixmap {
    /// The width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The backing scale: the pixels of one point.
    #[must_use]
    pub fn scale(&self) -> u32 {
        self.scale
    }

    /// The pixel at column `x` and row `y`. A position outside the image is
    /// transparent.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return TRANSPARENT;
        }
        let start = (y as usize * self.width as usize + x as usize) * BYTES_PER_PIXEL;
        let mut pixel = TRANSPARENT;
        pixel.copy_from_slice(&self.bytes[start..start + BYTES_PER_PIXEL]);
        pixel
    }

    /// The bytes of the image: four for each pixel, red first.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// The rows of the dot font for `0` to `9` and `+`: five low bits for each
/// row, top row first, the leftmost dot in bit 4.
const GLYPHS: [(char, [u8; 7]); 11] = [
    ('0', [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E]),
    ('1', [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ('2', [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F]),
    ('3', [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E]),
    ('4', [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02]),
    ('5', [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E]),
    ('6', [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E]),
    ('7', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
    ('8', [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E]),
    ('9', [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C]),
    ('+', [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00]),
];

/// The rows of the glyph of `character`. A character outside the font is
/// blank.
fn glyph(character: char) -> [u8; 7] {
    GLYPHS
        .iter()
        .find(|(known, _)| *known == character)
        .map_or([0; 7], |(_, rows)| *rows)
}

/// The size of a pixel grid and the drawing on it.
struct Canvas {
    pixmap: Pixmap,
}

impl Canvas {
    fn new(width: u32, height: u32, scale: u32) -> Self {
        Self {
            pixmap: Pixmap {
                width,
                height,
                scale,
                bytes: vec![0; width as usize * height as usize * BYTES_PER_PIXEL],
            },
        }
    }

    /// Set one pixel. A position outside the image is dropped.
    fn set(&mut self, x: u32, y: u32, color: Rgb) {
        if x >= self.pixmap.width || y >= self.pixmap.height {
            return;
        }
        let start = (y as usize * self.pixmap.width as usize + x as usize) * BYTES_PER_PIXEL;
        self.pixmap.bytes[start..start + BYTES_PER_PIXEL].copy_from_slice(&[
            color.red,
            color.green,
            color.blue,
            255,
        ]);
    }

    /// Fill the rectangle at `(x, y)`, `width` by `height` pixels.
    fn fill(&mut self, x: u32, y: u32, width: u32, height: u32, color: Rgb) {
        for row in y..y.saturating_add(height) {
            for column in x..x.saturating_add(width) {
                self.set(column, row, color);
            }
        }
    }

    /// Draw the border of the rectangle, `border` pixels thick.
    fn outline(&mut self, x: u32, y: u32, width: u32, height: u32, border: u32, color: Rgb) {
        for row in 0..height {
            for column in 0..width {
                let edge = column < border
                    || row < border
                    || column >= width.saturating_sub(border)
                    || row >= height.saturating_sub(border);
                if edge {
                    self.set(x + column, y + row, color);
                }
            }
        }
    }

    /// Draw `text` with its top left corner at `(x, y)`.
    fn text(&mut self, x: u32, y: u32, dot: u32, text: &str, color: Rgb) {
        let mut left = x;
        for character in text.chars() {
            for (row, bits) in glyph(character).into_iter().enumerate() {
                for column in 0..GLYPH_WIDTH {
                    if bits & (1 << (GLYPH_WIDTH - 1 - column)) != 0 {
                        let row = u32::try_from(row).unwrap_or(0);
                        self.fill(left + column * dot, y + row * dot, dot, dot, color);
                    }
                }
            }
            left += GLYPH_ADVANCE * dot;
        }
    }
}

/// The width of `text` in dots: each glyph and the space after it, less the
/// last space.
fn text_dots(text: &str) -> u32 {
    let glyphs = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    (glyphs * GLYPH_ADVANCE).saturating_sub(1)
}

/// Paint the mini-map for a menu bar `thickness` points high at backing
/// `scale` (1 or 2; 0 counts as 1).
#[must_use]
pub fn paint(minimap: &Minimap, thickness: u32, scale: u32, appearance: Appearance) -> Pixmap {
    let scale = scale.max(1);
    let ink = match appearance {
        Appearance::Light => BLACK,
        Appearance::Dark => WHITE,
    };
    let height = thickness * scale;
    let band_points = thickness
        .saturating_sub(BAND_MARGIN)
        .clamp(BAND_MIN, BAND_HEIGHT);
    let band = band_points * scale;
    let top = height.saturating_sub(band) / 2;
    let dot = (band / DOT_DIVISOR).max(1);
    let text_top = top + band.saturating_sub(GLYPH_HEIGHT * dot) / 2;

    let empty = minimap.tiles.is_empty() && minimap.overflow == 0;
    let tile_count = u32::try_from(minimap.tiles.len().max(usize::from(empty))).unwrap_or(0);
    let attention = (minimap.attention > 0).then(|| minimap.attention.to_string());
    let overflow = (minimap.overflow > 0).then(|| format!("+{}", minimap.overflow));

    let mut width = 2 * PADDING * scale;
    if let Some(number) = &attention {
        width += text_dots(number) * dot + 2 * FIELD_PADDING * scale + FIELD_GAP * scale;
    }
    width += (tile_count * TILE_WIDTH + tile_count.saturating_sub(1) * TILE_GAP) * scale;
    if let Some(mark) = &overflow {
        width += OVERFLOW_GAP * scale + text_dots(mark) * dot;
    }

    let mut canvas = Canvas::new(width, height, scale);
    let mut left = PADDING * scale;
    if let Some(number) = &attention {
        let field = text_dots(number) * dot + 2 * FIELD_PADDING * scale;
        canvas.fill(left, top, field, band, AMBER);
        canvas.text(left + FIELD_PADDING * scale, text_top, dot, number, BLACK);
        left += field + FIELD_GAP * scale;
    }
    let tile_width = TILE_WIDTH * scale;
    if empty {
        canvas.outline(left, top, tile_width, band, BORDER * scale, ink);
        left += tile_width;
    }
    for (index, tile) in minimap.tiles.iter().enumerate() {
        if index > 0 {
            left += TILE_GAP * scale;
        }
        paint_tile(&mut canvas, *tile, left, top, tile_width, band, scale);
        left += tile_width;
    }
    if let Some(mark) = &overflow {
        canvas.text(left + OVERFLOW_GAP * scale, text_top, dot, mark, ink);
    }
    canvas.pixmap
}

/// Paint one tile in its rectangle.
fn paint_tile(
    canvas: &mut Canvas,
    tile: Tile,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    scale: u32,
) {
    match tile.style() {
        TileStyle::Filled => canvas.fill(x, y, width, height, tile.color()),
        TileStyle::Outline => canvas.outline(x, y, width, height, BORDER * scale, tile.color()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctop::state::{PaneState, FROZEN_COLOR};

    const BAR: u32 = 24;

    fn tile(state: PaneState) -> Tile {
        Tile {
            state,
            handled: false,
            frozen: false,
        }
    }

    fn map(tiles: Vec<Tile>, overflow: usize, attention: usize) -> Minimap {
        Minimap {
            tiles,
            overflow,
            attention,
        }
    }

    fn rgba(color: Rgb) -> [u8; 4] {
        [color.red, color.green, color.blue, 255]
    }

    fn paint_light(minimap: &Minimap, scale: u32) -> Pixmap {
        paint(minimap, BAR, scale, Appearance::Light)
    }

    /// The row of the top of the band at scale 1 and `BAR` points.
    const TOP: u32 = 6;

    /// The pixels in the columns `from..to` of the whole image that are not
    /// transparent.
    fn opaque_in(pixmap: &Pixmap, from: u32, to: u32) -> Vec<(u32, u32)> {
        (0..pixmap.height())
            .flat_map(|y| (from..to).map(move |x| (x, y)))
            .filter(|&(x, y)| pixmap.pixel(x, y)[3] != 0)
            .collect()
    }

    // Behavior 7.
    #[test]
    fn filled_tiles_sit_in_the_band_with_a_gap() {
        let minimap = map(vec![tile(PaneState::Working), tile(PaneState::Idle)], 0, 0);
        let image = paint_light(&minimap, 1);
        assert_eq!((image.width(), image.height(), image.scale()), (24, 24, 1));
        assert_eq!(image.bytes().len(), 24 * 24 * 4);
        assert_eq!(image.pixel(7, 12), rgba(PaneState::Working.color()));
        assert_eq!(image.pixel(3, TOP), rgba(PaneState::Working.color()));
        assert_eq!(image.pixel(10, TOP + 11), rgba(PaneState::Working.color()));
        assert_eq!(image.pixel(12, 12), TRANSPARENT);
        assert_eq!(image.pixel(15, 12), rgba(PaneState::Idle.color()));
        assert_eq!(image.pixel(2, 12), TRANSPARENT);
        assert_eq!(image.pixel(7, TOP - 1), TRANSPARENT);
        assert_eq!(image.pixel(7, TOP + 12), TRANSPARENT);
        assert_eq!(image.pixel(21, 12), TRANSPARENT);
    }

    #[test]
    fn scale_two_doubles_every_size() {
        let minimap = map(vec![tile(PaneState::Working), tile(PaneState::Idle)], 0, 0);
        let image = paint_light(&minimap, 2);
        assert_eq!((image.width(), image.height(), image.scale()), (48, 48, 2));
        assert_eq!(image.pixel(14, 24), rgba(PaneState::Working.color()));
        assert_eq!(image.pixel(6, 12), rgba(PaneState::Working.color()));
        assert_eq!(image.pixel(5, 12), TRANSPARENT);
        assert_eq!(image.pixel(22, 24), TRANSPARENT);
        assert_eq!(image.pixel(25, 24), TRANSPARENT);
        assert_eq!(image.pixel(26, 24), rgba(PaneState::Idle.color()));
        assert_eq!(image.pixel(30, 23), rgba(PaneState::Idle.color()));
        assert_eq!(image.pixel(30, 11), TRANSPARENT);
    }

    #[test]
    fn scale_zero_counts_as_one() {
        let minimap = map(vec![tile(PaneState::Working)], 0, 0);
        assert_eq!(paint_light(&minimap, 0), paint_light(&minimap, 1));
        assert_eq!(paint_light(&minimap, 0).height(), BAR);
    }

    #[test]
    fn a_thin_bar_shrinks_the_band() {
        let minimap = map(vec![tile(PaneState::Working)], 0, 0);
        let image = paint(&minimap, 10, 1, Appearance::Light);
        assert_eq!(image.height(), 10);
        // The band is 10 - 4 = 6 points high, and its top row is (10 - 6) / 2.
        assert_eq!(image.pixel(7, 1), TRANSPARENT);
        assert_eq!(image.pixel(7, 2), rgba(PaneState::Working.color()));
        assert_eq!(image.pixel(7, 7), rgba(PaneState::Working.color()));
        assert_eq!(image.pixel(7, 8), TRANSPARENT);
    }

    #[test]
    fn the_band_has_a_minimum_height() {
        let minimap = map(vec![tile(PaneState::Working)], 0, 0);
        let image = paint(&minimap, 3, 1, Appearance::Light);
        assert_eq!(image.height(), 3);
        assert_eq!(image.pixel(7, 0), rgba(PaneState::Working.color()));
        assert_eq!(image.pixel(7, 1), rgba(PaneState::Working.color()));
        assert_eq!(image.pixel(7, 2), TRANSPARENT);
    }

    // Behavior 8.
    #[test]
    fn an_outline_tile_has_a_border_and_an_empty_inside() {
        let handled = Tile {
            state: PaneState::NeedsYou,
            handled: true,
            frozen: false,
        };
        let image = paint_light(&map(vec![handled], 0, 0), 1);
        let color = rgba(PaneState::NeedsYou.color());
        assert_eq!(image.pixel(3, TOP), color);
        assert_eq!(image.pixel(10, TOP + 11), color);
        assert_eq!(image.pixel(3, 12), color);
        assert_eq!(image.pixel(10, 12), color);
        assert_eq!(image.pixel(7, TOP), color);
        assert_eq!(image.pixel(7, TOP + 11), color);
        assert_eq!(image.pixel(4, 12), TRANSPARENT);
        assert_eq!(image.pixel(7, 12), TRANSPARENT);
        assert_eq!(image.pixel(4, TOP + 1), TRANSPARENT);
    }

    #[test]
    fn an_outline_border_is_one_point_thick_at_scale_two() {
        let handled = Tile {
            state: PaneState::Idle,
            handled: true,
            frozen: false,
        };
        let image = paint_light(&map(vec![handled], 0, 0), 2);
        let color = rgba(PaneState::Idle.color());
        assert_eq!(image.pixel(6, 24), color);
        assert_eq!(image.pixel(7, 24), color);
        assert_eq!(image.pixel(8, 24), TRANSPARENT);
        assert_eq!(image.pixel(14, 12), color);
        assert_eq!(image.pixel(14, 13), color);
        assert_eq!(image.pixel(14, 14), TRANSPARENT);
    }

    #[test]
    fn a_frozen_tile_is_filled_with_the_frozen_color() {
        let frozen = Tile {
            state: PaneState::Working,
            handled: false,
            frozen: true,
        };
        let frozen_handled = Tile {
            state: PaneState::NeedsYou,
            handled: true,
            frozen: true,
        };
        let image = paint_light(&map(vec![frozen, frozen_handled], 0, 0), 1);
        assert_eq!(image.pixel(7, 12), rgba(FROZEN_COLOR));
        assert_eq!(image.pixel(15, 12), rgba(FROZEN_COLOR));
    }

    // Behavior 9.
    #[test]
    fn the_attention_field_holds_a_black_digit_on_amber() {
        let image = paint_light(&map(vec![tile(PaneState::Working)], 0, 2), 1);
        // Field: columns 3..14. Gap of 4 points. The tile starts at 18.
        assert_eq!(image.width(), 6 + 11 + 4 + 8);
        assert_eq!(image.pixel(3, TOP), rgba(AMBER));
        assert_eq!(image.pixel(13, TOP + 11), rgba(AMBER));
        assert_eq!(image.pixel(14, 12), TRANSPARENT);
        assert_eq!(image.pixel(2, 12), TRANSPARENT);
        assert_eq!(image.pixel(17, 12), TRANSPARENT);
        assert_eq!(image.pixel(18, 12), rgba(PaneState::Working.color()));
        assert_eq!(image.pixel(25, 12), rgba(PaneState::Working.color()));
        let black = (3..14)
            .flat_map(|x| (0..24).map(move |y| (x, y)))
            .filter(|&(x, y)| image.pixel(x, y) == rgba(BLACK))
            .count();
        assert!(black >= 5, "the digit has dots, found {black}");
    }

    #[test]
    fn the_digit_two_has_the_dots_of_its_glyph() {
        let image = paint_light(&map(vec![], 0, 2), 1);
        // Text: 5 x 7 dots of 1 pixel, from column 6, centered in the band:
        // rows 8 + 0 ... the band is 12 high, the text 7, so it starts at
        // row 6 + 2.
        let top = TOP + 2;
        let black = rgba(BLACK);
        // Glyph 2: top row 0x0E gives columns 1..=3, the bottom row 0x1F all five.
        assert_eq!(image.pixel(6, top), rgba(AMBER));
        assert_eq!(image.pixel(7, top), black);
        assert_eq!(image.pixel(9, top), black);
        assert_eq!(image.pixel(10, top), rgba(AMBER));
        for column in 6..11 {
            assert_eq!(image.pixel(column, top + 6), black, "column {column}");
        }
    }

    #[test]
    fn the_field_grows_with_the_digits() {
        let one = paint_light(&map(vec![], 0, 5), 1);
        let two = paint_light(&map(vec![], 0, 12), 1);
        // 12 has two glyphs: 11 dots. Field = 11 + 6 wide.
        assert_eq!(two.width() - one.width(), 6);
        assert_eq!(two.pixel(3, TOP), rgba(AMBER));
        assert_eq!(two.pixel(19, TOP), rgba(AMBER));
        assert_eq!(two.pixel(20, TOP), TRANSPARENT);
    }

    #[test]
    fn the_field_scales_with_the_backing_scale() {
        let image = paint_light(&map(vec![tile(PaneState::Working)], 0, 2), 2);
        assert_eq!(image.width(), 2 * (6 + 11 + 4 + 8));
        assert_eq!(image.pixel(6, 12), rgba(AMBER));
        assert_eq!(image.pixel(27, 12), rgba(AMBER));
        assert_eq!(image.pixel(28, 24), TRANSPARENT);
        assert_eq!(image.pixel(36, 24), rgba(PaneState::Working.color()));
        // The dot is 24 / 9 = 2 pixels.
        let black = (6..28)
            .flat_map(|x| (0..48).map(move |y| (x, y)))
            .filter(|&(x, y)| image.pixel(x, y) == rgba(BLACK))
            .count();
        assert!(black >= 20, "{black}");
    }

    #[test]
    fn no_field_when_nothing_needs_the_person() {
        let image = paint_light(&map(vec![tile(PaneState::Working)], 0, 0), 1);
        assert_eq!(image.width(), 6 + 8);
        assert!(opaque_in(&image, 0, 3).is_empty());
        assert!(!(0..image.width())
            .flat_map(|x| (0..image.height()).map(move |y| (x, y)))
            .any(|(x, y)| image.pixel(x, y) == rgba(AMBER)));
    }

    // Behavior 10.
    #[test]
    fn the_overflow_mark_uses_the_ink() {
        let minimap = map(vec![tile(PaneState::Working)], 3, 0);
        let light = paint(&minimap, BAR, 1, Appearance::Light);
        let dark = paint(&minimap, BAR, 1, Appearance::Dark);
        // Tile 3..11, gap 3, text "+3" is 11 dots wide: 14..25, padding 3.
        assert_eq!(light.width(), 6 + 8 + 3 + 11);
        let ink = opaque_in(&light, 14, light.width());
        assert!(!ink.is_empty());
        assert!(ink.iter().all(|&(x, y)| light.pixel(x, y) == rgba(BLACK)));
        assert!(opaque_in(&light, 11, 14).is_empty());
        assert!(opaque_in(&light, 25, light.width()).is_empty());
        let ink = opaque_in(&dark, 14, dark.width());
        assert!(!ink.is_empty());
        assert!(ink.iter().all(|&(x, y)| dark.pixel(x, y) == rgba(WHITE)));
    }

    #[test]
    fn the_plus_glyph_is_a_cross() {
        let image = paint_light(&map(vec![tile(PaneState::Working)], 3, 0), 1);
        let top = TOP + 2;
        // Glyph `+`: the middle column and the middle row (row 3 of 7).
        assert_eq!(image.pixel(16, top + 1), rgba(BLACK));
        assert_eq!(image.pixel(16, top + 5), rgba(BLACK));
        assert_eq!(image.pixel(14, top + 3), rgba(BLACK));
        assert_eq!(image.pixel(18, top + 3), rgba(BLACK));
        assert_eq!(image.pixel(14, top + 1), TRANSPARENT);
        assert_eq!(image.pixel(18, top), TRANSPARENT);
    }

    #[test]
    fn no_overflow_mark_when_the_overflow_is_zero() {
        let image = paint_light(&map(vec![tile(PaneState::Working)], 0, 0), 1);
        assert_eq!(image.width(), 14);
        assert!(opaque_in(&image, 11, 14).is_empty());
    }

    #[test]
    fn field_tiles_and_overflow_follow_each_other() {
        let tiles = vec![tile(PaneState::Idle), tile(PaneState::Agents)];
        let image = paint_light(&map(tiles, 10, 1), 1);
        // 3 + field 11 + gap 4 + tiles 8 + 2 + 8 + gap 3 + "+10" 17 + 3.
        assert_eq!(image.width(), 3 + 11 + 4 + 18 + 3 + 17 + 3);
        assert_eq!(image.pixel(18, 12), rgba(PaneState::Idle.color()));
        assert_eq!(image.pixel(28, 12), rgba(PaneState::Agents.color()));
        assert_eq!(image.pixel(36, 12), TRANSPARENT);
    }

    // Behavior 11.
    #[test]
    fn an_empty_minimap_paints_one_outline_tile_in_the_ink() {
        let empty = map(vec![], 0, 0);
        let light = paint(&empty, BAR, 1, Appearance::Light);
        let dark = paint(&empty, BAR, 1, Appearance::Dark);
        assert_eq!(light.width(), 14);
        assert_eq!(light.pixel(3, TOP), rgba(BLACK));
        assert_eq!(light.pixel(10, TOP + 11), rgba(BLACK));
        assert_eq!(light.pixel(7, 12), TRANSPARENT);
        assert_eq!(dark.pixel(3, TOP), rgba(WHITE));
        assert_eq!(dark.pixel(7, 12), TRANSPARENT);
    }
}
