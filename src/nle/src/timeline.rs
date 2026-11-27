//! The layout of the timeline: the map from time to pixels, the zoom, the
//! scroll, the ruler, the event density of the clips, and the marks where
//! the terminal size changes. The functions are
//! pure, so a test needs no webview.
//!
//! The timeline shows project time: the times of the clips on their tracks,
//! before the filters change them.

use crate::format::format_clock;
use crate::types::{Clip, EditProject};

mod density;
pub use density::{
    density_path, tile_width, visible_tiles, ClipEvents, EventCache, OVERSCAN, TILE_WIDTH,
};

/// The largest zoom, in pixels per second: one pixel for each millisecond.
pub const MAX_PIXELS_PER_SECOND: f64 = 1000.0;
/// The factor of one step of a zoom button or a zoom key. Remotion Studio
/// uses the same factor.
pub const ZOOM_STEP: f64 = 1.2;
/// The rate of a wheel zoom: the zoom changes by the factor
/// `exp(-WHEEL_ZOOM_RATE * delta)` for a vertical delta in pixels. Remotion
/// Studio uses the same rate.
const WHEEL_ZOOM_RATE: f64 = 0.06;
/// The largest factor of one wheel event. One notch of some mouse wheels
/// gives a delta of about 100 pixels, which the rate above makes a factor of
/// `exp(-6)`. This limit keeps one notch from a jump to the end of the range.
const MAX_WHEEL_ZOOM_FACTOR: f64 = 2.0;
/// The pixels of one line of a wheel event that counts in lines.
const WHEEL_LINE_PIXELS: f64 = 16.0;
/// The steps between two labels of the ruler, in seconds. The ruler takes the
/// first step that leaves `LABEL_SPACING` pixels for each label. The steps
/// from one second are the steps of Remotion Studio. The steps below one
/// second are for the largest zooms, because the timeline counts time in
/// seconds and Remotion Studio counts frames.
const LABEL_STEPS: [f64; 18] = [
    0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 180.0, 300.0, 600.0, 900.0,
    1200.0, 1800.0, 3600.0,
];
/// The smallest space for one label of the ruler, in pixels.
const LABEL_SPACING: f64 = 80.0;
/// The smallest space between two ticks of the ruler, in pixels. Remotion
/// Studio uses the same space.
const MIN_TICK_SPACING: f64 = 16.0;
/// The counts of ticks in one step between two labels. The ruler takes the
/// first count that keeps `MIN_TICK_SPACING` between two ticks.
const TICK_DIVISIONS: [u32; 3] = [10, 5, 2];
/// The most doublings of the last label step. The largest zoom is above
/// zero, so a finite zoom needs far fewer.
const MAX_STEP_DOUBLINGS: u32 = 1100;
/// The most ticks of one ruler. A window of a sane width holds about one
/// tick for each `MIN_TICK_SPACING` pixels, far below this count.
const MAX_TICKS: i64 = 100_000;
/// The part of a tick step that a tick can be past an edge of the window
/// and still count as in the window.
const EDGE_MARGIN: f64 = 1e-9;
/// The largest integer below which an `f64` holds each integer: 2^53.
const MAX_EXACT_INTEGER: f64 = 9_007_199_254_740_992.0;
/// The smallest width of the window in pixels. A window that has no layout
/// yet counts as this width, so no division is by zero.
const MIN_WIDTH: f64 = 1.0;

/// What the timeline shows: the length of the project in seconds, and the
/// width of the window in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extent {
    duration: f64,
    width: f64,
}

impl Extent {
    /// Make an extent. A duration that is not finite, or that is below zero,
    /// counts as zero. A width that is not finite, or that is below
    /// `MIN_WIDTH`, counts as `MIN_WIDTH`.
    #[must_use]
    pub fn new(duration: f64, width: f64) -> Self {
        let duration = if duration.is_finite() && duration > 0.0 {
            duration
        } else {
            0.0
        };
        let width = if width.is_finite() && width > MIN_WIDTH {
            width
        } else {
            MIN_WIDTH
        };
        Self { duration, width }
    }

    /// The zoom that shows the whole project in the window. It is never above
    /// `MAX_PIXELS_PER_SECOND`, and it is above zero.
    fn fit(self) -> f64 {
        let fit = self.width / self.duration;
        // An empty project gives infinity, and `min` holds it at the largest
        // zoom. A very long project gives a very small zoom.
        if fit > 0.0 {
            fit.min(MAX_PIXELS_PER_SECOND)
        } else {
            f64::MIN_POSITIVE
        }
    }
}

/// The zoom and the scroll of the timeline.
///
/// The default view shows the whole project. A view that shows the whole
/// project stays that way when the window or the project changes its size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct View {
    /// The zoom in pixels per second, or `None` for the zoom that shows the
    /// whole project.
    zoom: Option<f64>,
    /// The time at the left edge of the window, in seconds.
    start: f64,
}

impl View {
    /// The zoom and the scroll of this view in `extent`. The zoom is at least
    /// the zoom that shows the whole project, and at most
    /// `MAX_PIXELS_PER_SECOND`. The start is at least zero, and the window
    /// never goes past the end of the project, unless the whole project is
    /// shorter than the window.
    #[must_use]
    pub fn frame(self, extent: Extent) -> Frame {
        let fit = extent.fit();
        let pixels_per_second = match self.zoom {
            Some(zoom) if zoom.is_finite() => zoom.clamp(fit, MAX_PIXELS_PER_SECOND),
            _ => fit,
        };
        let last_start = (extent.duration - extent.width / pixels_per_second).max(0.0);
        let start = if self.start.is_finite() {
            self.start.clamp(0.0, last_start)
        } else {
            0.0
        };
        Frame {
            pixels_per_second,
            start,
            width: extent.width,
        }
    }

    /// The view after a zoom by `factor` that keeps the time at `anchor`, a
    /// place in the window in pixels, at the same place. A factor above 1
    /// zooms in. A factor that is not finite, or that is not above zero,
    /// changes nothing. An anchor that is not finite is the center of the
    /// window.
    #[must_use]
    pub fn zoom(self, extent: Extent, factor: f64, anchor: f64) -> Self {
        if !factor.is_finite() || factor <= 0.0 {
            return self;
        }
        let frame = self.frame(extent);
        let anchor = if anchor.is_finite() {
            anchor
        } else {
            frame.width / 2.0
        };
        let time = frame.time_at(anchor);
        let fit = extent.fit();
        let pixels_per_second =
            (frame.pixels_per_second * factor).clamp(fit, MAX_PIXELS_PER_SECOND);
        // At the zoom of the whole project, the view follows later changes of
        // the window and of the project.
        let zoom = (pixels_per_second > fit).then_some(pixels_per_second);
        Self {
            zoom,
            start: time - anchor / pixels_per_second,
        }
        .clamped(extent)
    }

    /// The view after a scroll by `pixels`. A positive value shows later
    /// times. A value that is not finite changes nothing.
    #[must_use]
    pub fn pan(self, extent: Extent, pixels: f64) -> Self {
        if !pixels.is_finite() {
            return self;
        }
        let frame = self.frame(extent);
        Self {
            zoom: self.zoom,
            start: frame.start + pixels / frame.pixels_per_second,
        }
        .clamped(extent)
    }

    /// This view with the start that `frame` gives it in `extent`.
    fn clamped(self, extent: Extent) -> Self {
        Self {
            zoom: self.zoom,
            start: self.frame(extent).start,
        }
    }
}

/// The zoom and the scroll that the timeline draws: a map from time to
/// pixels in the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pixels_per_second: f64,
    start: f64,
    width: f64,
}

impl Frame {
    /// The zoom in pixels per second. It is above zero.
    #[must_use]
    pub fn pixels_per_second(self) -> f64 {
        self.pixels_per_second
    }

    /// The time at the left edge of the window, in seconds.
    #[must_use]
    pub fn start(self) -> f64 {
        self.start
    }

    /// The time at the right edge of the window, in seconds.
    #[must_use]
    pub fn end(self) -> f64 {
        self.start + self.width / self.pixels_per_second
    }

    /// The width of the window in pixels.
    #[must_use]
    pub fn width(self) -> f64 {
        self.width
    }

    /// The place of `time` in the window, in pixels from the left edge.
    #[must_use]
    pub fn x_of(self, time: f64) -> f64 {
        (time - self.start) * self.pixels_per_second
    }

    /// The time at `x`, a place in the window in pixels from the left edge.
    #[must_use]
    pub fn time_at(self, x: f64) -> f64 {
        self.start + x / self.pixels_per_second
    }
}

/// A tick of the ruler.
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    /// The place of the tick in the window, in pixels from the left edge.
    pub x: f64,
    /// The time of the tick as text, for a tick that has a label.
    pub label: Option<String>,
}

/// The ticks of the ruler in the window of `frame`, from left to right.
///
/// The step between two labels is the first of `LABEL_STEPS` that leaves
/// `LABEL_SPACING` pixels for each label. Past the last step, the step is
/// two times larger until it leaves the space. Between two labels, the ruler
/// puts the most ticks that keep `MIN_TICK_SPACING` pixels between two ticks.
/// The labels show tenths of a second when the step is below one second.
/// A window that needs more than `MAX_TICKS` ticks gets no tick.
#[must_use]
pub fn ruler_ticks(frame: Frame) -> Vec<Tick> {
    let pixels_per_second = frame.pixels_per_second;
    let Some(step) = label_step(pixels_per_second) else {
        return Vec::new();
    };
    let divisions = TICK_DIVISIONS
        .into_iter()
        .find(|&count| step / f64::from(count) * pixels_per_second >= MIN_TICK_SPACING)
        .unwrap_or(1);
    let tick_step = step / f64::from(divisions);
    // The margin keeps a tick at the edge that the division puts a little
    // past it.
    let first = whole_number((frame.start / tick_step - EDGE_MARGIN).ceil());
    let last = whole_number((frame.end() / tick_step + EDGE_MARGIN).floor());
    let (Some(first), Some(last)) = (first, last) else {
        return Vec::new();
    };
    // `whole_number` holds both ends within 2^53, so the difference fits.
    // The range holds `last - first + 1` ticks.
    if last < first || last - first >= MAX_TICKS {
        return Vec::new();
    }
    let divisions = i64::from(divisions);
    let tenths = step < 1.0;
    (first..=last)
        .map(|index| {
            // The time of the label is a whole count of steps, so its text
            // has no error of the division.
            let label = (index % divisions == 0)
                .then(|| format_clock((index / divisions) as f64 * step, tenths));
            Tick {
                x: frame.x_of(index as f64 * tick_step),
                label,
            }
        })
        .collect()
}

/// The step between two labels of the ruler at `pixels_per_second`, or
/// `None` for a zoom that is not finite and above zero.
fn label_step(pixels_per_second: f64) -> Option<f64> {
    if !pixels_per_second.is_finite() || pixels_per_second <= 0.0 {
        return None;
    }
    let fits = |step: f64| step * pixels_per_second >= LABEL_SPACING;
    if let Some(step) = LABEL_STEPS.into_iter().find(|&step| fits(step)) {
        return Some(step);
    }
    let mut step = LABEL_STEPS[LABEL_STEPS.len() - 1];
    for _ in 0..MAX_STEP_DOUBLINGS {
        step *= 2.0;
        if fits(step) {
            // A step can grow past the range of `f64` before it fits.
            return step.is_finite().then_some(step);
        }
    }
    None
}

/// `value` as an integer, when it is a whole number within 2^53, where an
/// `f64` holds each integer exactly.
fn whole_number(value: f64) -> Option<i64> {
    if value.is_finite() && value.abs() <= MAX_EXACT_INTEGER {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the value is a whole number within 2^53, which i64 holds"
        )]
        let whole = value as i64;
        Some(whole)
    } else {
        None
    }
}

/// A place on the timeline where the terminal size changes from one clip to
/// the next.
#[derive(Debug, Clone, PartialEq)]
pub struct SizeMark {
    /// The start of the clip with the new size, in seconds.
    pub time: f64,
    /// The columns and the rows of the terminal before the change.
    pub from: (u16, u16),
    /// The columns and the rows of the terminal after the change.
    pub to: (u16, u16),
}

/// The marks where the terminal size of `project` changes.
///
/// The clips are in the order of their start on the timeline. A tie goes to
/// the clip that comes first in track order, then in the order on its track,
/// as in the render of the pipeline. A mark is at the start of each clip
/// whose source has a different width or height from the source of the clip
/// before it.
#[must_use]
pub fn size_marks(project: &EditProject) -> Vec<SizeMark> {
    let mut ordered: Vec<&Clip> = project
        .timeline
        .iter()
        .flat_map(|track| &track.clips)
        .filter_map(|id| project.clips.iter().find(|clip| clip.id == *id))
        .collect();
    // A stable sort keeps track order, then clip order, on a tie.
    ordered.sort_by(|a, b| a.timeline_offset.get().total_cmp(&b.timeline_offset.get()));
    let size = |clip: &Clip| (clip.source.width, clip.source.height);
    ordered
        .windows(2)
        .filter_map(|pair| {
            let [before, after] = pair else {
                return None;
            };
            let (from, to) = (size(before), size(after));
            (from != to).then(|| SizeMark {
                time: after.timeline_offset.get(),
                from,
                to,
            })
        })
        .collect()
}

/// The unit of the deltas of a wheel event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelUnit {
    /// The deltas are pixels.
    Pixels,
    /// The deltas are lines of text.
    Lines,
    /// The deltas are pages: widths of the window.
    Pages,
}

/// The parts of a wheel event that the timeline reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wheel {
    /// The horizontal delta. A positive value scrolls to the right.
    pub x: f64,
    /// The vertical delta. A positive value scrolls down.
    pub y: f64,
    /// The unit of `x` and `y`.
    pub unit: WheelUnit,
    /// True when Cmd or Ctrl is down.
    pub command: bool,
    /// True when Shift is down.
    pub shift: bool,
}

/// What a wheel event does to the timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelAction {
    /// Zoom by the factor at the place of the pointer.
    Zoom(f64),
    /// Scroll by the pixels.
    Pan(f64),
    /// The timeline does nothing, and the event keeps its default action.
    Nothing,
}

/// What `wheel` does to a timeline in a window of `width` pixels.
///
/// With Cmd or Ctrl, the vertical delta zooms: down zooms out, and up zooms
/// in. With no modifier, a delta that is mostly horizontal scrolls by its
/// horizontal part. A delta that is mostly vertical keeps its default action,
/// so a trackpad swipe still scrolls the tracks and the page. With Shift, the
/// vertical part of such a delta scrolls, because some systems send Shift with
/// the wheel as a vertical delta. A delta that is not finite does nothing.
#[must_use]
pub fn wheel_action(wheel: Wheel, width: f64) -> WheelAction {
    let scale = match wheel.unit {
        WheelUnit::Pixels => 1.0,
        WheelUnit::Lines => WHEEL_LINE_PIXELS,
        WheelUnit::Pages => width,
    };
    let (x, y) = (wheel.x * scale, wheel.y * scale);
    if !x.is_finite() || !y.is_finite() {
        return WheelAction::Nothing;
    }
    if wheel.command {
        if y == 0.0 {
            return WheelAction::Nothing;
        }
        let factor = (-WHEEL_ZOOM_RATE * y)
            .exp()
            .clamp(1.0 / MAX_WHEEL_ZOOM_FACTOR, MAX_WHEEL_ZOOM_FACTOR);
        return WheelAction::Zoom(factor);
    }
    if x != 0.0 && x.abs() >= y.abs() {
        WheelAction::Pan(x)
    } else if wheel.shift && y != 0.0 {
        WheelAction::Pan(y)
    } else {
        WheelAction::Nothing
    }
}

/// What a key does to the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Zoom in by one step.
    ZoomIn,
    /// Zoom out by one step.
    ZoomOut,
    /// Show the whole project.
    Fit,
}

/// What the key with the text `key` does to the timeline. `+` and `=` zoom in,
/// `-` and `_` zoom out, and `0` shows the whole project. Remotion Studio uses
/// the keys `+`, `-`, and `0`. The other two keys share a key with `+` and `-`
/// on a US keyboard. A key with Cmd, Ctrl, or Alt does nothing, so the
/// shortcuts of the system stay.
#[must_use]
pub fn key_action(key: &str, command: bool) -> Option<KeyAction> {
    if command {
        return None;
    }
    match key {
        "+" | "=" => Some(KeyAction::ZoomIn),
        "-" | "_" => Some(KeyAction::ZoomOut),
        "0" => Some(KeyAction::Fit),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
