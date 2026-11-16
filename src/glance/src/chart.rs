//! The two charts of the status item, drawn into a pixel buffer.
//!
//! The left chart shows the CPU share. The right chart shows the memory in use,
//! in the color of the pressure level. Each chart has one column for each
//! sample that the history holds, and the newest sample sits at the right edge.
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
/// The width of the gap between the two charts, in points.
pub const GAP_POINTS: u32 = 4;
/// The space above and below the charts in the status bar, in points.
pub const VERTICAL_INSET_POINTS: u32 = 3;
/// The space above the charts when the label is on, in points.
pub const LABEL_TOP_INSET_POINTS: u32 = 1;
/// The gap between the charts and the label band, in points.
pub const LABEL_GAP_POINTS: u32 = 1;

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

/// The height of the charts in points, and the pixels per point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    height_points: u32,
    scale: u32,
    band_points: Option<u32>,
}

impl Geometry {
    /// The smallest height. A frame, one row, and a frame.
    pub const MIN_HEIGHT_POINTS: u32 = 3;

    /// Makes a geometry.
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
            band_points: None,
        })
    }

    /// Makes the geometry of the status bar of the screen.
    ///
    /// The inputs are `NSStatusBar.thickness`, `NSScreen.backingScaleFactor`,
    /// and the line height of the label text. The line height is `None` when
    /// there is no label. The scale is the backing scale rounded, and at least 1.
    ///
    /// The band is the line height rounded up to whole points. A line height
    /// that is not finite or not above 0 gives no band. With a band, the charts
    /// take the whole points of the thickness less the inset above, the gap,
    /// and the band. If that height is below `MIN_HEIGHT_POINTS`, the label goes
    /// away. Without a band, the height is the whole points of the thickness
    /// less the inset above and below, and at least `MIN_HEIGHT_POINTS`. An
    /// input that is not finite gives the minimum.
    #[must_use]
    pub fn from_status_bar(
        thickness_points: f64,
        backing_scale: f64,
        label_line_height_points: Option<f64>,
    ) -> Geometry {
        let thickness = whole_number(thickness_points.floor());
        let scale = whole_number(backing_scale.round()).max(1);
        let labeled = label_line_height_points
            .filter(|line_height| line_height.is_finite() && *line_height > 0.0)
            .map(|line_height| whole_number(line_height.ceil()))
            .and_then(|band| {
                let height_points = thickness
                    .saturating_sub(LABEL_TOP_INSET_POINTS + LABEL_GAP_POINTS)
                    .saturating_sub(band);
                (height_points >= Self::MIN_HEIGHT_POINTS).then_some(Geometry {
                    height_points,
                    scale,
                    band_points: Some(band),
                })
            });
        labeled.unwrap_or_else(|| Geometry {
            height_points: thickness
                .saturating_sub(2 * VERTICAL_INSET_POINTS)
                .max(Self::MIN_HEIGHT_POINTS),
            scale,
            band_points: None,
        })
    }

    /// The height of the whole image in points. It holds the charts and the label band.
    #[must_use]
    pub fn image_height_points(self) -> u32 {
        match self.band_points {
            Some(band) => LABEL_TOP_INSET_POINTS + self.height_points + LABEL_GAP_POINTS + band,
            None => self.height_points,
        }
    }

    /// The rows of the charts, in points from the top of the image.
    #[must_use]
    pub fn chart_rows_points(self) -> std::ops::Range<u32> {
        let top = if self.band_points.is_some() {
            LABEL_TOP_INSET_POINTS
        } else {
            0
        };
        top..top + self.height_points
    }

    /// The rows of the label band, in points from the top of the image.
    /// It is `None` when there is no label. The band touches the bottom edge.
    #[must_use]
    pub fn band_rows_points(self) -> Option<std::ops::Range<u32>> {
        self.band_points
            .map(|band| self.image_height_points() - band..self.image_height_points())
    }

    /// The height of the charts in points.
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
            .chunks_exact(BYTES_PER_PIXEL)
            .flat_map(|pixel| {
                let alpha = pixel[ALPHA_INDEX];
                let mut product = [alpha; BYTES_PER_PIXEL];
                for (channel, value) in product.iter_mut().zip(&pixel[..ALPHA_INDEX]) {
                    *channel = premultiply(*value, alpha);
                }
                product
            })
            .collect()
    }
}

/// The index of the alpha channel in one pixel.
const ALPHA_INDEX: usize = 3;

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
/// The image is `Geometry::image_height_points` high. The charts start at the
/// top of `Geometry::chart_rows_points`. The rows of the label band, and the
/// rows above and below the charts, stay transparent.
///
/// Each chart has `history.capacity()` columns inside of a frame. The CPU chart
/// is on the left, then a transparent gap, then the memory chart. The newest
/// sample sits in the rightmost column of each chart. A column that has no
/// sample yet stays empty.
///
/// # Panics
///
/// Panics if the capacity of the history does not fit in a `u32`.
#[must_use]
pub fn render(history: &History<Sample>, geometry: Geometry, appearance: Appearance) -> Pixmap {
    let scale = geometry.scale;
    let capacity = u32::try_from(history.capacity()).expect("the capacity fits in a u32");
    let chart_width = capacity + 2 * FRAME_POINTS;
    let mut pixmap = Pixmap::empty(
        CHART_COUNT * chart_width + GAP_POINTS,
        geometry.image_height_points(),
        scale,
    );

    let rows = (geometry.height_points - 2 * FRAME_POINTS) * scale;
    let chart_top = geometry.chart_rows_points().start * scale;
    let interior_top = chart_top + FRAME_POINTS * scale;
    let interior_bottom = interior_top + rows;
    let chart_origins = [0, chart_width + GAP_POINTS];

    for origin in chart_origins {
        pixmap.fill(
            origin * scale,
            chart_top,
            chart_width * scale,
            geometry.height_points * scale,
            appearance.frame(),
        );
        pixmap.fill(
            (origin + FRAME_POINTS) * scale,
            interior_top,
            capacity * scale,
            rows,
            TRANSPARENT,
        );
    }

    let count = u32::try_from(history.len()).unwrap_or(capacity);
    for (position, item) in (0..count).zip(history.iter()) {
        let slot = capacity - count + position;
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
    /// The height of the charts in the tests. The interior is 16 points high.
    const HEIGHT_POINTS: u32 = 18;
    /// The number of columns of the charts in most tests.
    const CAPACITY: usize = 30;
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
        Geometry::new(HEIGHT_POINTS, scale).unwrap()
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
        (HEIGHT_POINTS - 2 * FRAME_POINTS) * scale
    }

    /// The x pixel of the left edge of a slot, from the layout rules.
    fn slot_x(columns: usize, chart: u32, slot: usize, scale: u32) -> u32 {
        let columns = u32::try_from(columns).unwrap();
        let slot = u32::try_from(slot).unwrap();
        let chart_origin = chart * (columns + 2 * FRAME_POINTS + GAP_POINTS);
        (chart_origin + FRAME_POINTS + slot) * scale
    }

    /// The colors of one column, from the bottom row of the interior up.
    /// Each of the `scale` pixels of a row must have the same color.
    fn column(pixmap: &Pixmap, columns: usize, chart: u32, slot: usize, scale: u32) -> Vec<Rgba> {
        let x0 = slot_x(columns, chart, slot, scale);
        let bottom = (HEIGHT_POINTS - FRAME_POINTS) * scale;
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
    fn the_pixmap_has_the_size_of_the_layout() {
        let history = history_of(CAPACITY, &[]);
        for scale in SCALES {
            let pixmap = render(&history, geometry(scale), Appearance::Light);
            assert_eq!(pixmap.width(), 68 * scale);
            assert_eq!(pixmap.height(), 18 * scale);
            assert_eq!(pixmap.width_points(), 68);
            assert_eq!(pixmap.height_points(), 18);
            let expected = usize::try_from(pixmap.width() * pixmap.height() * 4).unwrap();
            assert_eq!(pixmap.bytes().len(), expected);
        }
    }

    #[test]
    fn the_frame_has_the_color_of_the_appearance_and_the_gap_is_transparent() {
        assert_ne!(Appearance::Light.frame(), Appearance::Dark.frame());
        let history = history_of(CAPACITY, &[]);
        for scale in SCALES {
            for appearance in [Appearance::Light, Appearance::Dark] {
                let pixmap = render(&history, geometry(scale), appearance);
                for chart in [CPU_CHART, MEMORY_CHART] {
                    let left = slot_x(CAPACITY, chart, 0, scale) - FRAME_POINTS * scale;
                    let width = (u32::try_from(CAPACITY).unwrap() + 2 * FRAME_POINTS) * scale;
                    for y in 0..pixmap.height() {
                        for x in left..left + width {
                            let on_edge = x < left + FRAME_POINTS * scale
                                || x >= left + width - FRAME_POINTS * scale
                                || y < FRAME_POINTS * scale
                                || y >= pixmap.height() - FRAME_POINTS * scale;
                            let expected = if on_edge {
                                appearance.frame()
                            } else {
                                TRANSPARENT
                            };
                            assert_eq!(pixmap.pixel(x, y), expected, "chart {chart} ({x}, {y})");
                        }
                    }
                }
                let gap_left = (u32::try_from(CAPACITY).unwrap() + 2 * FRAME_POINTS) * scale;
                for y in 0..pixmap.height() {
                    for x in gap_left..gap_left + GAP_POINTS * scale {
                        assert_eq!(pixmap.pixel(x, y), TRANSPARENT, "gap ({x}, {y})");
                    }
                }
            }
        }
    }

    /// Renders one sample in the newest slot and gives the CPU column.
    fn cpu_column_of(item: Sample, scale: u32) -> Vec<Rgba> {
        let history = history_of(CAPACITY, &[item]);
        let pixmap = render(&history, geometry(scale), Appearance::Light);
        column(&pixmap, CAPACITY, CPU_CHART, CAPACITY - 1, scale)
    }

    /// Renders one sample in the newest slot and gives the memory column.
    fn memory_column_of(item: Sample, scale: u32) -> Vec<Rgba> {
        let history = history_of(CAPACITY, &[item]);
        let pixmap = render(&history, geometry(scale), Appearance::Light);
        column(&pixmap, CAPACITY, MEMORY_CHART, CAPACITY - 1, scale)
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
            let history = history_of(CAPACITY, &three_samples());
            let pixmap = render(&history, geometry(scale), Appearance::Light);
            let cpu = |slot| column(&pixmap, CAPACITY, CPU_CHART, slot, scale);
            let memory = |slot| column(&pixmap, CAPACITY, MEMORY_CHART, slot, scale);
            assert_eq!(cpu(27), full_column(SYSTEM, scale));
            assert_eq!(cpu(28), full_column(USER, scale));
            assert_eq!(cpu(29), empty_column(scale));
            assert_eq!(cpu(26), empty_column(scale));
            assert_eq!(memory(27), full_column(NORMAL, scale));
            assert_eq!(memory(28), full_column(WARN, scale));
            assert_eq!(memory(29), full_column(CRITICAL, scale));
            assert_eq!(memory(26), empty_column(scale));
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
            let cpu = |slot| column(&pixmap, SMALL, CPU_CHART, slot, scale);
            let memory = |slot| column(&pixmap, SMALL, MEMORY_CHART, slot, scale);
            assert_eq!(cpu(0), full_column(USER, scale));
            assert_eq!(cpu(1), empty_column(scale));
            assert_eq!(cpu(2), full_column(SYSTEM, scale));
            assert_eq!(memory(0), full_column(WARN, scale));
            assert_eq!(memory(1), full_column(CRITICAL, scale));
            assert_eq!(memory(2), full_column(UNKNOWN, scale));
        }
    }

    #[test]
    fn each_memory_column_keeps_the_color_of_its_own_sample() {
        for scale in SCALES {
            let samples = [
                sample(0, 0, 100, LEVEL_NORMAL, ALL_IN_USE),
                sample(0, 0, 100, LEVEL_CRITICAL, ALL_IN_USE),
            ];
            let history = history_of(CAPACITY, &samples);
            let pixmap = render(&history, geometry(scale), Appearance::Light);
            let older = column(&pixmap, CAPACITY, MEMORY_CHART, CAPACITY - 2, scale);
            let newer = column(&pixmap, CAPACITY, MEMORY_CHART, CAPACITY - 1, scale);
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
    fn the_geometry_of_the_status_bar_follows_the_thickness_and_the_scale() {
        let cases = [
            ((22.0, 2.0), (16, 2)),
            ((24.0, 1.0), (18, 1)),
            ((22.0, 1.5), (16, 2)),
            ((4.0, 2.0), (3, 2)),
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
    }

    /// The line height of the label text in the tests. The band is 9 points.
    const LINE_HEIGHT: f64 = 8.35;
    /// The band that `LINE_HEIGHT` gives.
    const BAND_POINTS: u32 = 9;
    /// The thicknesses of the status bar in the label tests.
    const THICKNESSES: [f64; 2] = [22.0, 24.0];

    #[test]
    fn a_label_band_takes_the_rows_below_the_charts() {
        let cases = [(22.0, 11, 22, 1..12, 13..22), (24.0, 13, 24, 1..14, 15..24)];
        for (thickness, height, image, charts, band) in cases {
            for scale in [1.0, 2.0] {
                let geometry = Geometry::from_status_bar(thickness, scale, Some(LINE_HEIGHT));
                assert_eq!(geometry.height_points(), height, "thickness {thickness}");
                assert_eq!(geometry.image_height_points(), image);
                assert_eq!(geometry.chart_rows_points(), charts.clone());
                assert_eq!(band.len(), usize::try_from(BAND_POINTS).unwrap());
                assert_eq!(geometry.band_rows_points(), Some(band.clone()));
            }
        }
    }

    #[test]
    fn the_band_is_the_line_height_rounded_up() {
        let band = |line_height| {
            let geometry = Geometry::from_status_bar(22.0, 2.0, Some(line_height));
            geometry.band_rows_points().map(|rows| rows.len())
        };
        assert_eq!(band(8.0), Some(8));
        assert_eq!(band(8.01), Some(9));
    }

    #[test]
    fn a_line_height_that_is_not_usable_gives_no_band() {
        for line_height in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
            for thickness in THICKNESSES {
                let geometry = Geometry::from_status_bar(thickness, 2.0, Some(line_height));
                assert_eq!(geometry, Geometry::from_status_bar(thickness, 2.0, None));
                assert_eq!(geometry.band_rows_points(), None);
            }
        }
    }

    #[test]
    fn a_thickness_that_cannot_hold_the_band_drops_the_label() {
        let geometry = Geometry::from_status_bar(12.0, 2.0, Some(LINE_HEIGHT));
        assert_eq!(geometry, Geometry::from_status_bar(12.0, 2.0, None));
        assert_eq!(geometry.band_rows_points(), None);
    }

    #[test]
    fn a_geometry_without_a_band_keeps_the_layout_of_the_charts_only() {
        let geometry = geometry(2);
        assert_eq!(geometry.image_height_points(), HEIGHT_POINTS);
        assert_eq!(geometry.chart_rows_points(), 0..HEIGHT_POINTS);
        assert_eq!(geometry.band_rows_points(), None);
    }

    #[test]
    fn a_label_band_keeps_its_rows_transparent_and_moves_the_charts_down() {
        let samples = vec![sample(100, 0, 0, LEVEL_NORMAL, ALL_IN_USE); CAPACITY];
        let history = history_of(CAPACITY, &samples);
        for thickness in THICKNESSES {
            for scale in SCALES {
                let geometry =
                    Geometry::from_status_bar(thickness, f64::from(scale), Some(LINE_HEIGHT));
                let pixmap = render(&history, geometry, Appearance::Light);
                let thickness_points = whole_number(thickness);
                assert_eq!(pixmap.width(), 68 * scale);
                assert_eq!(pixmap.height(), thickness_points * scale);

                let band = geometry.band_rows_points().unwrap();
                let gap_row = geometry.chart_rows_points().end;
                let transparent_rows = (0..LABEL_TOP_INSET_POINTS)
                    .chain(gap_row..gap_row + LABEL_GAP_POINTS)
                    .chain(band);
                for point_row in transparent_rows {
                    for y in point_row * scale..(point_row + 1) * scale {
                        for x in 0..pixmap.width() {
                            assert_eq!(pixmap.pixel(x, y), TRANSPARENT, "({x}, {y})");
                        }
                    }
                }

                let x = slot_x(CAPACITY, CPU_CHART, CAPACITY - 1, scale);
                let chart_top = LABEL_TOP_INSET_POINTS * scale;
                let chart_bottom = (LABEL_TOP_INSET_POINTS + geometry.height_points()) * scale;
                let system: Vec<u32> = (0..pixmap.height())
                    .filter(|&y| pixmap.pixel(x, y) == SYSTEM)
                    .collect();
                let expected = (thickness_points - 1 - 1 - BAND_POINTS - 2 * FRAME_POINTS) * scale;
                assert_eq!(u32::try_from(system.len()).unwrap(), expected);
                assert_eq!(system.first(), Some(&(chart_top + FRAME_POINTS * scale)));
                assert_eq!(
                    system.last(),
                    Some(&(chart_bottom - FRAME_POINTS * scale - 1))
                );
                let frame = Appearance::Light.frame();
                assert_eq!(pixmap.pixel(x, chart_top), frame);
                assert_eq!(pixmap.pixel(x, chart_bottom - 1), frame);
            }
        }
    }

    #[test]
    fn the_label_color_is_the_frame_color_of_each_appearance() {
        assert_ne!(Appearance::Light.label(), Appearance::Dark.label());
        for appearance in [Appearance::Light, Appearance::Dark] {
            assert_eq!(appearance.label(), appearance.frame());
        }
    }
}
