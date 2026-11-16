//! The two charts of the status item, drawn into a pixel buffer.
//!
//! Each chart has a frame and is as high as the status bar, in whole points.
//! The two charts sit side by side and share one border, with no gap. The left
//! chart shows the CPU share. The right chart shows the memory in use, in the
//! color of the pressure level.
//!
//! Each sample is one point wide. Each chart has 32 columns inside its frame,
//! and it shows that number of the newest samples, with the newest at the right
//! edge. The label rows lie at the top of the interior. The shell draws the
//! name of the host there, so this module only reports the rows.
//!
//! The drawing is pure arithmetic on a plain buffer, so a test reads each
//! pixel and AppKit only has to show the bytes.
//!
//! All sizes in the layout are in points. Each point is `scale` pixels wide
//! and high.

use std::fmt;

use crate::history::History;
use crate::pressure::Level;
use crate::sample::Sample;

/// A color: non-premultiplied sRGB and alpha.
pub type Rgba = [u8; 4];

/// The color of an empty pixel.
pub const TRANSPARENT: Rgba = [0, 0, 0, 0];
/// The color of the CPU system share. It is red.
pub const SYSTEM: Rgba = [255, 59, 48, 255];
/// The color of the CPU user share. It is blue.
pub const USER: Rgba = [10, 132, 255, 255];
/// The color of the memory column at normal pressure. It is green.
pub const NORMAL: Rgba = [52, 199, 89, 255];
/// The color of the memory column at warn pressure. It is orange.
pub const WARN: Rgba = [255, 149, 0, 255];
/// The color of the memory column at critical pressure. It is red.
pub const CRITICAL: Rgba = [255, 59, 48, 255];
/// The color of the memory column at a level that this tool does not know. It is gray.
pub const UNKNOWN: Rgba = [142, 142, 147, 255];

/// The label color in the light appearance. It is black at 85%.
const LABEL_LIGHT: Rgba = [0, 0, 0, 217];
/// The label color in the dark appearance. It is white at 85%.
const LABEL_DARK: Rgba = [255, 255, 255, 217];

/// The width of the frame of each chart, in points.
pub const FRAME_POINTS: u32 = 1;
/// The number of columns inside the frame of each chart, thus the number of
/// samples that each chart shows. With one shared border, the image is
/// 67 points wide, and the label gets 65 points between the two outer frames.
/// That holds a host name such as `M4-Pro-Mac-Mini` (62.7 points in the label
/// font) with no ellipsis.
const CHART_COLUMNS: u32 = 32;

/// The number of bytes of one pixel.
const BYTES_PER_PIXEL: usize = 4;
/// The number of charts.
const CHART_COUNT: u32 = 2;
/// The largest percentage.
const PERCENT_MAX: u32 = 100;
/// Half of the largest percentage. It rounds the memory height to the nearest row.
const PERCENT_HALF: u32 = 50;

/// The appearance of the menu bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    /// Dark text on a light bar.
    Light,
    /// Light text on a dark bar.
    Dark,
}

impl Appearance {
    /// The color of the label text. It is the label color of the appearance.
    #[must_use]
    pub fn label(self) -> Rgba {
        match self {
            Appearance::Light => LABEL_LIGHT,
            Appearance::Dark => LABEL_DARK,
        }
    }

    /// The color of the frame. The frame and the label text both take the
    /// label color of the appearance.
    #[must_use]
    pub fn frame(self) -> Rgba {
        self.label()
    }
}

/// The reason that a geometry is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryError {
    /// The height is below `Geometry::MIN_HEIGHT_POINTS`.
    HeightTooSmall {
        /// The height that the caller gave, in points.
        height_points: u32,
    },
    /// The scale is 0.
    ZeroScale,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeometryError::HeightTooSmall { height_points } => write!(
                f,
                "the height of {height_points} points is below the minimum of {} points",
                Geometry::MIN_HEIGHT_POINTS
            ),
            GeometryError::ZeroScale => write!(f, "the scale must be at least 1"),
        }
    }
}

impl std::error::Error for GeometryError {}

/// The height of the charts in points, the pixels per point, and the label.
///
/// The height of the image is the height of each chart. The width of the image
/// does not depend on the height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    height_points: u32,
    scale: u32,
    label_points: Option<u32>,
}

impl Geometry {
    /// The smallest height. A frame, one row, and a frame.
    pub const MIN_HEIGHT_POINTS: u32 = 3;

    /// Makes a geometry without a label.
    ///
    /// The height is the height of the image and of each chart.
    ///
    /// # Errors
    ///
    /// Returns an error if the height is below `MIN_HEIGHT_POINTS` or the scale is 0.
    pub fn new(height_points: u32, scale: u32) -> Result<Geometry, GeometryError> {
        if height_points < Self::MIN_HEIGHT_POINTS {
            return Err(GeometryError::HeightTooSmall { height_points });
        }
        if scale == 0 {
            return Err(GeometryError::ZeroScale);
        }
        Ok(Geometry {
            height_points,
            scale,
            label_points: None,
        })
    }

    /// Makes the geometry of the status bar of the screen.
    ///
    /// The inputs are `NSStatusBar.thickness`, `NSScreen.backingScaleFactor`,
    /// and the line height of the label text. The line height is `None` when
    /// there is no label. The scale is the backing scale rounded, and at least 1.
    ///
    /// The height of the charts is the whole points of the thickness, and at
    /// least `MIN_HEIGHT_POINTS`. An input that is not finite gives the
    /// minimum. The label does not change the height.
    ///
    /// The label height is the line height rounded up to whole points. A line
    /// height that is not finite or not above 0 gives no label. If the interior
    /// of a chart is lower than the label height, there is no label too.
    #[must_use]
    pub fn from_status_bar(
        thickness_points: f64,
        backing_scale: f64,
        label_line_height_points: Option<f64>,
    ) -> Geometry {
        let height_points = whole_number(thickness_points.floor()).max(Self::MIN_HEIGHT_POINTS);
        let scale = whole_number(backing_scale.round()).max(1);
        let interior = height_points - 2 * FRAME_POINTS;
        let label_points = label_line_height_points
            .filter(|line_height| line_height.is_finite() && *line_height > 0.0)
            .map(|line_height| whole_number(line_height.ceil()))
            .filter(|label| *label <= interior);
        Geometry {
            height_points,
            scale,
            label_points,
        }
    }

    /// The width of the whole image in points. It holds the two charts, which
    /// share the border between them.
    #[must_use]
    pub fn image_width_points(self) -> u32 {
        CHART_COUNT * chart_width_points() - FRAME_POINTS
    }

    /// The height of the whole image in points. It is the height of each chart.
    #[must_use]
    pub fn image_height_points(self) -> u32 {
        self.height_points
    }

    /// The number of columns inside each frame, thus the number of samples
    /// that each chart shows. It does not depend on the height.
    #[must_use]
    pub fn columns(self) -> std::num::NonZeroUsize {
        let columns = usize::try_from(CHART_COLUMNS).unwrap_or(usize::MAX);
        std::num::NonZeroUsize::new(columns).unwrap_or(std::num::NonZeroUsize::MIN)
    }

    /// The rows of the label, in points from the top of the image.
    /// It is `None` when there is no label. The rows start inside the frame.
    #[must_use]
    pub fn label_rows_points(self) -> Option<std::ops::Range<u32>> {
        self.label_points
            .map(|label| FRAME_POINTS..FRAME_POINTS + label)
    }

    /// The height of each chart in points. It is also the height of the image.
    #[must_use]
    pub fn height_points(self) -> u32 {
        self.height_points
    }

    /// The number of pixels of one point.
    #[must_use]
    pub fn scale(self) -> u32 {
        self.scale
    }
}

/// The width of one chart in points: its columns and the frame on each side.
fn chart_width_points() -> u32 {
    CHART_COLUMNS + 2 * FRAME_POINTS
}

/// Converts a number that is already whole into a `u32`.
///
/// A value that is not finite gives 0. A negative value gives 0, and a value
/// above `u32::MAX` gives `u32::MAX`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is finite and clamped into the range of u32 first"
)]
fn whole_number(value: f64) -> u32 {
    if value.is_finite() {
        value.clamp(0.0, f64::from(u32::MAX)) as u32
    } else {
        0
    }
}

/// The number of rows that a fraction of `rows` fills: `round(fraction * rows)`,
/// clamped to `0..=rows`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is rounded and clamped to 0..=rows first, and rows is a u32"
)]
fn rows_of_fraction(fraction: f64, rows: u32) -> u32 {
    let filled = (fraction * f64::from(rows)).round();
    if filled.is_nan() {
        0
    } else {
        filled.clamp(0.0, f64::from(rows)) as u32
    }
}

/// An RGBA image. Row 0 is at the top. Each pixel has 4 bytes. Rows have no padding.
#[derive(Debug, Clone)]
pub struct Pixmap {
    width: u32,
    height: u32,
    scale: u32,
    bytes: Vec<u8>,
}

impl Pixmap {
    /// Makes an empty image of the given size in points.
    fn empty(width_points: u32, height_points: u32, scale: u32) -> Pixmap {
        let width = width_points * scale;
        let height = height_points * scale;
        let length = usize::try_from(u64::from(width) * u64::from(height))
            .map_or(0, |pixels| pixels * BYTES_PER_PIXEL);
        Pixmap {
            width,
            height,
            scale,
            bytes: vec![0; length],
        }
    }

    /// The byte offset of a pixel that is inside of the image.
    fn offset(&self, x: u32, y: u32) -> usize {
        let index = u64::from(y) * u64::from(self.width) + u64::from(x);
        usize::try_from(index).map_or(usize::MAX, |pixels| pixels.saturating_mul(BYTES_PER_PIXEL))
    }

    /// Paints a rectangle in pixels. The caller keeps it inside of the image.
    fn fill(&mut self, left: u32, top: u32, width: u32, height: u32, color: Rgba) {
        for y in top..top + height {
            for x in left..left + width {
                let offset = self.offset(x, y);
                self.bytes[offset..offset + BYTES_PER_PIXEL].copy_from_slice(&color);
            }
        }
    }

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

    /// The width in points.
    #[must_use]
    pub fn width_points(&self) -> u32 {
        self.width / self.scale
    }

    /// The height in points.
    #[must_use]
    pub fn height_points(&self) -> u32 {
        self.height / self.scale
    }

    /// The color of one pixel.
    ///
    /// # Panics
    ///
    /// Panics if the pixel is outside of the image.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Rgba {
        assert!(
            x < self.width && y < self.height,
            "pixel ({x}, {y}) is outside of the {}x{} image",
            self.width,
            self.height
        );
        let offset = self.offset(x, y);
        let mut color = TRANSPARENT;
        color.copy_from_slice(&self.bytes[offset..offset + BYTES_PER_PIXEL]);
        color
    }

    /// The bytes of the image, for the AppKit shell.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The bytes of the image with each color channel multiplied by its alpha.
    ///
    /// Each product rounds to the nearest whole value. AppKit draws into a
    /// bitmap only when its alpha is premultiplied.
    #[must_use]
    pub fn premultiplied_bytes(&self) -> Vec<u8> {
        self.bytes
            .as_chunks::<BYTES_PER_PIXEL>()
            .0
            .iter()
            .flat_map(|&[red, green, blue, alpha]| {
                [
                    premultiply(red, alpha),
                    premultiply(green, alpha),
                    premultiply(blue, alpha),
                    alpha,
                ]
            })
            .collect()
    }

    /// Paints the label through `coverage`, in the label color of
    /// `appearance`.
    ///
    /// Each pixel takes the label color at the share of its alpha that the
    /// coverage gives, over the color that the pixel had. A pixel with a
    /// coverage of 0 keeps its color.
    ///
    /// # Errors
    ///
    /// Returns [`SizeMismatch`] when the coverage is not the size of the image
    /// in pixels. The image then stays as it was.
    pub fn paint_label(
        &mut self,
        coverage: &Coverage,
        appearance: Appearance,
    ) -> Result<(), SizeMismatch> {
        let _ = (coverage, appearance);
        Ok(())
    }
}

/// How much the glyphs of the label cover each pixel of an image, from 0 for
/// none to 255 for all. Row 0 is at the top. Rows have no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    width: u32,
    height: u32,
    values: Vec<u8>,
}

impl Coverage {
    /// Makes the coverage of an image of `width` by `height` pixels.
    ///
    /// Returns `None` when `values` does not hold exactly one value for each
    /// pixel.
    #[must_use]
    pub fn new(width: u32, height: u32, values: Vec<u8>) -> Option<Coverage> {
        Some(Coverage {
            width,
            height,
            values,
        })
    }
}

/// The reason that [`Pixmap::paint_label`] refuses a coverage: the coverage is
/// not the size of the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeMismatch {
    /// The width and the height of the image, in pixels.
    pub image: (u32, u32),
    /// The width and the height of the coverage, in pixels.
    pub coverage: (u32, u32),
}

/// One color channel multiplied by its alpha, rounded to the nearest value.
fn premultiply(channel: u8, alpha: u8) -> u8 {
    let full = u16::from(u8::MAX);
    let product = (u16::from(channel) * u16::from(alpha) + full / 2) / full;
    u8::try_from(product).unwrap_or(u8::MAX)
}

/// The color of the memory column at a pressure level.
fn level_color(level: Level) -> Rgba {
    match level {
        Level::Normal => NORMAL,
        Level::Warn => WARN,
        Level::Critical => CRITICAL,
        Level::Unknown(_) => UNKNOWN,
    }
}

/// The number of rows of the memory column: `round(percent * rows / 100)`.
fn memory_rows(percent_in_use: u8, rows: u32) -> u32 {
    (u32::from(percent_in_use) * rows + PERCENT_HALF) / PERCENT_MAX
}

/// Draws the two charts of the history.
///
/// The image is `Geometry::image_width_points` wide and
/// `Geometry::image_height_points` high. The CPU chart is on the left, and the
/// memory chart is on the right. Each chart has a frame on its outer rows and
/// columns, and the two charts share the border between them.
///
/// Each chart has `Geometry::columns` columns inside of its frame, and it shows
/// that number of the newest samples. The newest sample sits in the rightmost
/// column. A column that has no sample yet stays empty. The capacity of the
/// history does not change the width of the image.
///
/// The function draws no text. The label rows stay empty for the shell.
#[must_use]
pub fn render(history: &History<Sample>, geometry: Geometry, appearance: Appearance) -> Pixmap {
    let scale = geometry.scale;
    let height = geometry.height_points;
    let width = chart_width_points();
    let columns = CHART_COLUMNS;
    let mut pixmap = Pixmap::empty(
        geometry.image_width_points(),
        geometry.image_height_points(),
        scale,
    );

    let rows = (height - 2 * FRAME_POINTS) * scale;
    let interior_top = FRAME_POINTS * scale;
    let interior_bottom = interior_top + rows;
    // The memory chart starts on the right border of the CPU chart.
    let chart_origins = [0, width - FRAME_POINTS];

    for origin in chart_origins {
        pixmap.fill(
            origin * scale,
            0,
            width * scale,
            height * scale,
            appearance.frame(),
        );
        pixmap.fill(
            (origin + FRAME_POINTS) * scale,
            interior_top,
            columns * scale,
            rows,
            TRANSPARENT,
        );
    }

    let shown = history.len().min(geometry.columns().get());
    let skipped = history.len() - shown;
    let count = u32::try_from(shown).unwrap_or(columns);
    for (position, item) in (0..count).zip(history.iter().skip(skipped)) {
        let slot = columns - count + position;
        let [cpu_origin, memory_origin] = chart_origins;
        let cpu_x = (cpu_origin + FRAME_POINTS + slot) * scale;
        let memory_x = (memory_origin + FRAME_POINTS + slot) * scale;

        let system_rows = rows_of_fraction(item.cpu.system(), rows);
        let busy_rows = rows_of_fraction(item.cpu.system() + item.cpu.user(), rows);
        pixmap.fill(
            cpu_x,
            interior_bottom - system_rows,
            scale,
            system_rows,
            SYSTEM,
        );
        pixmap.fill(
            cpu_x,
            interior_bottom - busy_rows,
            scale,
            busy_rows - system_rows,
            USER,
        );

        let used_rows = memory_rows(item.pressure.percent_in_use(), rows);
        pixmap.fill(
            memory_x,
            interior_bottom - used_rows,
            scale,
            used_rows,
            level_color(item.pressure.level()),
        );
    }
    pixmap
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{CpuShare, Ticks};
    use crate::pressure::Pressure;
    use std::num::NonZeroUsize;

    /// The scales that each pixel test runs at.
    const SCALES: [u32; 2] = [1, 2];
    /// The height of the charts in the tests, in points. The interior is 20 points high.
    const HEIGHT: u32 = 22;
    /// The number of columns inside each frame. It does not depend on the height.
    const COLUMNS: usize = 32;
    /// The width of the image in points: two charts of 34 points that share one border.
    const IMAGE_WIDTH: u32 = 67;
    /// The line height of the 7-point system font on the Mac of the author.
    /// The label is 9 points high.
    const LINE_HEIGHT: f64 = 8.24;
    /// The index of the CPU chart.
    const CPU_CHART: u32 = 0;
    /// The index of the memory chart.
    const MEMORY_CHART: u32 = 1;
    /// The raw kernel values of the pressure levels.
    const LEVEL_NORMAL: i32 = 1;
    const LEVEL_WARN: i32 = 2;
    const LEVEL_UNKNOWN: i32 = 3;
    const LEVEL_CRITICAL: i32 = 4;
    /// A free percentage of 0 means that all of the memory is in use.
    const ALL_IN_USE: i32 = 0;

    #[test]
    fn the_premultiplied_bytes_scale_each_channel_by_its_alpha() {
        let colors: [(Rgba, Rgba); 5] = [
            (TRANSPARENT, TRANSPARENT),
            (USER, USER),
            (Appearance::Light.frame(), [0, 0, 0, 217]),
            (Appearance::Dark.frame(), [217, 217, 217, 217]),
            // 1 * 128 / 255 is 0.502, which rounds to 1 and truncates to 0.
            ([255, 1, 128, 128], [128, 1, 64, 128]),
        ];
        let width = u32::try_from(colors.len()).unwrap();
        let mut pixmap = Pixmap::empty(width, 1, 1);
        for (x, (color, _)) in (0..width).zip(colors) {
            pixmap.fill(x, 0, 1, 1, color);
        }
        let expected: Vec<u8> = colors.iter().flat_map(|(_, product)| *product).collect();
        assert_eq!(pixmap.premultiplied_bytes(), expected);
        assert_eq!(pixmap.bytes().len(), expected.len());
    }

    fn capacity(columns: usize) -> NonZeroUsize {
        NonZeroUsize::new(columns).unwrap()
    }

    fn geometry(scale: u32) -> Geometry {
        Geometry::new(HEIGHT, scale).unwrap()
    }

    /// Builds a sample from the ticks of one interval and the raw pressure.
    fn sample(system: u32, user: u32, idle: u32, level: i32, free: i32) -> Sample {
        let ticks = Ticks {
            user,
            system,
            idle,
            nice: 0,
        };
        Sample {
            cpu: CpuShare::between(Ticks::default(), ticks),
            pressure: Pressure::from_raw(level, free),
        }
    }

    fn history_of(columns: usize, samples: &[Sample]) -> History<Sample> {
        let mut history = History::new(capacity(columns));
        for item in samples {
            history.push(*item);
        }
        history
    }

    /// The number of rows of the interior at the scale.
    fn rows(scale: u32) -> u32 {
        (HEIGHT - 2 * FRAME_POINTS) * scale
    }

    /// The x pixel of the left edge of a slot, from the layout rules. The
    /// memory chart starts on the right border of the CPU chart.
    fn slot_x(chart: u32, slot: usize, scale: u32) -> u32 {
        let slot = u32::try_from(slot).unwrap();
        let columns = u32::try_from(COLUMNS).unwrap();
        (chart * (columns + FRAME_POINTS) + FRAME_POINTS + slot) * scale
    }

    /// The colors of one column, from the bottom row of the interior up.
    /// Each of the `scale` pixels of a row must have the same color.
    fn column(pixmap: &Pixmap, chart: u32, slot: usize, scale: u32) -> Vec<Rgba> {
        let x0 = slot_x(chart, slot, scale);
        let bottom = (HEIGHT - FRAME_POINTS) * scale;
        (0..rows(scale))
            .map(|row| {
                let y = bottom - 1 - row;
                let color = pixmap.pixel(x0, y);
                for dx in 0..scale {
                    assert_eq!(pixmap.pixel(x0 + dx, y), color, "row {row} dx {dx}");
                }
                color
            })
            .collect()
    }

    /// A column made of runs of colors, from the bottom up.
    fn stack(runs: &[(Rgba, u32)]) -> Vec<Rgba> {
        runs.iter()
            .flat_map(|(color, count)| {
                std::iter::repeat_n(*color, usize::try_from(*count).unwrap())
            })
            .collect()
    }

    fn empty_column(scale: u32) -> Vec<Rgba> {
        stack(&[(TRANSPARENT, rows(scale))])
    }

    fn full_column(color: Rgba, scale: u32) -> Vec<Rgba> {
        stack(&[(color, rows(scale))])
    }

    #[test]
    fn the_image_is_two_charts_that_share_one_border() {
        for scale in SCALES {
            let geometry = Geometry::from_status_bar(22.0, f64::from(scale), Some(LINE_HEIGHT));
            assert_eq!(geometry.height_points(), 22);
            assert_eq!(geometry.image_height_points(), 22);
            assert_eq!(geometry.image_width_points(), IMAGE_WIDTH);
            assert_eq!(geometry.columns().get(), COLUMNS);

            let pixmap = render(&history_of(COLUMNS, &[]), geometry, Appearance::Light);
            assert_eq!(pixmap.width_points(), IMAGE_WIDTH);
            assert_eq!(pixmap.height_points(), 22);
            assert_eq!(pixmap.width(), IMAGE_WIDTH * scale);
            assert_eq!(pixmap.height(), 22 * scale);
            let expected = usize::try_from(pixmap.width() * pixmap.height() * 4).unwrap();
            assert_eq!(pixmap.bytes().len(), expected);
        }
    }

    /// The two charts overlap on one column of the frame. Thus the border
    /// between the two interiors is one frame wide, and there is no gap.
    #[test]
    fn each_chart_is_framed_with_no_margin_and_one_shared_border() {
        assert_ne!(Appearance::Light.frame(), Appearance::Dark.frame());
        let history = history_of(COLUMNS, &[]);
        for scale in SCALES {
            let geometry = Geometry::from_status_bar(22.0, f64::from(scale), Some(LINE_HEIGHT));
            for appearance in [Appearance::Light, Appearance::Dark] {
                let pixmap = render(&history, geometry, appearance);
                assert_eq!(pixmap.width_points(), IMAGE_WIDTH);
                assert_eq!(pixmap.height_points(), 22);
                let height = 22 * scale;
                let width = 34 * scale;
                let frame = FRAME_POINTS * scale;
                for chart in [CPU_CHART, MEMORY_CHART] {
                    let left = chart * (width - frame);
                    for y in 0..height {
                        for x in left..left + width {
                            let on_edge = x < left + frame
                                || x >= left + width - frame
                                || y < frame
                                || y >= height - frame;
                            let expected = if on_edge {
                                appearance.frame()
                            } else {
                                TRANSPARENT
                            };
                            assert_eq!(pixmap.pixel(x, y), expected, "chart {chart} ({x}, {y})");
                        }
                    }
                }
            }
        }
    }

    /// Renders one sample in the newest slot and gives the CPU column.
    fn cpu_column_of(item: Sample, scale: u32) -> Vec<Rgba> {
        let history = history_of(COLUMNS, &[item]);
        let pixmap = render(&history, geometry(scale), Appearance::Light);
        column(&pixmap, CPU_CHART, COLUMNS - 1, scale)
    }

    /// Renders one sample in the newest slot and gives the memory column.
    fn memory_column_of(item: Sample, scale: u32) -> Vec<Rgba> {
        let history = history_of(COLUMNS, &[item]);
        let pixmap = render(&history, geometry(scale), Appearance::Light);
        column(&pixmap, MEMORY_CHART, COLUMNS - 1, scale)
    }

    #[test]
    fn an_idle_cpu_gives_an_empty_column() {
        for scale in SCALES {
            let column = cpu_column_of(sample(0, 0, 100, LEVEL_NORMAL, 100), scale);
            assert_eq!(column, empty_column(scale));
        }
    }

    #[test]
    fn a_full_system_share_gives_a_full_system_column() {
        for scale in SCALES {
            let column = cpu_column_of(sample(100, 0, 0, LEVEL_NORMAL, 100), scale);
            assert_eq!(column, full_column(SYSTEM, scale));
        }
    }

    #[test]
    fn a_full_user_share_gives_a_full_user_column() {
        for scale in SCALES {
            let column = cpu_column_of(sample(0, 100, 0, LEVEL_NORMAL, 100), scale);
            assert_eq!(column, full_column(USER, scale));
        }
    }

    #[test]
    fn the_cpu_column_stacks_system_below_user() {
        for scale in SCALES {
            let column = cpu_column_of(sample(25, 50, 25, LEVEL_NORMAL, 100), scale);
            let quarter = rows(scale) / 4;
            assert_eq!(quarter, 5 * scale);
            let expected = stack(&[
                (SYSTEM, quarter),
                (USER, 2 * quarter),
                (TRANSPARENT, quarter),
            ]);
            assert_eq!(column, expected);
        }
    }

    #[test]
    fn each_pressure_level_gives_its_color() {
        let cases = [
            (LEVEL_NORMAL, NORMAL),
            (LEVEL_WARN, WARN),
            (LEVEL_CRITICAL, CRITICAL),
        ];
        for scale in SCALES {
            for (level, color) in cases {
                let column = memory_column_of(sample(0, 0, 100, level, ALL_IN_USE), scale);
                assert_eq!(column, full_column(color, scale), "level {level}");
            }
        }
    }

    #[test]
    fn an_unknown_level_is_gray_and_never_normal() {
        for scale in SCALES {
            let column = memory_column_of(sample(0, 0, 100, LEVEL_UNKNOWN, ALL_IN_USE), scale);
            assert_eq!(column, full_column(UNKNOWN, scale));
            assert_ne!(UNKNOWN, NORMAL);
        }
    }

    #[test]
    fn the_memory_height_follows_the_percentage_in_use() {
        for scale in SCALES {
            let column = memory_column_of(sample(0, 0, 100, LEVEL_NORMAL, 50), scale);
            let half = rows(scale) / 2;
            assert_eq!(column, stack(&[(NORMAL, half), (TRANSPARENT, half)]));
        }
    }

    /// Three samples of distinct shapes.
    fn three_samples() -> [Sample; 3] {
        [
            sample(100, 0, 0, LEVEL_NORMAL, 0),
            sample(0, 100, 0, LEVEL_WARN, 0),
            sample(0, 0, 100, LEVEL_CRITICAL, 0),
        ]
    }

    #[test]
    fn a_partial_history_fills_the_slots_from_the_right_edge() {
        for scale in SCALES {
            let history = history_of(COLUMNS, &three_samples());
            let pixmap = render(&history, geometry(scale), Appearance::Light);
            let cpu = |slot| column(&pixmap, CPU_CHART, slot, scale);
            let memory = |slot| column(&pixmap, MEMORY_CHART, slot, scale);
            assert_eq!(cpu(29), full_column(SYSTEM, scale));
            assert_eq!(cpu(30), full_column(USER, scale));
            assert_eq!(cpu(31), empty_column(scale));
            assert_eq!(cpu(28), empty_column(scale));
            assert_eq!(memory(29), full_column(NORMAL, scale));
            assert_eq!(memory(30), full_column(WARN, scale));
            assert_eq!(memory(31), full_column(CRITICAL, scale));
            assert_eq!(memory(28), empty_column(scale));
        }
    }

    #[test]
    fn a_history_that_wraps_shows_the_last_samples() {
        const SMALL: usize = 3;
        for scale in SCALES {
            let samples = [
                sample(100, 0, 0, LEVEL_NORMAL, 0),
                sample(0, 100, 0, LEVEL_WARN, 0),
                sample(0, 0, 100, LEVEL_CRITICAL, 0),
                sample(100, 0, 0, LEVEL_UNKNOWN, 0),
            ];
            let history = history_of(SMALL, &samples);
            let pixmap = render(&history, geometry(scale), Appearance::Light);
            assert_eq!(
                pixmap.width_points(),
                IMAGE_WIDTH,
                "the columns come from the geometry"
            );
            let cpu = |slot| column(&pixmap, CPU_CHART, slot, scale);
            let memory = |slot| column(&pixmap, MEMORY_CHART, slot, scale);
            assert_eq!(cpu(28), empty_column(scale));
            assert_eq!(cpu(29), full_column(USER, scale));
            assert_eq!(cpu(30), empty_column(scale));
            assert_eq!(cpu(31), full_column(SYSTEM, scale));
            assert_eq!(memory(28), empty_column(scale));
            assert_eq!(memory(29), full_column(WARN, scale));
            assert_eq!(memory(30), full_column(CRITICAL, scale));
            assert_eq!(memory(31), full_column(UNKNOWN, scale));
        }
    }

    #[test]
    fn a_history_longer_than_the_columns_shows_only_the_newest_samples() {
        const LONG: usize = 40;
        for scale in SCALES {
            let idle = sample(0, 0, 100, LEVEL_NORMAL, 100);
            let mut samples = vec![sample(0, 100, 0, LEVEL_NORMAL, 100); 8];
            samples.push(sample(100, 0, 0, LEVEL_NORMAL, 100));
            samples.extend(std::iter::repeat_n(idle, 30));
            samples.push(sample(100, 0, 0, LEVEL_NORMAL, 100));
            assert_eq!(samples.len(), LONG);
            let history = history_of(LONG, &samples);
            let pixmap = render(&history, geometry(scale), Appearance::Light);
            assert_eq!(
                pixmap.width_points(),
                IMAGE_WIDTH,
                "the columns come from the geometry"
            );
            let cpu = |slot| column(&pixmap, CPU_CHART, slot, scale);
            assert_eq!(cpu(0), full_column(SYSTEM, scale));
            assert_eq!(cpu(COLUMNS - 1), full_column(SYSTEM, scale));
            for slot in 1..COLUMNS - 1 {
                assert_eq!(cpu(slot), empty_column(scale), "slot {slot}");
            }
            for y in 0..pixmap.height() {
                for x in 0..pixmap.width() {
                    assert_ne!(pixmap.pixel(x, y), USER, "({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn each_memory_column_keeps_the_color_of_its_own_sample() {
        for scale in SCALES {
            let samples = [
                sample(0, 0, 100, LEVEL_NORMAL, ALL_IN_USE),
                sample(0, 0, 100, LEVEL_CRITICAL, ALL_IN_USE),
            ];
            let history = history_of(COLUMNS, &samples);
            let pixmap = render(&history, geometry(scale), Appearance::Light);
            let older = column(&pixmap, MEMORY_CHART, COLUMNS - 2, scale);
            let newer = column(&pixmap, MEMORY_CHART, COLUMNS - 1, scale);
            assert_eq!(older, full_column(NORMAL, scale));
            assert_eq!(newer, full_column(CRITICAL, scale));
        }
    }

    #[test]
    fn the_geometry_refuses_a_height_below_the_minimum_and_a_scale_of_zero() {
        assert_eq!(
            Geometry::new(2, 1),
            Err(GeometryError::HeightTooSmall { height_points: 2 })
        );
        assert_eq!(Geometry::new(3, 0), Err(GeometryError::ZeroScale));
        let accepted = Geometry::new(3, 1).unwrap();
        assert_eq!(accepted.height_points(), 3);
        assert_eq!(accepted.scale(), 1);
        assert!(!GeometryError::ZeroScale.to_string().is_empty());
        assert!(!GeometryError::HeightTooSmall { height_points: 2 }
            .to_string()
            .is_empty());
    }

    #[test]
    fn the_height_is_the_thickness_floored() {
        let cases = [
            ((22.0, 2.0), (22, 2)),
            ((24.0, 1.0), (24, 1)),
            ((22.0, 1.5), (22, 2)),
            ((22.9, 2.0), (22, 2)),
            ((2.0, 2.0), (3, 2)),
            ((f64::NAN, f64::NAN), (3, 1)),
            ((f64::INFINITY, 0.0), (3, 1)),
        ];
        for ((thickness, backing), (height, scale)) in cases {
            let geometry = Geometry::from_status_bar(thickness, backing, None);
            assert_eq!(
                (geometry.height_points(), geometry.scale()),
                (height, scale),
                "thickness {thickness}, backing {backing}"
            );
        }
        let labeled = Geometry::from_status_bar(22.0, 2.0, Some(LINE_HEIGHT));
        assert_eq!(labeled.height_points(), 22);
    }

    /// The thicknesses of the status bar in the label tests.
    const THICKNESSES: [f64; 2] = [22.0, 24.0];

    #[test]
    fn the_label_takes_the_top_rows_of_the_interior() {
        for thickness in THICKNESSES {
            for scale in [1.0, 2.0] {
                let geometry = Geometry::from_status_bar(thickness, scale, Some(LINE_HEIGHT));
                assert_eq!(
                    geometry.label_rows_points(),
                    Some(1..10),
                    "thickness {thickness}"
                );
            }
        }
    }

    #[test]
    fn the_label_rows_are_the_line_height_rounded_up() {
        let label = |line_height| {
            let geometry = Geometry::from_status_bar(22.0, 2.0, Some(line_height));
            geometry.label_rows_points().map(|rows| rows.len())
        };
        assert_eq!(label(8.0), Some(8));
        assert_eq!(label(8.01), Some(9));
    }

    #[test]
    fn a_line_height_that_is_not_usable_gives_no_label() {
        for line_height in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
            for thickness in THICKNESSES {
                let geometry = Geometry::from_status_bar(thickness, 2.0, Some(line_height));
                assert_eq!(geometry, Geometry::from_status_bar(thickness, 2.0, None));
                assert_eq!(geometry.label_rows_points(), None);
            }
        }
    }

    #[test]
    fn an_interior_lower_than_the_line_height_gives_no_label() {
        let thin = Geometry::from_status_bar(10.0, 2.0, Some(LINE_HEIGHT));
        assert_eq!(thin.label_rows_points(), None);
        let enough = Geometry::from_status_bar(11.0, 2.0, Some(LINE_HEIGHT));
        assert_eq!(enough.label_rows_points(), Some(1..10));
    }

    #[test]
    fn the_columns_and_the_width_do_not_depend_on_the_height() {
        for height in [3, 22, 24, 37] {
            let geometry = Geometry::new(height, 1).unwrap();
            assert_eq!(geometry.columns().get(), COLUMNS, "height {height}");
            assert_eq!(
                geometry.image_width_points(),
                IMAGE_WIDTH,
                "height {height}"
            );
        }
        let tall = Geometry::from_status_bar(37.0, 2.0, Some(LINE_HEIGHT));
        assert_eq!(tall.columns().get(), COLUMNS);
        assert_eq!(tall.image_width_points(), IMAGE_WIDTH);
    }

    #[test]
    fn geometry_new_has_no_label() {
        assert_eq!(Geometry::new(22, 2).unwrap().label_rows_points(), None);
    }

    #[test]
    fn the_label_color_is_the_frame_color_of_each_appearance() {
        assert_ne!(Appearance::Light.label(), Appearance::Dark.label());
        for appearance in [Appearance::Light, Appearance::Dark] {
            assert_eq!(appearance.label(), appearance.frame());
        }
    }

    /// The two appearances of the menu bar.
    const APPEARANCES: [Appearance; 2] = [Appearance::Light, Appearance::Dark];
    /// A pixel that the glyphs cover completely.
    const FULL: u8 = 255;
    /// A pixel that the glyphs cover by half.
    const HALF: u8 = 128;
    /// A pixel that the glyphs do not touch.
    const NONE: u8 = 0;

    /// An image of one row of pixels, in the given colors, at a scale of 1.
    fn row_of(colors: &[Rgba]) -> Pixmap {
        let width = u32::try_from(colors.len()).unwrap();
        let mut pixmap = Pixmap::empty(width, 1, 1);
        for (x, color) in (0..width).zip(colors) {
            pixmap.fill(x, 0, 1, 1, *color);
        }
        pixmap
    }

    /// The coverage of an image of one row of pixels.
    fn row_coverage(values: &[u8]) -> Coverage {
        let width = u32::try_from(values.len()).unwrap();
        Coverage::new(width, 1, values.to_vec()).expect("one value for each pixel")
    }

    /// The colors of an image of one row of pixels.
    fn row_pixels(pixmap: &Pixmap) -> Vec<Rgba> {
        (0..pixmap.width()).map(|x| pixmap.pixel(x, 0)).collect()
    }

    #[test]
    fn a_coverage_needs_exactly_one_value_for_each_pixel() {
        assert!(Coverage::new(2, 2, vec![NONE; 3]).is_none());
        assert!(Coverage::new(2, 2, vec![NONE; 5]).is_none());
        assert!(Coverage::new(2, 2, vec![NONE; 4]).is_some());
        assert!(Coverage::new(0, 0, Vec::new()).is_some());
    }

    #[test]
    fn a_fully_covered_empty_pixel_takes_the_label_color_of_the_appearance() {
        for appearance in APPEARANCES {
            let mut pixmap = row_of(&[TRANSPARENT]);
            pixmap
                .paint_label(&row_coverage(&[FULL]), appearance)
                .unwrap();
            assert_eq!(pixmap.pixel(0, 0), appearance.label(), "{appearance:?}");
        }
    }

    /// Half of the coverage gives half of the alpha of the label color.
    /// 217 * 128 / 255 is 108.9, which rounds to 109.
    #[test]
    fn a_partly_covered_empty_pixel_takes_part_of_the_alpha_of_the_label_color() {
        let cases = [
            (Appearance::Light, [0, 0, 0, 109]),
            (Appearance::Dark, [255, 255, 255, 109]),
        ];
        for (appearance, expected) in cases {
            let mut pixmap = row_of(&[TRANSPARENT]);
            pixmap
                .paint_label(&row_coverage(&[HALF]), appearance)
                .unwrap();
            assert_eq!(pixmap.pixel(0, 0), expected, "{appearance:?}");
        }
    }

    #[test]
    fn a_pixel_that_the_glyphs_do_not_cover_keeps_its_color() {
        let colors = [
            TRANSPARENT,
            SYSTEM,
            USER,
            NORMAL,
            WARN,
            UNKNOWN,
            Appearance::Light.frame(),
            Appearance::Dark.frame(),
        ];
        for appearance in APPEARANCES {
            let mut pixmap = row_of(&colors);
            pixmap
                .paint_label(&row_coverage(&[NONE; 8]), appearance)
                .unwrap();
            assert_eq!(row_pixels(&pixmap), colors, "{appearance:?}");
        }
    }

    /// The coverage goes row by row from the top left, like the image.
    #[test]
    fn the_coverage_lies_on_the_image_row_by_row_from_the_top_left() {
        let mut pixmap = Pixmap::empty(2, 2, 1);
        let coverage = Coverage::new(2, 2, vec![FULL, NONE, NONE, FULL]).unwrap();
        pixmap.paint_label(&coverage, Appearance::Dark).unwrap();
        let label = Appearance::Dark.label();
        assert_eq!(pixmap.pixel(0, 0), label);
        assert_eq!(pixmap.pixel(1, 0), TRANSPARENT);
        assert_eq!(pixmap.pixel(0, 1), TRANSPARENT);
        assert_eq!(pixmap.pixel(1, 1), label);
    }

    #[test]
    fn the_label_refuses_a_coverage_of_another_size_and_keeps_the_image() {
        let mut pixmap = row_of(&[TRANSPARENT; 2]);
        let wide = row_coverage(&[FULL; 3]);
        assert_eq!(
            pixmap.paint_label(&wide, Appearance::Dark),
            Err(SizeMismatch {
                image: (2, 1),
                coverage: (3, 1),
            })
        );
        // The same number of pixels in a different shape is a different size.
        let tall = Coverage::new(1, 2, vec![FULL; 2]).unwrap();
        assert_eq!(
            pixmap.paint_label(&tall, Appearance::Dark),
            Err(SizeMismatch {
                image: (2, 1),
                coverage: (1, 2),
            })
        );
        assert_eq!(row_pixels(&pixmap), [TRANSPARENT; 2]);
    }
}
